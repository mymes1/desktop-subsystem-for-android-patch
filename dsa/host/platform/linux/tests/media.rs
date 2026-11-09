//! Sound, codecs and root approval in an ordinary app on the real image, with SELinux enforcing:
//! the service boots the image under crosvm, the test installs the media probe
//! (`dsa/guest/media-probe/`) through adb (developer mode) and runs its instrumentation, which
//! plays sound and encodes and decodes H.264 and AAC in the app's own process.
//!
//! An app runs in the untrusted app domain at an MLS level of its own, and its sound goes
//! through memory it shares with the audio server: a streaming track's control block, which
//! the audio server makes, and a static track's buffer, which the app makes. When the guest's
//! policy refused either, `AudioTrack` failed with "Could not get control block" and no app
//! played a sound, while the platform's own sounds, made at the services' level, still played.
//!
//! Every codec's input buffers are memory the framework allocates in the app's process, the
//! way the device's default codec store says to allocate it. When DSA's store (dsa-codec2)
//! answered that it knew how and then refused to say, every `MediaCodec.start()` failed with
//! NO_MEMORY, the software codecs' too, whatever SELinux's mode. The probe decodes with every
//! decoder the guest lists for each type, so a host that offers its GPU's decoder has that one
//! checked as well.
//!
//! The test serves the guest's video port itself, with a host decoder whose pictures are the
//! probe's colour bars in a padded surface ([`PatternDecoders`]), so the guest lists DSA's
//! decoders as it does on a host with a GPU decoder. The probe's video check then decodes
//! clips at 426x240 and 360x640 with those and the guest's own decoders, to buffers and to
//! surfaces on screen, and fails on green or wrong colours, on pictures that stop reaching the
//! screen when a player or a feed of short videos changes surface, and on a sound that stops.
//!
//! The test also reads the guest's log for a denial of a media service's shared memory, and
//! for the codec HAL losing a picture because its surface gave it no buffer.
//!
//! Runs when CI gives it what it needs: `DSA_CROSVM` names the crosvm to run,
//! `DSA_WIDGET_TEST_IMAGE` the image folder built from the commit under test,
//! `DSA_WIDGET_TEST_KERNEL` the kernel to boot it with when the folder carries none,
//! `DSA_MEDIA_PROBE` the probe's APK, and `DSA_ADB` the adb to reach the guest with (`adb` on
//! the path when not set). It needs `/dev/kvm` and `/dev/vhost-vsock`. What it read stays in
//! its folder (`.scratch/media-<pid>`) for the job to keep.
#![cfg(target_os = "linux")]

mod boot_check;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use boot_check::{lock, run_bounded, Process as Service, Watchdog};

use dsa_host_core::guest_log::LogFilter;
use dsa_host_core::host_api::HostApiClient;
use dsa_host_core::session::SessionState;
use dsa_host_core::video::{
    Codec, CodecSupport, Packet, Picture, VideoDecoder, VideoDecoders, VIDEO_PORT,
};
use dsa_host_core::video_layout::SurfaceFormat;
use dsa_host_linux::devices::relay::{device_socket, DeviceSocketListener};
use dsa_host_unix::socket::UnixLocalConnector;

const PROBE_PACKAGE: &str = "io.github.thisisthepy.dsa.mediaprobe";
const PROBE_RUNNER: &str = "io.github.thisisthepy.dsa.mediaprobe/.MediaProbe";
/// `AudioTrack.STATE_INITIALIZED`.
const TRACK_INITIALIZED: i64 = 1;
/// The tone the probe plays: a second at 48 kHz.
const TONE_FRAMES: i64 = 48_000;
/// The guest's own decoders, which every image has.
const SOFTWARE_DECODERS: &[(&str, &str)] = &[
    ("avc", "c2.android.avc.decoder"),
    ("aac", "c2.android.aac.decoder"),
];
/// The memfd types of the media services: a denial naming one means an app or a service could
/// not use memory another process shared with it.
const SHARED_MEMORY_TYPES: &[&str] = &[
    "audioserver_tmpfs",
    "mediaserver_tmpfs",
    "mediaextractor_tmpfs",
    "cameraserver_tmpfs",
    "appdomain_tmpfs",
    // The sensors HAL's event queue, which system_server makes.
    "system_server_tmpfs",
    // The codec services' buffer pool queues.
    "dsa_codec2_tmpfs",
];
/// The media check, boot included, stays under fifteen minutes: the video check decodes
/// four clips with each decoder three ways.
const MEDIA_BUDGET: Duration = Duration::from_secs(900);
/// The root check boots three times (as it starts, then with root elevation on and off), so
/// it stays under twenty-one minutes.
const ROOT_BUDGET: Duration = Duration::from_secs(1260);
/// How long past its budget a check may run before it stops everything it started and ends:
/// inside its job step's limit (18 minutes for media, 23 for root), so the step never kills
/// it with its VM still running.
const OVERRUN: Duration = Duration::from_secs(60);
/// How long one adb command may take. The probe's longest check, the video check, encodes
/// four clips and decodes each with every decoder to buffers, to a surface and as a feed; a
/// log dump of the whole boot takes seconds.
const ADB_LIMIT: Duration = Duration::from_secs(420);

