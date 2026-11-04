//! The guest's sound, played on the host: the audio port's streams, the jitter buffer that
//! absorbs late deliveries, and the conversion to the output device's format.
//!
//! The guest's audio HAL opens one connection to the audio port for each output stream. It
//! sends sound in the one format the guest uses (48 kHz, stereo, 16-bit), paced by its own
//! clock, and never waits for the host. The host keeps a short queue of it ([`JitterBuffer`]),
//! so a delivery that comes a little late still plays on time, and the platform's player pulls
//! from that queue at the device's pace through a [`Renderer`], which converts to the device's
//! format. When the queue runs dry the device plays silence; nothing ever waits on the guest.
//!
//! Nothing here names a host operating system API. The platform supplies the device through
//! [`AudioDevice`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{
    AudioOutputCommand, AudioOutputOpen, AudioOutputReady, AudioSampleFormat, Envelope, Role,
};
use dsa_protocol::handshake::{host_accept_open, local_hello};
use dsa_protocol::{decode, encode};
use dsa_transport::{Connection, Listener, TransportError};

/// The vsock port the guest's audio HAL connects to.
pub const AUDIO_PORT: u32 = 5568;

/// The one rate the guest sends, in frames per second.
pub const SAMPLE_RATE: u32 = 48_000;

/// The one channel count the guest sends (left, right).
pub const CHANNELS: u16 = 2;

/// Bytes in one frame of the guest's format (two 16-bit samples).
pub const BYTES_PER_FRAME: usize = 4;

/// How much sound the host queues before it starts playing, and again after the queue ran
/// dry: 40 ms. The guest sends 20 ms at a time on its own clock, so a delivery may come up to
/// one burst late from the guest's scheduling alone, and the host's own thread may wake one
/// Windows timer tick (15.6 ms) late. 40 ms covers both with a little to spare, and keeps the
/// whole path (the guest's mixer, this queue and the device) near 100 ms, under the point
/// where a click and its sound feel apart.
pub const TARGET_BUFFER_FRAMES: usize = (SAMPLE_RATE as usize) * 40 / 1000;

/// The most the queue holds: twice the target. The guest's clock and the device's drift
/// apart by a few frames a second; when the queue reaches this, the oldest sound goes and the
/// queue is back at the target, so the delay never grows past 80 ms.
pub const MAX_BUFFER_FRAMES: usize = TARGET_BUFFER_FRAMES * 2;

/// The largest AudioPcm the host takes, in frames: one second. The guest sends 20 ms.
pub const MAX_PCM_FRAMES: usize = SAMPLE_RATE as usize;

/// How long the host waits for the guest's Hello and its AudioOutputOpen.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// What happened to the sound of one stream, for the log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BufferStats {
    /// Frames of silence played because the queue was empty while the stream played.
    pub underrun_frames: u64,
    /// Frames dropped because the queue was full.
    pub dropped_frames: u64,
    /// Frames of silence put in for frames the guest skipped (its own underruns).
    pub gap_frames: u64,
    /// Frames the guest sent.
    pub received_frames: u64,
}

/// The host's queue of one stream's sound, in the guest's format.
///
/// Sound goes in at the guest's pace ([`JitterBuffer::push`]) and comes out at the device's
/// ([`JitterBuffer::pull`]). It starts playing once it holds [`JitterBuffer::target`] frames,
/// and after it runs dry it waits to hold that many again, so one late delivery costs one
/// short silence instead of a crackle on every burst after it.
#[derive(Debug)]
pub struct JitterBuffer {
    /// Interleaved left, right samples.
    samples: VecDeque<i16>,
    target_frames: usize,
    max_frames: usize,
    /// Whether the stream was started and not paused or put in standby.
    playing: bool,
    /// Whether the queue reached the target since it last ran dry.
    primed: bool,
    /// The position of the frame after the last one received, once one was.
    next_position: Option<u64>,
    stats: BufferStats,
}

impl Default for JitterBuffer {
    fn default() -> Self {
        JitterBuffer::new(TARGET_BUFFER_FRAMES, MAX_BUFFER_FRAMES)
    }
}

