//! The guest's microphone: the user's default capture device, through WASAPI.
//!
//! An input stream of the audio port opens a shared-mode client of the default capture
//! device in the guest's own format (48 kHz, stereo, 16-bit); Windows converts from the
//! device's mix format. A thread per stream takes each packet the device records and hands
//! it on without waiting. The device is open only while the guest's stream is: dropping the
//! recording stops the thread and releases the device, and with it the microphone indicator
//! on the taskbar.
//!
//! Windows applies its own privacy setting: when the user turned the microphone off for
//! desktop apps, the client does not initialise (access denied), and the guest's stream does
//! not open, with the reason. Android's own permission is checked in the guest before any
//! stream is opened, so both apply.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::audio::{CaptureSink, Recording, BYTES_PER_FRAME, SAMPLE_RATE};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
    WAVE_FORMAT_PCM,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use super::audio::{device_id, Com, Event, FORMAT};

/// The device buffer each client asks for: 20 ms, in 100 ns units.
const BUFFER_DURATION: i64 = 20 * 10_000;

/// How often the stream checks which device is the default.
const DEFAULT_CHECK: Duration = Duration::from_secs(1);

/// How long the stream waits before it opens a device again after one went away.
const RETRY: Duration = Duration::from_millis(500);

/// The packet flag that says the device recorded silence and the data is to be ignored
/// (AUDCLNT_BUFFERFLAGS_SILENT).
const BUFFER_SILENT: u32 = 0x2;

/// E_ACCESSDENIED: what the client returns while the privacy setting keeps desktop apps from
/// the microphone.
const ACCESS_DENIED: u32 = 0x8007_0005;

/// E_NOTFOUND: there is no capture device.
const NOT_FOUND: u32 = 0x8007_0490;

struct WasapiRecording {
    stop: Arc<AtomicBool>,
    device_frames: Arc<AtomicU32>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Recording for WasapiRecording {
    fn device_frames(&self) -> u32 {
        self.device_frames.load(Ordering::Relaxed)
    }
}

impl Drop for WasapiRecording {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts recording the default capture device into `sink` for the stream `name`. Returns
/// once the device records, or with the reason it does not.
pub fn record(
    name: &str,
    sink: CaptureSink,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) -> Result<Box<dyn Recording>, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let device_frames = Arc::new(AtomicU32::new(0));
    let (opened_tx, opened_rx) = mpsc::channel();
    let thread = {
        let stop = Arc::clone(&stop);
        let device_frames = Arc::clone(&device_frames);
        let name = name.to_string();
        thread::Builder::new()
            .name("dsa-audio-capture".into())
            .spawn(move || capture(&name, sink, &stop, &device_frames, opened_tx, &*log))
            .map_err(|e| format!("no thread to record on: {e}"))?
    };
    let recording = WasapiRecording {
        stop,
        device_frames,
        thread: Some(thread),
    };
    match opened_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(Box::new(recording)),
        // Dropping the recording joins the thread, which has already ended.
        Ok(Err(why)) => Err(why),
        Err(_) => Err("the default capture device did not open in time".into()),
    }
}

/// Words for why the capture device does not record.
fn explain(what: &str, e: &windows::core::Error) -> String {
    match e.code().0 as u32 {
        ACCESS_DENIED => "Windows lets no desktop app use the microphone; turn on microphone \
                          access for desktop apps in Settings, Privacy & security, Microphone"
            .into(),
        NOT_FOUND => "there is no microphone".into(),
        _ => format!("{what}: {e}"),
    }
}

/// One shared-mode client of one capture device, started.
struct Client {
    id: String,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Event,
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: a started client of this thread's COM.
        let _ = unsafe { self.client.Stop() };
    }
}

fn default_device(enumerator: &IMMDeviceEnumerator) -> Result<IMMDevice, String> {
    // SAFETY: an enumerator of this thread's COM.
    unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) }
        .map_err(|e| explain("there is no default capture device", &e))
}

fn open(enumerator: &IMMDeviceEnumerator) -> Result<(Client, u32), String> {
    let device = default_device(enumerator)?;
    let id = device_id(&device)?;
    // SAFETY: activating the device's audio client on this thread's COM.
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| explain("the default capture device has no audio client", &e))?;
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
    // SAFETY: a valid format; Windows converts the device's mix format to it.
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
    .map_err(|e| explain("the default capture device refused the guest's format", &e))?;
    let event = Event::new()?;
    // SAFETY: the event outlives the client, which this struct drops first.
    unsafe { client.SetEventHandle(HANDLE(event.0)) }
        .map_err(|e| format!("the capture device takes no event: {e}"))?;
    // SAFETY: an initialised client.
    let capture: IAudioCaptureClient = unsafe { client.GetService() }
        .map_err(|e| format!("the capture device gives no sound: {e}"))?;
    // SAFETY: an initialised client.
    let latency = unsafe { client.GetStreamLatency() }.unwrap_or(0).max(0) as u64;
    let device_frames = (latency * u64::from(SAMPLE_RATE) / 10_000_000) as u32;
    // SAFETY: an initialised client.
    unsafe { client.Start() }.map_err(|e| explain("the capture device did not start", &e))?;
    Ok((
        Client {
            id,
            client,
            capture,
            event,
        },
        device_frames,
    ))
}

/// Hands every packet the device holds to `sink`. Returns false when the device went away.
fn drain(client: &Client, sink: &mut CaptureSink, silence: &mut Vec<u8>) -> bool {
    loop {
        // SAFETY: a started client.
        let packet = match unsafe { client.capture.GetNextPacketSize() } {
            Ok(frames) => frames,
            Err(_) => return false,
        };
        if packet == 0 {
            return true;
        }
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        // SAFETY: the out pointers are valid; the packet is released below.
        if unsafe {
            client
                .capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
        }
        .is_err()
        {
            return false;
        }
        let bytes = frames as usize * BYTES_PER_FRAME;
        if bytes > 0 {
            if flags & BUFFER_SILENT != 0 || data.is_null() {
                silence.clear();
                silence.resize(bytes, 0);
                sink(&silence[..]);
            } else {
                // SAFETY: GetBuffer gave `frames` frames of the client's format at `data`,
                // readable until ReleaseBuffer.
                sink(unsafe { std::slice::from_raw_parts(data, bytes) });
            }
        }
        // SAFETY: releases the packet GetBuffer gave.
        if unsafe { client.capture.ReleaseBuffer(frames) }.is_err() {
            return false;
        }
    }
}

fn capture(
    name: &str,
    mut sink: CaptureSink,
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
    let mut silence = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let client = match open(&enumerator) {
            Ok((client, frames)) => {
                device_frames.store(frames, Ordering::Relaxed);
                report(&mut opened, Ok(()));
                log(&format!("audio input {name} records from {}", client.id));
                client
            }
            Err(e) => {
                // The first device either opens or tells the guest why not. Later, the
                // stream goes on silent until a device opens again.
                if opened.is_some() {
                    return report(&mut opened, Err(e));
                }
                log(&format!("audio input {name}: {e}"));
                thread::sleep(RETRY);
                continue;
            }
        };
        let mut checked = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            client.event.wait(Duration::from_millis(100));
            if !drain(&client, &mut sink, &mut silence) {
                log(&format!(
                    "audio input {name}: the capture device went away; moving to the default"
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
                        "audio input {name}: the default capture device changed"
                    ));
                    break;
                }
            }
        }
        drop(client);
    }
}
