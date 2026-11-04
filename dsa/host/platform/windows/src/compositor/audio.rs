//! The guest's sound on the user's default output device, through WASAPI. The guest's
//! microphone, the default capture device, is in [`super::microphone`].
//!
//! The window manager runs in the user's session, so the sound plays where the user hears
//! it. Each stream of the audio port gets a shared-mode client of the default output device
//! in the guest's own format (48 kHz, stereo, 16-bit); Windows converts to the device's mix
//! format and mixes it with every other program. A thread per stream fills the device's
//! buffer each time the device asks, from the stream's jitter buffer, and never waits on
//! the guest. When the user picks another default device, or the device goes away, the
//! stream moves to the new default within a second.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::audio::{
    AudioDevice, CaptureSink, DeviceFormat, Playback, Recording, Renderer, SampleType,
    SharedBuffer, CHANNELS, SAMPLE_RATE,
};
use windows::core::PWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
    WAVE_FORMAT_PCM,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

/// The guest's format, which the client asks Windows to convert from.
pub(super) const FORMAT: DeviceFormat = DeviceFormat {
    sample_rate: SAMPLE_RATE,
    channels: CHANNELS,
    sample: SampleType::I16,
};

/// The device buffer each client asks for: 20 ms, in 100 ns units.
const BUFFER_DURATION: i64 = 20 * 10_000;

/// How often the stream checks which device is the default.
const DEFAULT_CHECK: Duration = Duration::from_secs(1);

/// How long the stream waits before it opens a device again after one failed.
const RETRY: Duration = Duration::from_millis(500);

/// The user's default output device, and their default capture device for the guest's
/// microphone.
pub struct DefaultOutput {
    log: Arc<dyn Fn(&str) + Send + Sync>,
}

impl DefaultOutput {
    /// Plays on the default output device; `log` takes lines for the window manager's log.
    pub fn new(log: Arc<dyn Fn(&str) + Send + Sync>) -> DefaultOutput {
        DefaultOutput { log }
    }
}

struct WasapiPlayback {
    stop: Arc<AtomicBool>,
    device_frames: Arc<AtomicU32>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Playback for WasapiPlayback {
    fn device_frames(&self) -> u32 {
        self.device_frames.load(Ordering::Relaxed)
    }
}

impl Drop for WasapiPlayback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl AudioDevice for DefaultOutput {
    fn play(&self, name: &str, source: SharedBuffer) -> Result<Box<dyn Playback>, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let device_frames = Arc::new(AtomicU32::new(0));
        let (opened_tx, opened_rx) = mpsc::channel();
        let thread = {
            let stop = Arc::clone(&stop);
            let device_frames = Arc::clone(&device_frames);
            let log = Arc::clone(&self.log);
            let name = name.to_string();
            thread::Builder::new()
                .name("dsa-audio-render".into())
                .spawn(move || render(&name, source, &stop, &device_frames, opened_tx, &*log))
                .map_err(|e| format!("no thread to play on: {e}"))?
        };
        let playback = WasapiPlayback {
            stop,
            device_frames,
            thread: Some(thread),
        };
        // The first device either opens or says why not. Later failures are the thread's to
        // recover from.
        match opened_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Box::new(playback)),
            Ok(Err(why)) => Err(why),
            Err(_) => Err("the default output device did not open in time".into()),
        }
    }

    fn record(&self, name: &str, sink: CaptureSink) -> Result<Box<dyn Recording>, String> {
        super::microphone::record(name, sink, Arc::clone(&self.log))
    }
}

/// COM on this thread, for as long as it lives.
pub(super) struct Com;

impl Com {
    pub(super) fn enter() -> Result<Com, String> {
        // SAFETY: initialises COM for this thread; undone on drop.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| format!("COM did not start: {e}"))?;
        Ok(Com)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx of this thread.
        unsafe { CoUninitialize() };
    }
}

/// An event the device sets each time it wants more sound.
pub(super) struct Event(pub(super) windows_sys::Win32::Foundation::HANDLE);