impl JitterBuffer {
    /// A queue that starts playing at `target_frames` and holds at most `max_frames`
    /// (at least the target).
    pub fn new(target_frames: usize, max_frames: usize) -> JitterBuffer {
        JitterBuffer {
            samples: VecDeque::with_capacity(max_frames.max(target_frames) * CHANNELS as usize),
            target_frames,
            max_frames: max_frames.max(target_frames),
            playing: false,
            primed: false,
            next_position: None,
            stats: BufferStats::default(),
        }
    }

    /// The frames queued before playing starts.
    pub fn target(&self) -> usize {
        self.target_frames
    }

    /// The frames queued now.
    pub fn buffered_frames(&self) -> usize {
        self.samples.len() / CHANNELS as usize
    }

    /// What happened so far.
    pub fn stats(&self) -> BufferStats {
        self.stats
    }

    /// Whether the stream plays (started, and not paused or in standby).
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Queues the frames of `pcm` (whole frames of little-endian 16-bit stereo), which start
    /// at `position` in the stream. Frames the guest skipped since the last push become
    /// silence; frames it sends again are dropped. Returns false, queuing nothing, when `pcm`
    /// is not whole frames.
    pub fn push(&mut self, position: u64, pcm: &[u8]) -> bool {
        if pcm.len() % BYTES_PER_FRAME != 0 {
            return false;
        }
        let mut frames = pcm.len() / BYTES_PER_FRAME;
        let mut skip = 0usize;
        if let Some(next) = self.next_position {
            if position > next {
                // The guest skipped frames: keep the timeline with silence, but never more
                // than the queue can hold.
                let gap = ((position - next) as usize).min(self.max_frames);
                self.samples
                    .resize(self.samples.len() + gap * CHANNELS as usize, 0);
                self.stats.gap_frames += gap as u64;
            } else if position < next {
                // Frames already queued, sent again.
                skip = ((next - position) as usize).min(frames);
            }
        }
        self.stats.received_frames += frames as u64;
        let end = position + frames as u64;
        self.next_position = Some(self.next_position.map_or(end, |next| next.max(end)));
        frames -= skip;
        let start = skip * BYTES_PER_FRAME;
        for sample in pcm[start..start + frames * BYTES_PER_FRAME].chunks_exact(2) {
            self.samples
                .push_back(i16::from_le_bytes([sample[0], sample[1]]));
        }
        let queued = self.buffered_frames();
        if queued > self.max_frames {
            let drop = queued - self.target_frames;
            self.samples.drain(..drop * CHANNELS as usize);
            self.stats.dropped_frames += drop as u64;
        }
        if self.buffered_frames() >= self.target_frames {
            self.primed = true;
        }
        true
    }

    /// Fills `out` (interleaved stereo) with the next frames to play. Whatever the queue
    /// cannot give is silence; this never waits.
    pub fn pull(&mut self, out: &mut [i16]) {
        let wanted = out.len() / CHANNELS as usize;
        if !self.playing || !self.primed {
            out.fill(0);
            return;
        }
        let have = self.buffered_frames().min(wanted);
        for (slot, sample) in out
            .iter_mut()
            .zip(self.samples.drain(..have * CHANNELS as usize))
        {
            *slot = sample;
        }
        out[have * CHANNELS as usize..].fill(0);
        if have < wanted {
            self.stats.underrun_frames += (wanted - have) as u64;
            self.primed = false;
        }
    }

    /// Plays what is queued and what comes.
    pub fn start(&mut self) {
        self.playing = true;
    }

    /// Keeps what is queued but plays nothing.
    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Drops what is queued. The next push starts a new timeline.
    pub fn flush(&mut self) {
        self.samples.clear();
        self.primed = false;
        self.next_position = None;
    }

    /// Drops what is queued and plays nothing.
    pub fn standby(&mut self) {
        self.flush();
        self.playing = false;
    }
}

/// A [`JitterBuffer`] shared between the thread that receives the stream and the device's.
#[derive(Debug, Clone, Default)]
pub struct SharedBuffer(Arc<Mutex<JitterBuffer>>);

