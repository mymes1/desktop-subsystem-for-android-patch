//! The guest's sound on the Mac, through Audio Queue Services: an output queue on the
//! system's default output device (which it follows when the user picks another) and an
//! input queue on the default input device.

use std::ffi::c_void;

use dsa_host_core::audio::{
    AudioDevice, CaptureSink, DeviceFormat, Playback, Recording, Renderer, SampleType,
    SharedBuffer,
};

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioQueueNewOutput(
        format: *const StreamDescription,
        callback: extern "C" fn(*mut c_void, *mut c_void, *mut QueueBuffer),
        user_data: *mut c_void,
        run_loop: *const c_void,
        run_loop_mode: *const c_void,
        flags: u32,
        queue: *mut *mut c_void,
    ) -> i32;
    fn AudioQueueNewInput(
        format: *const StreamDescription,
        callback: extern "C" fn(
            *mut c_void,
            *mut c_void,
            *mut QueueBuffer,
            *const c_void,
            u32,
            *const c_void,
        ),
        user_data: *mut c_void,
        run_loop: *const c_void,
        run_loop_mode: *const c_void,
        flags: u32,
        queue: *mut *mut c_void,
    ) -> i32;
    fn AudioQueueAllocateBuffer(queue: *mut c_void, bytes: u32, buffer: *mut *mut QueueBuffer) -> i32;
    fn AudioQueueEnqueueBuffer(
        queue: *mut c_void,
        buffer: *mut QueueBuffer,
        packet_descriptions: u32,
        descriptions: *const c_void,
    ) -> i32;
    fn AudioQueueStart(queue: *mut c_void, start_time: *const c_void) -> i32;
    fn AudioQueueStop(queue: *mut c_void, immediate: u8) -> i32;
    fn AudioQueueDispose(queue: *mut c_void, immediate: u8) -> i32;
}

#[repr(C)]
struct StreamDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct QueueBuffer {
    capacity: u32,
    data: *mut c_void,
    byte_size: u32,
    user_data: *mut c_void,
    packet_description_capacity: u32,
    packet_descriptions: *mut c_void,
    packet_description_count: u32,
}

/// `kAudioFormatLinearPCM`.
const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const FLAG_FLOAT: u32 = 1;
const FLAG_SIGNED_INTEGER: u32 = 4;
const FLAG_PACKED: u32 = 8;
/// Buffers of 10 ms at 48 kHz, three of them: about 20 ms of the device's own delay.
const BUFFER_FRAMES: u32 = 480;
const BUFFERS: usize = 3;

/// The output format: the guest's rate and channels in 32-bit float, the Mac's own sample
/// type, which the renderer converts to.
const OUTPUT: DeviceFormat = DeviceFormat {
    sample_rate: 48_000,
    channels: 2,
    sample: SampleType::F32,
};

fn description(format: DeviceFormat) -> StreamDescription {
    let bytes_per_frame = format.bytes_per_frame() as u32;
    let (flags, bits) = match format.sample {
        SampleType::F32 => (FLAG_FLOAT | FLAG_PACKED, 32),
        SampleType::I16 => (FLAG_SIGNED_INTEGER | FLAG_PACKED, 16),
    };
    StreamDescription {
        sample_rate: f64::from(format.sample_rate),
        format_id: LINEAR_PCM,
        format_flags: flags,
        bytes_per_packet: bytes_per_frame,
        frames_per_packet: 1,
        bytes_per_frame,
        channels_per_frame: u32::from(format.channels),
        bits_per_channel: bits,
        reserved: 0,
    }
}

extern "C" fn fill(user: *mut c_void, queue: *mut c_void, buffer: *mut QueueBuffer) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let renderer = &mut *(user as *mut Renderer);
        let buffer = &mut *buffer;
        let out = std::slice::from_raw_parts_mut(buffer.data as *mut u8, buffer.capacity as usize);
        renderer.render(out);
        buffer.byte_size = buffer.capacity;
        AudioQueueEnqueueBuffer(queue, buffer, 0, std::ptr::null());
    }));
}