impl Event {
    pub(super) fn new() -> Result<Event, String> {
        // SAFETY: an unnamed auto-reset event with default security.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if handle.is_null() {
            return Err(format!(
                "no event for the device: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Event(handle))
    }

    /// Whether the event was set within `timeout`.
    pub(super) fn wait(&self, timeout: Duration) -> bool {
        // SAFETY: a valid event handle this struct owns.
        unsafe { WaitForSingleObject(self.0, timeout.as_millis() as u32) == WAIT_OBJECT_0 }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle CreateEventW returned, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// One shared-mode client of one device, started.
struct Client {
    id: String,
    client: IAudioClient,
    render: IAudioRenderClient,
    buffer_frames: u32,
    event: Event,
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: a started client of this thread's COM.
        let _ = unsafe { self.client.Stop() };
    }
}

pub(super) fn device_id(device: &IMMDevice) -> Result<String, String> {
    // SAFETY: a device of this thread's COM; the string is freed below.
    let id: PWSTR = unsafe { device.GetId() }.map_err(|e| format!("the device has no ID: {e}"))?;
    // SAFETY: GetId returned a valid NUL-terminated string.
    let text = unsafe { id.to_string() }.unwrap_or_default();
    // SAFETY: GetId's string, allocated with the COM allocator.
    unsafe { CoTaskMemFree(Some(id.0 as *const _)) };
    Ok(text)
}

fn default_device(enumerator: &IMMDeviceEnumerator) -> Result<IMMDevice, String> {
    // SAFETY: an enumerator of this thread's COM.
    unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|e| format!("there is no default output device: {e}"))
}

fn open(enumerator: &IMMDeviceEnumerator) -> Result<(Client, u32), String> {
    let device = default_device(enumerator)?;
    let id = device_id(&device)?;
    // SAFETY: activating the device's audio client on this thread's COM.
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| format!("the default output device has no audio client: {e}"))?;
    let block_align = FORMAT.bytes_per_frame() as u16;
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: FORMAT.channels,
        nSamplesPerSec: FORMAT.sample_rate,
        nAvgBytesPerSec: FORMAT.sample_rate * u32::from(block_align),
        nBlockAlign: block_align,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    // SAFETY: a valid format; Windows converts it to the device's mix format.
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            BUFFER_DURATION,
            0,
            &format,
            None,
        )
    }
    .map_err(|e| format!("the default output device refused the guest's format: {e}"))?;
    let event = Event::new()?;
    // SAFETY: the event outlives the client, which this struct drops first.
    unsafe { client.SetEventHandle(HANDLE(event.0)) }
        .map_err(|e| format!("the device takes no event: {e}"))?;
    // SAFETY: an initialised client.
    let buffer_frames =
        unsafe { client.GetBufferSize() }.map_err(|e| format!("the device has no buffer: {e}"))?;
    // SAFETY: an initialised client.
    let render: IAudioRenderClient =
        unsafe { client.GetService() }.map_err(|e| format!("the device takes no sound: {e}"))?;
    // The device's own delay, at the guest's rate (the stream's rate before conversion).
    // SAFETY: an initialised client.
    let latency = unsafe { client.GetStreamLatency() }.unwrap_or(0).max(0) as u64;
    let device_frames =
        (latency * u64::from(SAMPLE_RATE) / 10_000_000) as u32 + buffer_frames.min(SAMPLE_RATE);
    // SAFETY: an initialised client.
    unsafe { client.Start() }.map_err(|e| format!("the device did not start: {e}"))?;
    Ok((
        Client {
            id,
            client,
            render,
            buffer_frames,
            event,
        },
        device_frames,
    ))
}

/// Fills what the device has room for. Returns false when the device went away.
fn fill(client: &Client, renderer: &mut Renderer) -> bool {
    // SAFETY: a started client.
    let padding = match unsafe { client.client.GetCurrentPadding() } {
        Ok(padding) => padding,
        Err(_) => return false,
    };
    let frames = client.buffer_frames.saturating_sub(padding);
    if frames == 0 {
        return true;
    }
    // SAFETY: the device's buffer has room for `frames` frames of the client's format.
    let data = match unsafe { client.render.GetBuffer(frames) } {
        Ok(data) => data,
        Err(_) => return false,
    };
    let bytes = frames as usize * FORMAT.bytes_per_frame();
    // SAFETY: GetBuffer returned `bytes` writable bytes, released below.
    let out = unsafe { std::slice::from_raw_parts_mut(data, bytes) };
    renderer.render(out);
    // SAFETY: releases the frames GetBuffer gave.
    unsafe { client.render.ReleaseBuffer(frames, 0) }.is_ok()
}

fn render(
    name: &str,
    source: SharedBuffer,
    stop: &AtomicBool,
    device_frames: &AtomicU32,
    opened: mpsc::Sender<Result<(), String>>,
    log: &(dyn Fn(&str) + Send + Sync),
) {
    let mut opened = Some(opened);
    let report = |opened: &mut Option<mpsc::Sender<Result<(), String>>>, result| {
        if let Some(tx) = opened.take() {
            let _ = tx.send(result);
        }
    };
    let _com = match Com::enter() {
        Ok(com) => com,
        Err(e) => return report(&mut opened, Err(e)),
    };
    // SAFETY: COM is entered on this thread.
    let enumerator: IMMDeviceEnumerator =
        match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
            Ok(enumerator) => enumerator,
            Err(e) => {
                return report(
                    &mut opened,
                    Err(format!("the audio devices cannot be listed: {e}")),
                )
            }
        };
    let mut renderer = match Renderer::new(FORMAT, source) {
        Ok(renderer) => renderer,
        Err(e) => return report(&mut opened, Err(e)),
    };
    while !stop.load(Ordering::Relaxed) {
        let client = match open(&enumerator) {
            Ok((client, frames)) => {
                device_frames.store(frames, Ordering::Relaxed);
                report(&mut opened, Ok(()));
                log(&format!(
                    "audio stream {name} plays on {} ({} frames of device buffer)",
                    client.id, client.buffer_frames
                ));
                client
            }
            Err(e) => {
                if opened.is_some() {
                    return report(&mut opened, Err(e));
                }
                log(&format!("audio stream {name}: {e}"));
                thread::sleep(RETRY);
                continue;
            }
        };
        let mut checked = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            client.event.wait(Duration::from_millis(100));
            if !fill(&client, &mut renderer) {
                log(&format!(
                    "audio stream {name}: the output device went away; moving to the default"
                ));
                break;
            }
            if checked.elapsed() >= DEFAULT_CHECK {
                checked = Instant::now();
                let moved = default_device(&enumerator)
                    .and_then(|d| device_id(&d))
                    .map(|id| id != client.id)
                    .unwrap_or(false);
                if moved {
                    log(&format!(
                        "audio stream {name}: the default output device changed"
                    ));
                    break;
                }
            }
        }
        drop(client);
    }
}