impl SharedBuffer {
    pub fn new(buffer: JitterBuffer) -> SharedBuffer {
        SharedBuffer(Arc::new(Mutex::new(buffer)))
    }

    /// The queue. A thread that panicked while holding it left it usable.
    pub fn lock(&self) -> MutexGuard<'_, JitterBuffer> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// How the device takes each sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    /// 32-bit float, -1.0 to 1.0.
    F32,
    /// Signed 16-bit.
    I16,
}

/// The format the output device plays, which the [`Renderer`] converts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample: SampleType,
}

impl DeviceFormat {
    /// Bytes in one frame.
    pub fn bytes_per_frame(&self) -> usize {
        self.channels as usize
            * match self.sample {
                SampleType::F32 => 4,
                SampleType::I16 => 2,
            }
    }
}

/// Converts the guest's stereo 16-bit 48 kHz sound into the device's format as the device
/// asks for it: to its rate (linear interpolation), its channels (left and right go to the
/// first two, a single channel gets their mean, any others are silent) and its sample type.
#[derive(Debug)]
pub struct Renderer {
    format: DeviceFormat,
    source: SharedBuffer,
    /// The source frames being interpolated between, as left, right.
    previous: [i16; 2],
    next: [i16; 2],
    /// How far the output is from `previous` toward `next`, in units of 1/device rate.
    phase: u64,
    /// Frames pulled from the source and not yet used.
    pending: Vec<i16>,
    pending_at: usize,
}

/// Frames the renderer takes from the queue at a time: 5 ms.
const PULL_FRAMES: usize = (SAMPLE_RATE as usize) / 200;

impl Renderer {
    /// A renderer for `format` playing what `source` holds. Fails for a format it cannot
    /// play: no channels or no rate.
    pub fn new(format: DeviceFormat, source: SharedBuffer) -> Result<Renderer, String> {
        if format.channels == 0 || format.sample_rate == 0 {
            return Err(format!(
                "the output device's format has {} channels at {} Hz, which cannot be played",
                format.channels, format.sample_rate
            ));
        }
        Ok(Renderer {
            format,
            source,
            previous: [0; 2],
            next: [0; 2],
            phase: 0,
            pending: Vec::new(),
            pending_at: 0,
        })
    }

    /// The device format this renders.
    pub fn format(&self) -> DeviceFormat {
        self.format
    }

    fn next_source_frame(&mut self) -> [i16; 2] {
        if self.pending_at >= self.pending.len() {
            // Enough for the device's frames at its rate, in pieces no larger than a few
            // milliseconds so the queue does not lose its margin all at once.
            self.pending.resize(PULL_FRAMES * CHANNELS as usize, 0);
            self.source.lock().pull(&mut self.pending);
            self.pending_at = 0;
        }
        let frame = [
            self.pending[self.pending_at],
            self.pending[self.pending_at + 1],
        ];
        self.pending_at += CHANNELS as usize;
        frame
    }

    /// Fills `out` with whole frames of the device's format. Bytes past the last whole frame
    /// are left as they are.
    pub fn render(&mut self, out: &mut [u8]) {
        let frame_bytes = self.format.bytes_per_frame();
        let rate = self.format.sample_rate as u64;
        for frame in out.chunks_exact_mut(frame_bytes) {
            let [left, right] = if self.phase == 0 {
                self.previous.map(f32::from)
            } else {
                let t = self.phase as f32 / rate as f32;
                [0, 1].map(|c| {
                    let a = f32::from(self.previous[c]);
                    let b = f32::from(self.next[c]);
                    a + (b - a) * t
                })
            };
            self.write_frame(frame, left, right);
            self.phase += SAMPLE_RATE as u64;
            while self.phase >= rate {
                self.phase -= rate;
                self.previous = self.next;
                self.next = self.next_source_frame();
            }
        }
    }