fn scratch(kind: &str) -> PathBuf {
    // Short: the service's socket goes inside, and a socket's path has at most 107 bytes.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest
        .ancestors()
        .nth(4)
        .unwrap()
        .join(".scratch")
        .join(format!("{kind}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Fails the test with what the service printed and the guest log's last lines.
fn fail(why: String, service: &Service, log: &Mutex<Vec<String>>) -> ! {
    // The lock is let go before the panic, so the thread that reads the guest log is not
    // left with a poisoned lock and does not panic on top of this failure.
    let tail = {
        let log = lock(log);
        let _ = std::fs::write(service.log.with_file_name("guest.log"), log.join("\n"));
        log[log.len().saturating_sub(200)..].join("\n")
    };
    let devices =
        std::fs::read_to_string(service.log.with_file_name("devices.log")).unwrap_or_default();
    panic!(
        "{why}\n--- the service's output ---\n{}\n--- the devices' output ---\n{devices}\n--- the guest log's last lines ---\n{tail}",
        service.log(),
    )
}

fn client(socket: &Path, wait: Duration) -> HostApiClient {
    let mut connector = UnixLocalConnector {
        path: socket.to_path_buf(),
    };
    let deadline = Instant::now() + wait;
    loop {
        match HostApiClient::connect(&mut connector, "media-test", Duration::from_secs(30)) {
            Ok(client) => return client,
            Err(why) if Instant::now() >= deadline => {
                panic!("the service did not answer on {}: {why}", socket.display())
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

/// adb, talking to the guest through the service's developer mode port.
struct Adb {
    program: PathBuf,
    serial: String,
}

impl Adb {
    /// Runs adb with `args` against the guest, for [ADB_LIMIT] at most: a command that
    /// hangs fails the check with what it was, instead of holding it until the job's timeout.
    fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let out = run_bounded(
            Command::new(&self.program)
                .arg("-s")
                .arg(&self.serial)
                .args(args),
            ADB_LIMIT,
            usize::MAX,
        )?;
        match out.status {
            Some(status) if status.success() => Ok(out.stdout),
            Some(_) => Err(format!(
                "adb {} failed: {}{}",
                args.join(" "),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )),
            None => Err(format!(
                "adb {} did not finish within {} s and was stopped",
                args.join(" "),
                ADB_LIMIT.as_secs()
            )),
        }
    }

    fn shell(&self, command: &str) -> Result<String, String> {
        self.run(&["shell", command])
            .map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    /// Connects, and waits until the guest's shell answers.
    fn connect(&self, wait: Duration) -> Result<(), String> {
        let deadline = Instant::now() + wait;
        let _ = run_bounded(
            Command::new(&self.program).arg("start-server"),
            ADB_LIMIT,
            usize::MAX,
        );
        loop {
            let _ = run_bounded(
                Command::new(&self.program).arg("connect").arg(&self.serial),
                ADB_LIMIT,
                usize::MAX,
            );
            match self.shell("echo ready") {
                Ok(out) if out.trim() == "ready" => return Ok(()),
                Ok(out) if Instant::now() >= deadline => {
                    return Err(format!("the guest's shell answered {out:?}"))
                }
                Err(why) if Instant::now() >= deadline => return Err(why),
                _ => std::thread::sleep(Duration::from_secs(2)),
            }
        }
    }

    /// Runs one of the probe's checks and returns its results, keeping its output in `dir`.
    fn probe(&self, dir: &Path, check: &str) -> Result<BTreeMap<String, String>, String> {
        let out = self.shell(&format!(
            "am instrument -w -r -e check {check} {PROBE_RUNNER}"
        ))?;
        let _ = std::fs::write(dir.join(format!("probe-{check}.txt")), &out);
        instrumentation_results(&out)
    }
}

/// The results an instrumentation finished with, from `am instrument -r`'s output: each
/// `INSTRUMENTATION_RESULT: key=value` line. An instrumentation that did not finish (its
/// process died, or it never started) has no `INSTRUMENTATION_CODE: -1` line.
fn instrumentation_results(out: &str) -> Result<BTreeMap<String, String>, String> {
    let mut results = BTreeMap::new();
    let mut finished = false;
    for line in out.lines() {
        let line = line.trim_end();
        if let Some(pair) = line.strip_prefix("INSTRUMENTATION_RESULT: ") {
            if let Some((key, value)) = pair.split_once('=') {
                results.insert(key.to_string(), value.to_string());
            }
        } else if line == "INSTRUMENTATION_CODE: -1" {
            finished = true;
        }
    }
    if !finished {
        return Err(format!("the probe did not finish:\n{out}"));
    }
    if let Some(error) = results.get("error") {
        return Err(format!("the probe failed: {error}"));
    }
    Ok(results)
}

/// The lines of `log` that record an SELinux denial naming one of `types`.
fn denials<'a>(log: &'a str, types: &[&str]) -> Vec<&'a str> {
    log.lines()
        .filter(|line| line.contains("avc:") && line.contains("denied"))
        .filter(|line| types.iter().any(|kind| line.contains(&format!(":{kind}:"))))
        .collect()
}

/// A number the probe reported, or why it is not there.
fn number(results: &BTreeMap<String, String>, key: &str) -> Result<i64, String> {
    let value = results.get(key).ok_or_else(|| {
        let error = results
            .get(
                &key.rsplit_once('.')
                    .map(|(head, _)| format!("{head}.error"))
                    .unwrap_or_default(),
            )
            .cloned()
            .unwrap_or_else(|| "no error was reported".into());
        format!("the probe reported no {key} ({error})")
    })?;
    value
        .parse()
        .map_err(|_| format!("the probe reported {key}={value}, which is not a number"))
}

fn root_absent_problems(results: &BTreeMap<String, String>) -> Vec<String> {
    let mut problems = Vec::new();
    for path in ["/system/bin/su", "/system/xbin/su", "/sbin/su"] {
        for operation in ["stat", "procstat", "access", "exec"] {
            let key = format!("root.{path}.{operation}");
            if results.get(&key).map(String::as_str) != Some("ENOENT") {
                problems.push(format!(
                    "{key} did not report ENOENT: {:?}",
                    results.get(&key)
                ));
            }
        }
    }
    if results.get("root.path_lookup_exit").map(String::as_str) != Some("1") {
        problems.push(format!(
            "PATH found su: {:?}",
            results.get("root.path_lookup_exit")
        ));
    }
    problems
}

fn restart_for_root(
    api: &mut HostApiClient,
    adb: &Adb,
    dir: &Path,
    enabled: bool,
) -> Result<(), String> {
    api.stop().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if api.status().map_err(|e| e.to_string())?.state == SessionState::Stopped {
            break;
        }
        if Instant::now() >= deadline {
            return Err("Android did not stop".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let output = Command::new("sudo")
        .arg("env")
        .arg(format!(
            "DSA_ROOT_TEST_SOCKET={}",
            dir.join("host.sock").display()
        ))
        .arg(format!("DSA_ROOT_TEST_ENABLED={}", u8::from(enabled)))
        .arg(executable)
        .args(["--exact", "fr35_set_machine_switch_as_admin", "--nocapture"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "setting root elevation: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if enabled {
        let result = api
            .set_app_root(PROBE_PACKAGE, "ask")
            .map_err(|e| e.to_string())?;
        if result.level != "ask" {
            return Err(format!(
                "setting the probe's root level: {}",
                result.message
            ));
        }
    }
    api.ensure_running(Duration::from_secs(360), Duration::from_secs(2), |_| {})
        .map_err(|e| e.to_string())?;
    adb.connect(Duration::from_secs(120))
}

#[test]
fn fr35_set_machine_switch_as_admin() {
    let Some(socket) = std::env::var_os("DSA_ROOT_TEST_SOCKET") else {
        return;
    };
    let enabled = std::env::var("DSA_ROOT_TEST_ENABLED").unwrap() == "1";
    let mut api = client(Path::new(&socket), Duration::from_secs(10));
    let mut settings = api.settings().unwrap().settings;
    settings.root_elevation = enabled;
    api.set_settings(&settings).unwrap();
}

fn root_approval_check(api: &mut HostApiClient, adb: &Adb, dir: &Path) -> Result<(), String> {
    let approver = dir.join("approver");
    std::fs::write(approver.join("answer"), "deny").map_err(|e| e.to_string())?;
    restart_for_root(api, adb, dir, true)?;
    let visible_until = Instant::now() + Duration::from_secs(90);
    loop {
        let results = adb.probe(dir, "root_visible")?;
        if results.get("root.visible").map(String::as_str) == Some("yes") {
            break;
        }
        if Instant::now() >= visible_until {
            return Err(format!("the allowed app cannot see su: {results:?}"));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    let requests = approver.join("requests.jsonl");
    let count = || -> Result<usize, String> {
        std::fs::read_to_string(&requests)
            .map(|text| text.lines().count())
            .map_err(|e| e.to_string())
    };
    let denied = adb.probe(dir, "root_request")?;
    if number(&denied, "root.exit")? == 0 || count()? != 1 {
        return Err(format!("a refused root call did not fail once: {denied:?}"));
    }
    let records = std::fs::read_to_string(&requests).map_err(|e| e.to_string())?;
    let first: serde_json::Value =
        serde_json::from_str(records.lines().next().unwrap_or("")).map_err(|e| e.to_string())?;
    if first["package"] != PROBE_PACKAGE
        || first["app_name"] != "DSA media probe"
        || first["command"][0] != "id"
        || first["uid"].as_u64().unwrap_or(0) < 10_000
        || first["icon_bytes"].as_u64().unwrap_or(0) == 0
    {
        return Err(format!(
            "the approval prompt did not identify the calling app and command: {first}"
        ));
    }
    std::fs::remove_file(approver.join("answer")).map_err(|e| e.to_string())?;
    let waited = Instant::now();
    let unanswered = adb.probe(dir, "root_request")?;
    if number(&unanswered, "root.exit")? == 0
        || count()? != 2
        || waited.elapsed() < Duration::from_secs(55)
    {
        return Err(format!(
            "an unanswered root call did not time out: {unanswered:?}"
        ));
    }
    // Let the host's own deadline finish before changing the shared answer file.
    std::thread::sleep(Duration::from_secs(3));
    std::fs::write(approver.join("answer"), "approve").map_err(|e| e.to_string())?;
    let approved = adb.probe(dir, "root_request")?;
    if number(&approved, "root.exit")? != 0
        || !approved
            .get("root.output")
            .is_some_and(|out| out.contains("uid=0"))
        || count()? != 3
    {
        return Err(format!(
            "the approved command did not run as root: {approved:?}"
        ));
    }
    let grace = adb.probe(dir, "root_request")?;
    if number(&grace, "root.exit")? != 0 || count()? != 3 {
        return Err(format!(
            "the app's second root call did not use its grace: {grace:?}"
        ));
    }
    restart_for_root(api, adb, dir, false)?;
    let off = adb.probe(dir, "root_absent")?;
    let problems = root_absent_problems(&off);
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let helper = adb.shell("getprop init.svc.dsa_su")?;
    if !matches!(helper.trim(), "" | "stopped") {
        return Err(format!(
            "the helper still runs after root elevation is off: {helper}"
        ));
    }
    Ok(())
}

#[test]
fn fr35_the_boot_check_rejects_a_visible_su() {
    let mut results = BTreeMap::new();
    for path in ["/system/bin/su", "/system/xbin/su", "/sbin/su"] {
        for operation in ["stat", "procstat", "access", "exec"] {
            results.insert(format!("root.{path}.{operation}"), "ENOENT".into());
        }
    }
    results.insert("root.path_lookup_exit".into(), "1".into());
    assert!(root_absent_problems(&results).is_empty());
    results.insert("root./system/bin/su.stat".into(), "visible".into());
    assert_eq!(root_absent_problems(&results).len(), 1);
}

/// What is wrong with the probe's audio results: each track must initialise, take the whole
/// tone and play at least half of it.
fn audio_problems(results: &BTreeMap<String, String>) -> Vec<String> {
    let mut problems = Vec::new();
    for track in ["stream", "static"] {
        if let Some(error) = results.get(&format!("audio.{track}.error")) {
            problems.push(format!("the {track} track failed: {error}"));
            continue;
        }
        match number(results, &format!("audio.{track}.state")) {
            Ok(TRACK_INITIALIZED) => {}
            Ok(state) => problems.push(format!(
                "the {track} track's state is {state}, not initialised"
            )),
            Err(why) => problems.push(why),
        }
        match number(results, &format!("audio.{track}.written")) {
            Ok(TONE_FRAMES) => {}
            Ok(written) => problems.push(format!(
                "the {track} track took {written} of the tone's {TONE_FRAMES} frames"
            )),
            Err(why) => problems.push(why),
        }
        match number(results, &format!("audio.{track}.played")) {
            Ok(played) if played >= TONE_FRAMES / 2 => {}
            Ok(played) => problems.push(format!(
                "the {track} track played {played} frames in three seconds, not half the tone"
            )),
            Err(why) => problems.push(why),
        }
    }
    problems
}

/// What is wrong with the probe's decode results: each stream must be encoded, the guest's
/// software decoder must be among the decoders, and every decoder must start and give back at
/// least half of what went in (pictures for H.264, sample frames for AAC).
fn decode_problems(results: &BTreeMap<String, String>) -> Vec<String> {
    let mut problems = Vec::new();
    for (stream, software) in SOFTWARE_DECODERS {
        if let Some(error) = results.get(&format!("encode.{stream}.error")) {
            problems.push(format!("the {stream} encoder failed: {error}"));
            continue;
        }
        let wanted = if *stream == "avc" {
            match number(results, "encode.avc.units") {
                Ok(units) if units > 0 => (units + 1) / 2,
                Ok(_) => {
                    problems.push("the H.264 encoder gave no pictures".into());
                    continue;
                }
                Err(why) => {
                    problems.push(why);
                    continue;
                }
            }
        } else {
            TONE_FRAMES / 2
        };
        let Some(decoders) = results.get(&format!("decode.{stream}.decoders")) else {
            problems.push(format!("the probe decoded no {stream} stream"));
            continue;
        };
        let decoders: Vec<&str> = decoders
            .split(',')
            .filter(|name| !name.is_empty())
            .collect();
        if !decoders.contains(software) {
            problems.push(format!(
                "the guest lists no {software} among its decoders ({decoders:?})"
            ));
        }
        for decoder in decoders {
            if let Some(error) = results.get(&format!("decode.{decoder}.error")) {
                problems.push(format!("{decoder} failed: {error}"));
                continue;
            }
            match number(results, &format!("decode.{decoder}.out")) {
                Ok(out) if out >= wanted => {}
                Ok(out) => problems.push(format!(
                    "{decoder} gave back {out} of the {wanted} it should have"
                )),
                Err(why) => problems.push(why),
            }
        }
    }
    problems
}

/// The bars the probe's video clips show, left to right, as Y, Cb, Cr: grey, red, yellow,
/// blue. The probe (`VideoCheck.BARS`) encodes the same ones.
const BARS: [[u8; 3]; 4] = [
    [180, 128, 128],
    [81, 90, 240],
    [210, 16, 146],
    [41, 240, 110],
];

/// The probe's video clips, and the guest's own decoder for each format.
const CLIPS: &[(&str, &str)] = &[
    ("avc_426x240", "c2.android.avc.decoder"),
    ("avc_360x640", "c2.android.avc.decoder"),
    ("vp9_426x240", "c2.android.vp9.decoder"),
    ("vp9_360x640", "c2.android.vp9.decoder"),
];

/// The host decoder each clip's format has here: the boot check's own (see
/// [`PatternDecoders`]), which the guest lists as `c2.dsa.<format>.decoder`.
fn host_decoder(clip: &str) -> &'static str {
    if clip.starts_with("vp9") {
        "c2.dsa.vp9.decoder"
    } else {
        "c2.dsa.avc.decoder"
    }
}

/// The boot check's host decoder: CI's host has no GPU decoder, so this stands in for one on
/// the video port. Whatever a packet holds, its picture is the probe's colour bars at the
/// size the guest opened the stream with, put in a surface padded the way GPU decoders pad
/// theirs (the coded size rounded up to 16, the surface taller still, its padding zero and
/// its chroma plane after the surface's rows) and copied out with the rules every host's
/// decoder uses. So the check covers the whole path a host decoder's picture takes: the plane
/// offset and visible size on the host, the video port, and the guest's buffer, crop and
/// surface.
struct PatternDecoders;

struct PatternDecoder {
    surface: Vec<u8>,
    pitch: usize,
    chroma_row: usize,
    width: u32,
    height: u32,
}

/// The bars at `width` by `height` in a padded NV12 surface: its pitch and the row its chroma
/// plane starts at.
fn pattern_surface(width: u32, height: u32) -> (Vec<u8>, usize, usize) {
    let (w, h) = (width as usize, height as usize);
    let coded = (w.div_ceil(16) * 16, h.div_ceil(16) * 16);
    let pitch = coded.0.div_ceil(64) * 64 + 64;
    // A driver's surface for the largest picture it expects: taller than the coded size.
    let rows = coded.1.div_ceil(64) * 64 + 64;
    let chroma_row = dsa_host_core::video_layout::chroma_start_row(Some(rows), coded.1, h);
    let mut surface = vec![0u8; pitch * (rows + rows.div_ceil(2))];
    let bar = |x: usize| (x * 4 / w).min(3);
    for y in 0..h {
        for x in 0..w {
            surface[y * pitch + x] = BARS[bar(x)][0];
        }
    }
    for y in 0..h.div_ceil(2) {
        for x in 0..w.div_ceil(2) {
            let at = (chroma_row + y) * pitch + 2 * x;
            surface[at] = BARS[bar(2 * x)][1];
            surface[at + 1] = BARS[bar(2 * x)][2];
        }
    }
    (surface, pitch, chroma_row)
}

impl VideoDecoders for PatternDecoders {
    fn capabilities(&self) -> Vec<CodecSupport> {
        [Codec::H264, Codec::Vp9]
            .into_iter()
            .map(|codec| CodecSupport {
                codec,
                max_width: 4096,
                max_height: 2304,
            })
            .collect()
    }

    fn open(&self, _: Codec, width: u32, height: u32) -> Result<Box<dyn VideoDecoder>, String> {
        if width == 0 || height == 0 {
            return Err(format!("the guest opened a stream of {width}x{height}"));
        }
        let (surface, pitch, chroma_row) = pattern_surface(width, height);
        Ok(Box::new(PatternDecoder {
            surface,
            pitch,
            chroma_row,
            width,
            height,
        }))
    }
}

impl VideoDecoder for PatternDecoder {
    fn decode(&mut self, packet: Packet) -> Result<Vec<Picture>, String> {
        if packet.codec_config {
            return Ok(Vec::new());
        }
        dsa_host_core::video_layout::picture_from_surface(
            packet.id,
            &self.surface,
            SurfaceFormat::Nv12,
            self.pitch,
            self.chroma_row,
            self.width,
            self.height,
        )
        .map(|picture| vec![picture])
        .ok_or_else(|| "the pattern does not fit its surface".into())
    }

    fn drain(&mut self) -> Result<Vec<Picture>, String> {
        Ok(Vec::new())
    }

    fn flush(&mut self) {}
}

/// Serves the video port with [`PatternDecoders`] on the socket the service relays the guest's
/// video port to, until `stop`. Its log lines go to `log`.
fn serve_pattern_video(
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Vec<String>>>,
) -> Result<std::thread::JoinHandle<()>, String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .ok_or("XDG_RUNTIME_DIR names no runtime folder for the video port's socket")?;
    let path = device_socket(&runtime, VIDEO_PORT);
    let listener = DeviceSocketListener::bind(&path)
        .map_err(|e| format!("{} cannot be opened: {e}", path.display()))?;
    let decoders: Arc<dyn VideoDecoders> = Arc::new(PatternDecoders);
    let line: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |text: &str| {
        lock(&log).push(text.to_string());
    });
    std::thread::Builder::new()
        .name("pattern-video".into())
        .spawn(move || {
            dsa_host_core::video::serve(
                Box::new(listener),
                decoders,
                "media-test pattern-video".into(),
                stop,
                line,
            )
        })
        .map_err(|e| format!("the video port's thread did not start: {e}"))
}

/// The guest's codec HAL logging that a picture had no buffer or did not fit one.
fn codec_buffer_errors(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|line| line.contains("dsa-codec2"))
        .filter(|line| {
            line.contains("gave no")
                || line.contains("did not fit")
                || line.contains("cannot write")
        })
        .collect()
}

/// What is wrong with the probe's video results (see `VideoCheck`): each clip must be encoded
/// and decoded by the guest's own decoder and the host's; every decoder's pictures must keep
/// the bars' colours to the edges, in buffers and on screen; pictures must reach both
/// surfaces of a player that moves between them, while its sound plays; and a feed's second
/// clip must show its first picture within seconds.
fn video_problems(results: &BTreeMap<String, String>) -> Vec<String> {
    let mut problems = Vec::new();
    for (clip, software) in CLIPS {
        let key = format!("video.{clip}");
        if let Some(error) = results.get(&format!("{key}.encode_error")) {
            problems.push(format!("{clip}: the encoder failed: {error}"));
            continue;
        }
        let units = match number(results, &format!("{key}.units")) {
            Ok(units) if units > 0 => units,
            Ok(_) => {
                problems.push(format!("{clip}: the encoder gave no pictures"));
                continue;
            }
            Err(why) => {
                problems.push(why);
                continue;
            }
        };
        let Some(decoders) = results.get(&format!("{key}.decoders")) else {
            problems.push(format!("{clip}: the probe decoded nothing"));
            continue;
        };
        let decoders: Vec<&str> = decoders.split(',').filter(|n| !n.is_empty()).collect();
        for wanted in [*software, host_decoder(clip)] {
            if !decoders.contains(&wanted) {
                problems.push(format!(
                    "{clip}: the guest lists no {wanted} among its decoders ({decoders:?})"
                ));
            }
        }
        for decoder in decoders {
            let at = format!("{key}.{decoder}");
            let mut check = |what: &str, found: Result<(), String>| {
                if let Err(why) = found {
                    problems.push(format!("{clip}, {decoder}, {what}: {why}"));
                }
            };
            for way in ["buffer", "surface", "shorts"] {
                if let Some(error) = results.get(&format!("{at}.{way}.error")) {
                    check(way, Err(format!("failed: {error}")));
                    continue;
                }
                let colour = results
                    .get(&format!("{at}.{way}.colour"))
                    .map(String::as_str);
                if colour != Some("ok") {
                    check(
                        way,
                        Err(format!("colours: {}", colour.unwrap_or("not reported"))),
                    );
                }
            }
            let at_least = |key: &str, wanted: i64| -> Result<(), String> {
                match number(results, key) {
                    Ok(n) if n >= wanted => Ok(()),
                    Ok(n) => Err(format!("{key} is {n}, below {wanted}")),
                    Err(why) => Err(why),
                }
            };
            if !results.contains_key(&format!("{at}.buffer.error")) {
                check(
                    "buffer",
                    at_least(&format!("{at}.buffer.out"), (units + 1) / 2),
                );
            }
            if !results.contains_key(&format!("{at}.surface.error")) {
                check(
                    "surface",
                    at_least(&format!("{at}.surface.rendered"), (units + 1) / 2),
                );
                // Pictures reached the screen on the first surface and kept coming on the
                // second: a decoder whose surface gives it no buffer shows none.
                check("surface", at_least(&format!("{at}.surface.presented"), 2));
                check("surface", at_least(&format!("{at}.surface.after_swap"), 1));
                check(
                    "surface",
                    at_least(&format!("{at}.surface.audio"), TONE_FRAMES / 4),
                );
            }
            if !results.contains_key(&format!("{at}.shorts.error")) {
                check("shorts", at_least(&format!("{at}.shorts.first"), 1));
                check("shorts", at_least(&format!("{at}.shorts.second"), 1));
                match number(results, &format!("{at}.shorts.first_frame_ms")) {
                    Ok(ms) if (0..=3000).contains(&ms) => {}
                    Ok(ms) => check(
                        "shorts",
                        Err(format!("the second clip's first picture took {ms} ms")),
                    ),
                    Err(why) => check("shorts", Err(why)),
                }
            }
        }
    }
    problems
}

#[test]
fn fr26_the_pattern_decoders_pictures_are_the_bars_out_of_a_padded_surface() {
    for (width, height) in [(426u32, 240u32), (360, 640), (425, 239)] {
        let mut decoder = PatternDecoders.open(Codec::H264, width, height).unwrap();
        let pictures = decoder
            .decode(Packet {
                id: 7,
                data: vec![0, 0, 0, 1, 0x65],
                codec_config: false,
                timestamp_us: 0,
            })
            .unwrap();
        assert_eq!(pictures.len(), 1);
        let picture = &pictures[0];
        assert_eq!(
            (picture.id, picture.width, picture.height),
            (7, width, height)
        );
        let stride = picture.stride as usize;
        let (w, h) = (width as usize, height as usize);
        // The right edge and the bottom row hold the last bar, chroma included: nothing of
        // the zero padding comes along.
        assert_eq!(picture.nv12[(h - 1) * stride + w - 1], BARS[3][0]);
        let chroma = &picture.nv12[stride * h..];
        let last = (h.div_ceil(2) - 1) * stride + 2 * (w.div_ceil(2) - 1);
        assert_eq!(&chroma[last..last + 2], &BARS[3][1..]);
        assert!(chroma.chunks(2).all(|pair| pair != [0, 0]), "chroma lost");
    }
    let mut decoder = PatternDecoders.open(Codec::Vp9, 426, 240).unwrap();
    let config = Packet {
        id: 1,
        data: vec![1],
        codec_config: true,
        timestamp_us: 0,
    };
    assert!(decoder.decode(config).unwrap().is_empty());
}

fn video_results() -> BTreeMap<String, String> {
    let mut results = BTreeMap::new();
    for (clip, software) in CLIPS {
        let key = format!("video.{clip}");
        results.insert(format!("{key}.units"), "30".into());
        results.insert(
            format!("{key}.decoders"),
            format!("{},{software}", host_decoder(clip)),
        );
        for decoder in [host_decoder(clip), *software] {
            let at = format!("{key}.{decoder}");
            for (k, v) in [
                ("buffer.out", "30"),
                ("buffer.colour", "ok"),
                ("surface.rendered", "30"),
                ("surface.presented", "30"),
                ("surface.after_swap", "15"),
                ("surface.colour", "ok"),
                ("surface.audio", "48000"),
                ("shorts.first", "30"),
                ("shorts.second", "30"),
                ("shorts.first_frame_ms", "40"),
                ("shorts.colour", "ok"),
            ] {
                results.insert(format!("{at}.{k}"), v.to_string());
            }
        }
    }
    results
}

#[test]
fn fr26_green_pictures_and_a_stalled_surface_fail_the_video_check() {
    let good = video_results();
    assert!(
        video_problems(&good).is_empty(),
        "{:?}",
        video_problems(&good)
    );
    let at = "video.vp9_426x240.c2.dsa.vp9.decoder";

    // A picture whose chroma was lost.
    let mut green = good.clone();
    green.insert(
        format!("{at}.buffer.colour"),
        "no chroma (green) at 425,120: Y41 Cb0 Cr0".into(),
    );
    let problems = video_problems(&green);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("green"));

    // What YouTube showed: the surface gave the decoder no buffer, so nothing reached the
    // screen, and a feed never started its second clip.
    let mut stalled = good.clone();
    stalled.insert(format!("{at}.surface.presented"), "0".into());
    stalled.insert(format!("{at}.surface.after_swap"), "0".into());
    stalled.insert(format!("{at}.shorts.second"), "0".into());
    stalled.insert(format!("{at}.shorts.first_frame_ms"), "-1".into());
    assert_eq!(
        video_problems(&stalled).len(),
        4,
        "{:?}",
        video_problems(&stalled)
    );

    // The sound stopped while the video played.
    let mut silent = good.clone();
    silent.insert(format!("{at}.surface.audio"), "0".into());
    assert_eq!(video_problems(&silent).len(), 1);

    // A host whose decoder the guest does not list.
    let mut unlisted = good.clone();
    unlisted.insert(
        "video.avc_360x640.decoders".into(),
        "c2.android.avc.decoder".into(),
    );
    assert_eq!(video_problems(&unlisted).len(), 1);

    // The error the HAL logged on every picture.
    let log = "11-10 05:00:00.000  612  640 E dsa-codec2: c2.dsa.vp9.decoder: the surface gave no 426x240 buffer (1 in a row): Status(-8, EX_SERVICE_SPECIFIC): '14: '\n\
               11-10 05:00:00.010  612  640 I dsa-codec2: c2.dsa.vp9.decoder: decoding 426x240 on the host's GPU\n";
    assert_eq!(codec_buffer_errors(log).len(), 1);
}

#[test]
fn pr4_the_probes_results_are_read_from_the_instrumentation_output() {
    let out = "INSTRUMENTATION_RESULT: audio.stream.state=1\r\n\
               INSTRUMENTATION_RESULT: audio.stream.written=48000\r\n\
               INSTRUMENTATION_CODE: -1\r\n";
    let results = instrumentation_results(out).unwrap();
    assert_eq!(results["audio.stream.state"], "1");
    assert_eq!(results["audio.stream.written"], "48000");

    // A probe whose process died never reports its code.
    let died = "INSTRUMENTATION_RESULT: shortMsg=Process crashed.\nINSTRUMENTATION_CODE: 0\n";
    assert!(instrumentation_results(died).is_err());
    // An error the probe caught fails the check.
    let caught =
        "INSTRUMENTATION_RESULT: error=java.lang.IllegalStateException\nINSTRUMENTATION_CODE: -1\n";
    assert!(instrumentation_results(caught).is_err());
}

#[test]
fn pr4_a_track_that_could_not_get_its_control_block_is_a_failure() {
    let good: BTreeMap<String, String> = [
        ("audio.stream.state", "1"),
        ("audio.stream.written", "48000"),
        ("audio.stream.played", "47000"),
        ("audio.static.state", "1"),
        ("audio.static.written", "48000"),
        ("audio.static.played", "48000"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert!(
        audio_problems(&good).is_empty(),
        "{:?}",
        audio_problems(&good)
    );

    // What the image's apps saw: the builder threw, so there is no state.
    let mut refused = good.clone();
    refused.remove("audio.stream.state");
    refused.remove("audio.stream.written");
    refused.remove("audio.stream.played");
    refused.insert(
        "audio.stream.error".into(),
        "java.lang.UnsupportedOperationException: Cannot create AudioTrack".into(),
    );
    let problems = audio_problems(&refused);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("Cannot create AudioTrack"));

    let mut silent = good;
    silent.insert("audio.static.played".into(), "0".into());
    assert_eq!(audio_problems(&silent).len(), 1);
}

#[test]
fn pr4_a_decoder_that_could_not_start_is_a_failure() {
    let good: BTreeMap<String, String> = [
        ("encode.avc.units", "30"),
        (
            "decode.avc.decoders",
            "c2.dsa.avc.decoder,c2.android.avc.decoder",
        ),
        ("decode.c2.dsa.avc.decoder.out", "30"),
        ("decode.c2.android.avc.decoder.out", "30"),
        ("encode.aac.units", "48"),
        ("decode.aac.decoders", "c2.android.aac.decoder"),
        ("decode.c2.android.aac.decoder.out", "47104"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert!(
        decode_problems(&good).is_empty(),
        "{:?}",
        decode_problems(&good)
    );

    // What every app saw: start() failed with NO_MEMORY, the software decoders' too.
    let mut refused = good.clone();
    refused.remove("decode.c2.android.avc.decoder.out");
    refused.insert(
        "decode.c2.android.avc.decoder.error".into(),
        "android.media.MediaCodec$CodecException: Error 0xfffffff4".into(),
    );
    let problems = decode_problems(&refused);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].starts_with("c2.android.avc.decoder failed"));

    // A decoder that started but gave nothing back.
    let mut empty = good.clone();
    empty.insert("decode.c2.dsa.avc.decoder.out".into(), "0".into());
    assert_eq!(decode_problems(&empty).len(), 1);

    // An encoder that failed leaves nothing to decode.
    let mut no_encoder = good;
    no_encoder.insert(
        "encode.aac.error".into(),
        "java.io.IOException: no encoder".into(),
    );
    assert_eq!(decode_problems(&no_encoder).len(), 1);
}

#[test]
fn pr4_a_denial_of_a_media_services_shared_memory_is_found_in_the_log() {
    let log = "11-08 12:00:01.554  4650  4650 I auditd  : type=1400 audit(0.0:30): avc:  denied  { read write } for  comm=\"binder:772_3\" path=\"/memfd:AudioFlinger::Client (deleted)\" dev=\"tmpfs\" ino=120 scontext=u:r:untrusted_app:s0:c149,c256,c512,c768 tcontext=u:object_r:audioserver_tmpfs:s0 tclass=file permissive=0\n\
               11-08 12:00:02.000  4650  4650 I auditd  : type=1400 audit(0.0:31): avc:  denied  { read } for  comm=\"uiautomator\" scontext=u:r:shell:s0 tcontext=u:object_r:qemu_sf_lcd_density_prop:s0 tclass=file permissive=0\n\
               11-08 12:00:03.000  4650  4650 I AudioTrack: start\n";
    let found = denials(log, SHARED_MEMORY_TYPES);
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("audioserver_tmpfs"));
}

#[test]
fn fr26_the_sensors_hal_refused_its_event_queue_is_found_in_the_log() {
    // What every boot logged before the sensors HAL could use system_server's memfds.
    let log = "11-08 11:53:18.987   772   772 I auditd  : type=1400 audit(0.0:8): avc:  denied  { read write } for  comm=\"system-server-i\" path=2F6D656D66643A4D6573736167655175657565202864656C6574656429 dev=\"tmpfs\" ino=2073 scontext=u:r:hal_sensors_default:s0 tcontext=u:object_r:system_server_tmpfs:s0 tclass=file permissive=0\n";
    assert_eq!(denials(log, SHARED_MEMORY_TYPES).len(), 1);
}

#[test]
fn pr4_an_app_plays_sound_and_decodes_h264_and_aac_with_selinux_enforcing() {
    boot_check(true, false);
}

#[test]
fn fr35_an_app_needs_approval_and_su_is_hidden_when_disabled() {
    boot_check(false, true);
}

fn boot_check(check_media: bool, check_root: bool) {
    let (Some(crosvm), Some(image), Some(probe)) = (
        std::env::var_os("DSA_CROSVM").map(PathBuf::from),
        std::env::var_os("DSA_WIDGET_TEST_IMAGE").map(PathBuf::from),
        std::env::var_os("DSA_MEDIA_PROBE").map(PathBuf::from),
    ) else {
        eprintln!(
            "skipped: DSA_CROSVM, DSA_WIDGET_TEST_IMAGE and DSA_MEDIA_PROBE name no crosvm, image and probe"
        );
        return;
    };
    let kernel = std::env::var_os("DSA_WIDGET_TEST_KERNEL").map(PathBuf::from);
    let adb_program = std::env::var_os("DSA_ADB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("adb"));
    let started = Instant::now();
    let (budget, what) = if check_root {
        (ROOT_BUDGET, "the root check")
    } else {
        (MEDIA_BUDGET, "the media check")
    };
    let deadline = started + budget;
    let _watchdog = Watchdog::start(what, budget + OVERRUN);
    let dir = scratch(if check_root { "root" } else { "media" });
    let approver = dir.join("approver");
    std::fs::create_dir_all(&approver).unwrap();
    std::fs::set_permissions(&approver, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("host.sock");
    let config = dir.join("dsa-hostd.json");
    let settings = serde_json::json!({
        "image_dir": image,
        "kernel": kernel,
        "crosvm": crosvm,
        "socket": socket,
        "machine_dir": dir.join("machine"),
        "network": false,
        "memory_mb": 4096,
        "processors": 2,
        "data_disk_size_gb": 8,
        "test_root_approver": approver,
    });
    std::fs::write(&config, settings.to_string()).unwrap();
    // The host decoder the guest's codec store finds when it starts, so it must be up before
    // the guest boots.
    let video_stop = Arc::new(AtomicBool::new(false));
    let video_log: Arc<Mutex<Vec<String>>> = Arc::default();
    let pattern_video = if check_media {
        Some(serve_pattern_video(
            Arc::clone(&video_stop),
            Arc::clone(&video_log),
        ))
    } else {
        None
    };
    let service = Service::spawn(
        Command::new(env!("CARGO_BIN_EXE_dsa-linux-hostd"))
            .arg("--config")
            .arg(&config),
        dir.join("hostd.log"),
    );
    let _compositor = Service::spawn(
        Command::new(env!("CARGO_BIN_EXE_dsa-compositor")).env("DSA_SOCKET", &socket),
        dir.join("compositor.log"),
    );
    // The guest's sound goes to the host's audio port, which dsa-devices serves.
    let _devices = Service::spawn(
        Command::new(env!("CARGO_BIN_EXE_dsa-devices"))
            .arg("--socket")
            .arg(&socket),
        dir.join("devices.log"),
    );
    let mut api = client(&socket, Duration::from_secs(30));
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    {
        let reader = client(&socket, Duration::from_secs(10))
            .watch_log(&LogFilter::default(), false)
            .expect("reading the guest log");
        let log = Arc::clone(&log);
        std::thread::spawn(move || {
            let mut reader = reader;
            loop {
                match reader.next(Duration::from_secs(5)) {
                    Ok(Some(batch)) => {
                        let mut log = lock(&log);
                        for entry in batch.entries {
                            log.push(entry.threadtime());
                        }
                    }
                    Ok(None) => {}
                    Err(_) => return,
                }
            }
        });
    }
    let boot = deadline.saturating_duration_since(Instant::now()) - Duration::from_secs(240);
    if let Err(why) = api.ensure_running(boot, Duration::from_secs(2), |state| {
        eprintln!("{} s: Android is {state:?}", started.elapsed().as_secs());
    }) {
        fail(
            format!("Android did not become ready: {why}"),
            &service,
            &log,
        );
    }
    eprintln!(
        "Android was ready {} s after the start",
        started.elapsed().as_secs()
    );

    // Developer mode, so adb reaches the guest through the service's loopback port.
    let mut wanted = match api.settings() {
        Ok(report) => report.settings,
        Err(why) => fail(format!("reading the settings: {why}"), &service, &log),
    };
    wanted.developer_mode = true;
    wanted.adb_auto_connect = false;
    let port = wanted.adb_port;
    if let Err(why) = api.set_settings(&wanted) {
        fail(format!("turning developer mode on: {why}"), &service, &log);
    }
    let adb = Adb {
        program: adb_program,
        serial: format!("127.0.0.1:{port}"),
    };
    if let Err(why) = adb.connect(Duration::from_secs(120)) {
        fail(
            format!("adb did not reach the guest: {why}"),
            &service,
            &log,
        );
    }

    // The policy is what is under test, so it must be in force.
    match adb.shell("getenforce") {
        Ok(mode) if mode.trim() == "Enforcing" => {}
        Ok(mode) => fail(
            format!("SELinux is {:?} on the image, not enforcing", mode.trim()),
            &service,
            &log,
        ),
        Err(why) => fail(why, &service, &log),
    }

    let probe_path = probe.to_string_lossy().into_owned();
    if let Err(why) = adb.run(&["install", "-r", "-g", &probe_path]) {
        fail(format!("installing the media probe: {why}"), &service, &log);
    }
    match adb.shell(&format!("pm list packages {PROBE_PACKAGE}")) {
        Ok(list) if list.contains(PROBE_PACKAGE) => {}
        Ok(list) => fail(
            format!("the media probe is not installed: {list}"),
            &service,
            &log,
        ),
        Err(why) => fail(why, &service, &log),
    }

    let mut problems = Vec::new();
    if check_root {
        // Keep the first boot's init and kernel messages before the approval check restarts it.
        for (name, command) in [
            ("boot-logcat.txt", "logcat -d -b all"),
            ("guard.txt", "ls -ldZ /system/xbin /system/xbin/su; stat /system/xbin /system/xbin/su; cat /proc/filesystems; cat /proc/mounts; cat /sys/kernel/security/lsm; ls -lZ /sys/kernel/security /sys/kernel/security/dsa_su"),
        ] {
            let output = adb.shell(command).unwrap_or_else(|error| error);
            let _ = std::fs::write(dir.join(name), output);
        }
        match adb.shell("getprop init.svc.dsa_su") {
            Ok(state) if matches!(state.trim(), "" | "stopped") => {}
            Ok(state) => problems.push(format!(
                "the root helper is running while root elevation is off: {state}"
            )),
            Err(why) => problems.push(why),
        }
        match adb.shell("getprop ro.debuggable") {
            Ok(value) if value.trim() == "0" => {}
            Ok(value) => problems.push(format!("ro.debuggable is not 0: {value}")),
            Err(why) => problems.push(why),
        }
        match adb.shell("getprop ro.build.tags") {
            Ok(value) if value.trim() == "release-keys" => {}
            Ok(value) => problems.push(format!("ro.build.tags is not release-keys: {value}")),
            Err(why) => problems.push(why),
        }
        match adb.probe(&dir, "root_absent") {
            Ok(results) => problems.extend(root_absent_problems(&results)),
            Err(why) => problems.push(why),
        }
    }
    if check_media {
        match adb.probe(&dir, "audio") {
            Ok(results) => {
                eprintln!("audio: {results:?}");
                problems.extend(audio_problems(&results));
            }
            Err(why) => problems.push(why),
        }
        match adb.probe(&dir, "decode") {
            Ok(results) => {
                eprintln!("decode: {results:?}");
                problems.extend(decode_problems(&results));
            }
            Err(why) => problems.push(why),
        }
        match &pattern_video {
            Some(Ok(_)) => match adb.probe(&dir, "video") {
                Ok(results) => {
                    eprintln!("video: {results:?}");
                    problems.extend(video_problems(&results));
                }
                Err(why) => problems.push(why),
            },
            Some(Err(why)) => problems.push(format!("the boot check's host decoder: {why}")),
            None => {}
        }
        let _ = std::fs::write(dir.join("video.log"), lock(&video_log).join("\n"));
        // Every denial since the boot, from the guest's own log, where logd keeps the kernel's
        // audit messages.
        let guest_log = adb.shell("logcat -d -b all").unwrap_or_default();
        let _ = std::fs::write(dir.join("logcat.txt"), &guest_log);
        for line in denials(&guest_log, SHARED_MEMORY_TYPES) {
            problems.push(format!("SELinux refused shared memory: {line}"));
        }
        for line in codec_buffer_errors(&guest_log) {
            problems.push(format!("the codec HAL lost a picture: {line}"));
        }
    }
    if check_root {
        if let Err(why) = root_approval_check(&mut api, &adb, &dir) {
            problems.push(format!("root elevation: {why}"));
        }
    }

    video_stop.store(true, Ordering::Relaxed);
    if !problems.is_empty() {
        fail(problems.join("\n"), &service, &log);
    }
}