extern "C" fn captured(
    user: *mut c_void,
    queue: *mut c_void,
    buffer: *mut QueueBuffer,
    _start: *const c_void,
    _packets: u32,
    _descriptions: *const c_void,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let sink = &mut *(user as *mut CaptureSink);
        let buffer = &mut *buffer;
        let data = std::slice::from_raw_parts(buffer.data as *const u8, buffer.byte_size as usize);
        // Whole frames only: four bytes each.
        let whole = data.len() - data.len() % 4;
        if whole > 0 {
            sink(&data[..whole]);
        }
        AudioQueueEnqueueBuffer(queue, buffer, 0, std::ptr::null());
    }));
}

fn check(status: i32, what: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{what} failed (Core Audio status {status})"))
    }
}

struct Queue<T> {
    queue: *mut c_void,
    /// What the callback reads, freed after the queue is disposed of.
    user: *mut T,
}

// The queue and its callback state are used by Core Audio's thread and dropped here once.
unsafe impl<T> Send for Queue<T> {}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        unsafe {
            AudioQueueStop(self.queue, 1);
            AudioQueueDispose(self.queue, 1);
            drop(Box::from_raw(self.user));
        }
    }
}

impl Playback for Queue<Renderer> {
    fn device_frames(&self) -> u32 {
        BUFFER_FRAMES * BUFFERS as u32
    }
}

impl Recording for Queue<CaptureSink> {
    fn device_frames(&self) -> u32 {
        BUFFER_FRAMES * BUFFERS as u32
    }
}

unsafe fn prime(
    queue: *mut c_void,
    bytes: u32,
    each: impl Fn(*mut QueueBuffer),
) -> Result<(), String> {
    for _ in 0..BUFFERS {
        let mut buffer: *mut QueueBuffer = std::ptr::null_mut();
        check(
            AudioQueueAllocateBuffer(queue, bytes, &mut buffer),
            "making a sound buffer",
        )?;
        each(buffer);
    }
    Ok(())
}

/// The Mac's default sound devices.
#[derive(Debug, Default)]
pub struct CoreAudio;

impl AudioDevice for CoreAudio {
    fn play(&self, _name: &str, source: SharedBuffer) -> Result<Box<dyn Playback>, String> {
        let renderer = Box::into_raw(Box::new(Renderer::new(OUTPUT, source)?));
        unsafe {
            let mut queue: *mut c_void = std::ptr::null_mut();
            let status = AudioQueueNewOutput(
                &description(OUTPUT),
                fill,
                renderer as *mut c_void,
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut queue,
            );
            if status != 0 {
                drop(Box::from_raw(renderer));
                return Err(format!(
                    "the Mac's sound output could not be opened (Core Audio status {status})"
                ));
            }
            let playing = Queue {
                queue,
                user: renderer,
            };
            let bytes = BUFFER_FRAMES * OUTPUT.bytes_per_frame() as u32;
            prime(queue, bytes, |buffer| fill(renderer as *mut c_void, queue, buffer))?;
            check(AudioQueueStart(queue, std::ptr::null()), "starting the sound output")?;
            Ok(Box::new(playing))
        }
    }

    fn record(&self, _name: &str, sink: CaptureSink) -> Result<Box<dyn Recording>, String> {
        let format = DeviceFormat {
            sample_rate: 48_000,
            channels: 2,
            sample: SampleType::I16,
        };
        let user = Box::into_raw(Box::new(sink));
        unsafe {
            let mut queue: *mut c_void = std::ptr::null_mut();
            let status = AudioQueueNewInput(
                &description(format),
                captured,
                user as *mut c_void,
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut queue,
            );
            if status != 0 {
                drop(Box::from_raw(user));
                return Err(format!(
                    "the Mac's microphone could not be opened: the user may not have allowed DSA \
                     to use it, in System Settings, Privacy & Security, Microphone (Core Audio \
                     status {status})"
                ));
            }
            let recording = Queue { queue, user };
            let bytes = BUFFER_FRAMES * format.bytes_per_frame() as u32;
            prime(queue, bytes, |buffer| {
                AudioQueueEnqueueBuffer(queue, buffer, 0, std::ptr::null());
            })?;
            check(AudioQueueStart(queue, std::ptr::null()), "starting the microphone")?;
            Ok(Box::new(recording))
        }
    }
}
