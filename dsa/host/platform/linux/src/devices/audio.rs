//! The guest's sound on the user's speakers, and the user's microphone in the guest, through
//! PulseAudio's simple API.
//!
//! PulseAudio's API is what every Linux desktop speaks: PipeWire serves it through
//! `pipewire-pulse`, and PulseAudio itself where it still runs. The library is loaded when a
//! stream opens (`libpulse-simple.so.0`), so a computer without it still runs DSA, without
//! sound. Each stream goes to the user's default device and follows it when the user picks
//! another, because no device is named.
//!
//! The guest's format (48 kHz, stereo, 16-bit) is what each stream asks for; the sound
//! server converts it to the device's. A microphone the user has muted or a computer that has
//! none fails the stream with the server's reason, which the guest is told.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};

use dsa_host_core::audio::{
    AudioDevice, CaptureSink, DeviceFormat, Playback, Recording, Renderer, SampleType, SharedBuffer,
};

/// The guest's rate.
const RATE: u32 = 48_000;
/// The guest's channels.
const CHANNELS: u8 = 2;
/// Bytes in one frame of the guest's format.
const FRAME_BYTES: usize = 4;
/// Frames written or read at a time: 10 ms.
const CHUNK_FRAMES: usize = (RATE as usize) / 100;
/// How much the server keeps queued for playback, in bytes: 40 ms. Short, so the guest's own
/// timing stays close to what is heard.
const TARGET_BYTES: u32 = (RATE / 25) * FRAME_BYTES as u32;

/// `PA_SAMPLE_S16LE`.
const PA_SAMPLE_S16LE: c_int = 3;
/// `PA_STREAM_PLAYBACK`.
const PA_STREAM_PLAYBACK: c_int = 1;
/// `PA_STREAM_RECORD`.
const PA_STREAM_RECORD: c_int = 2;

#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

type NewFn = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    c_int,
    *const c_char,
    *const c_char,
    *const SampleSpec,
    *const c_void,
    *const BufferAttr,
    *mut c_int,
) -> *mut c_void;
type IoFn = unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int;
type ReadFn = unsafe extern "C" fn(*mut c_void, *mut c_void, usize, *mut c_int) -> c_int;
type LatencyFn = unsafe extern "C" fn(*mut c_void, *mut c_int) -> u64;
type FreeFn = unsafe extern "C" fn(*mut c_void);
type StrErrorFn = unsafe extern "C" fn(c_int) -> *const c_char;

/// The functions of the simple API this uses.
struct Pulse {
    new: NewFn,
    write: IoFn,
    read: ReadFn,
    latency: LatencyFn,
    free: FreeFn,
    strerror: Option<StrErrorFn>,
}

/// Looks up `name` in the library `handle`.
///
/// # Safety
///
/// `T` must be the function pointer type of the symbol.
unsafe fn symbol<T: Copy>(handle: *mut c_void, name: &CStr) -> Option<T> {
    // SAFETY: a library handle dlopen returned and a NUL-terminated name.
    let address = unsafe { libc::dlsym(handle, name.as_ptr()) };
    if address.is_null() {
        return None;
    }
    // SAFETY: the caller names the symbol's type; a pointer is the size of a function pointer.
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&address) })
}

fn open_library(name: &CStr) -> *mut c_void {
    // SAFETY: a NUL-terminated library name. The library stays loaded for the process.
    unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) }
}

/// The simple API, loaded once.
fn pulse() -> Result<&'static Pulse, String> {
    static PULSE: OnceLock<Result<Pulse, String>> = OnceLock::new();
    PULSE
        .get_or_init(|| {
            let simple = open_library(c"libpulse-simple.so.0");
            if simple.is_null() {
                return Err(
                    "this computer has no PulseAudio client library (libpulse-simple.so.0); \
                     install PipeWire's PulseAudio server or PulseAudio"
                        .into(),
                );
            }
            let common = open_library(c"libpulse.so.0");
            let missing = || "libpulse-simple.so.0 lacks a function DSA uses".to_string();
            // SAFETY: each symbol is looked up with the type the simple API declares for it.
            unsafe {
                Ok(Pulse {
                    new: symbol(simple, c"pa_simple_new").ok_or_else(missing)?,
                    write: symbol(simple, c"pa_simple_write").ok_or_else(missing)?,
                    read: symbol(simple, c"pa_simple_read").ok_or_else(missing)?,
                    latency: symbol(simple, c"pa_simple_get_latency").ok_or_else(missing)?,
                    free: symbol(simple, c"pa_simple_free").ok_or_else(missing)?,
                    strerror: if common.is_null() {
                        None
                    } else {
                        symbol(common, c"pa_strerror")
                    },
                })
            }
        })
        .as_ref()
        .map_err(Clone::clone)
}

impl Pulse {
    fn describe(&self, error: c_int) -> String {
        if let Some(strerror) = self.strerror {
            // SAFETY: pa_strerror returns a static string or null.
            let text = unsafe { strerror(error) };
            if !text.is_null() {
                // SAFETY: a NUL-terminated static string.
                return unsafe { CStr::from_ptr(text) }
                    .to_string_lossy()
                    .into_owned();
            }
        }
        format!("PulseAudio error {error}")
    }
}

/// One open stream of the simple API.
struct Stream {
    pulse: &'static Pulse,
    handle: *mut c_void,
}

// SAFETY: a simple API stream is used by one thread at a time: it is opened on the caller's
// and then used only by the stream's own thread, which frees it.
unsafe impl Send for Stream {}