    fn write_frame(&self, frame: &mut [u8], left: f32, right: f32) {
        let channels = self.format.channels as usize;
        for channel in 0..channels {
            let value = match (channels, channel) {
                (1, _) => (left + right) / 2.0,
                (_, 0) => left,
                (_, 1) => right,
                _ => 0.0,
            };
            match self.format.sample {
                SampleType::F32 => {
                    let v = (value / 32768.0).to_le_bytes();
                    frame[channel * 4..channel * 4 + 4].copy_from_slice(&v);
                }
                SampleType::I16 => {
                    let v = (value.round().clamp(-32768.0, 32767.0) as i16).to_le_bytes();
                    frame[channel * 2..channel * 2 + 2].copy_from_slice(&v);
                }
            }
        }
    }
}

/// One stream playing on the output device. Dropping it stops the playback and lets the
/// device go idle.
pub trait Playback: Send {
    /// The device's own delay, in frames at the guest's rate, when known.
    fn device_frames(&self) -> u32 {
        0
    }
}

/// The host's output device.
pub trait AudioDevice: Send + Sync {
    /// Starts playing what `source` holds on the user's default output device, following it
    /// when the user picks another. `name` names the stream in the log.
    fn play(&self, name: &str, source: SharedBuffer) -> Result<Box<dyn Playback>, String>;
}

/// How one stream on the audio port ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEnd {
    /// The stream's name, as the guest gave it.
    pub name: String,
    pub stats: BufferStats,
    /// Why it ended, in words for the log.
    pub reason: String,
}

fn refuse_format(open: &AudioOutputOpen) -> Option<String> {
    if open.sample_rate != SAMPLE_RATE
        || open.channel_count != u32::from(CHANNELS)
        || open.format != AudioSampleFormat::S16le as i32
    {
        return Some(format!(
            "the guest asked for {} Hz, {} channels, sample format {}; the host plays only \
             {SAMPLE_RATE} Hz, {CHANNELS} channels, 16-bit",
            open.sample_rate, open.channel_count, open.format
        ));
    }
    None
}

/// Serves one connection on the audio port until the guest closes it: the handshake, the
/// stream's format, then its sound and its state. The device plays only while the stream is
/// started, and what the guest sends never makes this wait or panic: a bad message ends this
/// stream, and nothing else. `log` takes lines for the host's log.
pub fn serve_stream(
    connection: &mut dyn Connection,
    device: &dyn AudioDevice,
    build: &str,
    log: &dyn Fn(&str),
) -> Result<StreamEnd, String> {
    let local = local_hello(Role::Host, build);
    host_accept_open(connection, &local, Role::GuestAudio, OPEN_TIMEOUT)
        .map_err(|e| format!("audio stream refused: {e}"))?;
    let open = match connection.recv(Some(OPEN_TIMEOUT)).map(|f| decode(&f)) {
        Ok(Ok(Envelope {
            body: Some(Body::AudioOutputOpen(open)),
        })) => open,
        Ok(_) => {
            connection.close();
            return Err("the audio stream did not start with its format".into());
        }
        Err(e) => return Err(format!("the audio stream ended before its format: {e}")),
    };
    if let Some(why) = refuse_format(&open) {
        connection.close();
        return Err(why);
    }
    let name = if open.name.is_empty() {
        "unnamed".to_string()
    } else {
        open.name.chars().take(64).collect()
    };
    let source = SharedBuffer::default();
    let mut playback: Option<Box<dyn Playback>> = None;
    let mut device_frames = 0;
    let ready = AudioOutputReady {
        buffered_frames: TARGET_BUFFER_FRAMES as u32,
        device_frames,
    };
    connection
        .send(&encode(&Envelope {
            body: Some(Body::AudioOutputReady(ready)),
        }))
        .map_err(|e| format!("audio stream {name}: {e}"))?;
    log(&format!("audio stream {name} opened"));
    let reason = loop {
        let frame = match connection.recv(None) {
            Ok(frame) => frame,
            Err(TransportError::Timeout) => continue,
            Err(TransportError::Closed) => break "the guest closed it".to_string(),
            Err(e) => break e.to_string(),
        };
        let body = match decode(&frame) {
            Ok(Envelope { body: Some(body) }) => body,
            Ok(Envelope { body: None }) => continue,
            Err(e) => break e.to_string(),
        };
        match body {
            Body::AudioPcm(pcm) => {
                if pcm.data.len() > MAX_PCM_FRAMES * BYTES_PER_FRAME {
                    break format!("a piece of {} bytes is too large", pcm.data.len());
                }
                if !source.lock().push(pcm.position, &pcm.data) {
                    break format!("a piece of {} bytes is not whole frames", pcm.data.len());
                }
            }
            Body::AudioOutputControl(control) => {
                match AudioOutputCommand::try_from(control.command) {
                    Ok(AudioOutputCommand::Start) => {
                        source.lock().start();
                        if playback.is_none() {
                            match device.play(&name, source.clone()) {
                                Ok(p) => {
                                    device_frames = p.device_frames();
                                    playback = Some(p);
                                }
                                // The stream goes on, unheard, so the guest is never held up.
                                Err(e) => log(&format!("audio stream {name}: {e}")),
                            }
                        }
                    }
                    Ok(AudioOutputCommand::Pause) => source.lock().pause(),
                    Ok(AudioOutputCommand::Flush) => source.lock().flush(),
                    Ok(AudioOutputCommand::Standby) => {
                        source.lock().standby();
                        // The device goes idle until the next start.
                        playback = None;
                    }
                    _ => log(&format!(
                        "audio stream {name}: unknown command {}",
                        control.command
                    )),
                }
            }
            other => log(&format!(
                "audio stream {name}: ignored a message that is not for the audio port ({})",
                body_name(&other)
            )),
        }
    };
    drop(playback);
    connection.close();
    let stats = source.lock().stats();
    log(&format!(
        "audio stream {name} closed ({reason}): {} frames received, {} frames of underrun, \
         {} dropped, {} skipped by the guest, device delay {device_frames} frames",
        stats.received_frames, stats.underrun_frames, stats.dropped_frames, stats.gap_frames
    ));
    Ok(StreamEnd {
        name,
        stats,
        reason,
    })
}

fn body_name(body: &Body) -> &'static str {
    match body {
        Body::Hello(_) => "Hello",
        Body::AudioOutputOpen(_) => "AudioOutputOpen",
        Body::AudioOutputReady(_) => "AudioOutputReady",
        _ => "another message",
    }
}

/// The streams that may play at once. Each one costs a thread and a device stream, and the
/// guest opens one or two; more is a misbehaving process in the guest.
pub const MAX_STREAMS: usize = 8;

/// Serves the audio port until `stop` is set: each connection is a stream, served on a
/// thread of its own by [`serve_stream`], so the platform's mixer sums them. A connection
/// past [`MAX_STREAMS`] is closed at once, and the streams already playing go on.
pub fn serve_port(
    listener: &mut dyn Listener,
    device: Arc<dyn AudioDevice>,
    build: &str,
    stop: &AtomicBool,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        match listener.accept(Duration::from_millis(100)) {
            Ok(mut connection) => {
                if active.load(Ordering::SeqCst) >= MAX_STREAMS {
                    log(&format!(
                        "audio port: refused a stream, {MAX_STREAMS} already play"
                    ));
                    connection.close();
                    continue;
                }
                active.fetch_add(1, Ordering::SeqCst);
                let device = Arc::clone(&device);
                let stream_log = Arc::clone(&log);
                let stream_active = Arc::clone(&active);
                let build = build.to_string();
                let spawned = thread::Builder::new()
                    .name("dsa-audio-stream".into())
                    .spawn(move || {
                        if let Err(e) =
                            serve_stream(connection.as_mut(), &*device, &build, &*stream_log)
                        {
                            stream_log(&e);
                        }
                        stream_active.fetch_sub(1, Ordering::SeqCst);
                    });
                if let Err(e) = spawned {
                    // The connection went with the closure; the guest connects again.
                    active.fetch_sub(1, Ordering::SeqCst);
                    log(&format!("audio port: no thread for a stream: {e}"));
                }
            }
            Err(TransportError::Timeout) => {}
            Err(e) => {
                log(&format!("audio port: {e}"));
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}