impl Stream {
    fn open(direction: c_int, name: &str) -> Result<Stream, String> {
        let pulse = pulse()?;
        let spec = SampleSpec {
            format: PA_SAMPLE_S16LE,
            rate: RATE,
            channels: CHANNELS,
        };
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: TARGET_BYTES,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: (CHUNK_FRAMES * FRAME_BYTES) as u32,
        };
        let stream_name = CString::new(name.replace('\0', " ")).unwrap_or_default();
        let mut error: c_int = 0;
        // SAFETY: NUL-terminated strings and structures that outlive the call; null asks for
        // the default server, device and channel map.
        let handle = unsafe {
            (pulse.new)(
                std::ptr::null(),
                c"Desktop Subsystem for Android".as_ptr(),
                direction,
                std::ptr::null(),
                stream_name.as_ptr(),
                &spec,
                std::ptr::null(),
                &attr,
                &mut error,
            )
        };
        if handle.is_null() {
            return Err(pulse.describe(error));
        }
        Ok(Stream { pulse, handle })
    }

    /// The sound still to be heard, in frames at the guest's rate.
    fn latency_frames(&self) -> u32 {
        let mut error: c_int = 0;
        // SAFETY: an open stream.
        let micros = unsafe { (self.pulse.latency)(self.handle, &mut error) };
        u32::try_from(micros * u64::from(RATE) / 1_000_000).unwrap_or(u32::MAX)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: an open stream, freed once.
        unsafe { (self.pulse.free)(self.handle) };
    }
}

/// The user's sound devices, through the sound server.
pub struct PulseAudio;

/// A stream's thread, stopped and waited for when it is dropped.
struct Running {
    stop: Arc<AtomicBool>,
    delay: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    fn start(
        name: &str,
        stream: Stream,
        run: impl FnOnce(Stream, &AtomicBool, &AtomicU32) + Send + 'static,
    ) -> Result<Running, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let delay = Arc::new(AtomicU32::new(0));
        let (thread_stop, thread_delay) = (Arc::clone(&stop), Arc::clone(&delay));
        let thread = thread::Builder::new()
            .name(format!("dsa-audio-{name}"))
            .spawn(move || run(stream, &thread_stop, &thread_delay))
            .map_err(|e| format!("cannot start the stream's thread: {e}"))?;
        Ok(Running {
            stop,
            delay,
            thread: Some(thread),
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Playback for Running {
    fn device_frames(&self) -> u32 {
        self.delay.load(Ordering::Relaxed)
    }
}

impl Recording for Running {
    fn device_frames(&self) -> u32 {
        self.delay.load(Ordering::Relaxed)
    }
}

/// The format the stream is opened in, which the renderer fills.
pub fn stream_format() -> DeviceFormat {
    DeviceFormat {
        sample_rate: RATE,
        channels: u16::from(CHANNELS),
        sample: SampleType::I16,
    }
}

impl AudioDevice for PulseAudio {
    fn play(&self, name: &str, source: SharedBuffer) -> Result<Box<dyn Playback>, String> {
        let mut renderer = Renderer::new(stream_format(), source)?;
        let stream = Stream::open(PA_STREAM_PLAYBACK, name)?;
        let running = Running::start(name, stream, move |stream, stop, delay| {
            let mut chunk = vec![0u8; CHUNK_FRAMES * FRAME_BYTES];
            while !stop.load(Ordering::Relaxed) {
                renderer.render(&mut chunk);
                let mut error: c_int = 0;
                // SAFETY: an open stream and a buffer of the length passed. The call returns
                // once the server has room, which paces this loop at the device's rate.
                let written = unsafe {
                    (stream.pulse.write)(
                        stream.handle,
                        chunk.as_ptr().cast(),
                        chunk.len(),
                        &mut error,
                    )
                };
                if written < 0 {
                    eprintln!("sound: playback ended: {}", stream.pulse.describe(error));
                    break;
                }
                delay.store(stream.latency_frames(), Ordering::Relaxed);
            }
        })?;
        Ok(Box::new(running))
    }

    fn record(&self, name: &str, mut sink: CaptureSink) -> Result<Box<dyn Recording>, String> {
        let stream = Stream::open(PA_STREAM_RECORD, name)
            .map_err(|why| format!("the microphone cannot be opened: {why}"))?;
        let running = Running::start(name, stream, move |stream, stop, delay| {
            let mut chunk = vec![0u8; CHUNK_FRAMES * FRAME_BYTES];
            while !stop.load(Ordering::Relaxed) {
                let mut error: c_int = 0;
                // SAFETY: an open stream and a buffer of the length passed; the call returns
                // once that much has been recorded.
                let read = unsafe {
                    (stream.pulse.read)(
                        stream.handle,
                        chunk.as_mut_ptr().cast(),
                        chunk.len(),
                        &mut error,
                    )
                };
                if read < 0 {
                    eprintln!("sound: recording ended: {}", stream.pulse.describe(error));
                    break;
                }
                delay.store(stream.latency_frames(), Ordering::Relaxed);
                sink(&chunk);
            }
        })?;
        Ok(Box::new(running))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_streams_open_in_the_guests_format() {
        let format = stream_format();
        assert_eq!(format.sample_rate, 48_000);
        assert_eq!(format.channels, 2);
        assert_eq!(format.bytes_per_frame(), FRAME_BYTES);
        assert_eq!(CHUNK_FRAMES * FRAME_BYTES, 1920);
    }

    #[test]
    fn fr26_the_simple_api_structures_have_the_c_layout() {
        assert_eq!(std::mem::size_of::<SampleSpec>(), 12);
        assert_eq!(std::mem::size_of::<BufferAttr>(), 20);
    }
}
