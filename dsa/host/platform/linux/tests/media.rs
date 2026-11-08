//! Sound in an ordinary app on the real image, end to end, with SELinux enforcing: the service
//! boots the image under crosvm, the test installs the media probe (`dsa/guest/media-probe/`)
//! through adb (developer mode) and runs its instrumentation, which plays sound in the app's
//! own process.
//!
//! An app runs in the untrusted app domain at an MLS level of its own, and its sound goes
//! through memory it shares with the audio server: a streaming track's control block, which
//! the audio server makes, and a static track's buffer, which the app makes. When the guest's
//! policy refused either, `AudioTrack` failed with "Could not get control block" and no app
//! played a sound, while the platform's own sounds, made at the services' level, still played.
//! The test also reads the guest's log for a denial of a media service's shared memory.
//!
//! Runs when CI gives it what it needs: `DSA_CROSVM` names the crosvm to run,
//! `DSA_WIDGET_TEST_IMAGE` the image folder built from the commit under test,
//! `DSA_WIDGET_TEST_KERNEL` the kernel to boot it with when the folder carries none,
//! `DSA_MEDIA_PROBE` the probe's APK, and `DSA_ADB` the adb to reach the guest with (`adb` on
//! the path when not set). It needs `/dev/kvm` and `/dev/vhost-vsock`. What it read stays in
//! its folder (`/tmp/media-test-<pid>`) for the job to keep.
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsa_host_core::guest_log::LogFilter;
use dsa_host_core::host_api::HostApiClient;
use dsa_host_unix::socket::UnixLocalConnector;

const PROBE_PACKAGE: &str = "io.github.thisisthepy.dsa.mediaprobe";
const PROBE_RUNNER: &str = "io.github.thisisthepy.dsa.mediaprobe/.MediaProbe";
/// `AudioTrack.STATE_INITIALIZED`.
const TRACK_INITIALIZED: i64 = 1;
/// The tone the probe plays: a second at 48 kHz.
const TONE_FRAMES: i64 = 48_000;
/// The memfd types of the media services: a denial naming one means an app or a service could
/// not use memory another process shared with it.
const SHARED_MEMORY_TYPES: &[&str] = &[
    "audioserver_tmpfs",
    "mediaserver_tmpfs",
    "mediaextractor_tmpfs",
    "cameraserver_tmpfs",
    "appdomain_tmpfs",
];
/// The whole test, boot included, stays under twelve minutes.
const BUDGET: Duration = Duration::from_secs(690);

fn scratch() -> PathBuf {
    // Short: the service's socket goes inside, and a socket's path has at most 107 bytes.
    let dir = std::env::temp_dir().join(format!("media-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A child process, stopped (SIGTERM, then SIGKILL) when dropped, so a failing test leaves no
/// VM.
struct Service {
    child: Child,
    log: PathBuf,
}

impl Service {
    fn spawn(command: &mut Command, log: PathBuf) -> Service {
        let out = std::fs::File::create(&log).unwrap();
        let child = command
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .expect("starting a DSA program");
        Service { child, log }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // SAFETY: kill with a child's own process ID and a constant signal.
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Fails the test with what the service printed and the guest log's last lines.
fn fail(why: String, service: &Service, log: &Mutex<Vec<String>>) -> ! {
    let log = log.lock().unwrap();
    let _ = std::fs::write(service.log.with_file_name("guest.log"), log.join("\n"));
    let tail = &log[log.len().saturating_sub(200)..];
    let devices =
        std::fs::read_to_string(service.log.with_file_name("devices.log")).unwrap_or_default();
    panic!(
        "{why}\n--- the service's output ---\n{}\n--- the devices' output ---\n{devices}\n--- the guest log's last lines ---\n{}",
        service.log(),
        tail.join("\n")
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
    fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let out = Command::new(&self.program)
            .arg("-s")
            .arg(&self.serial)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|why| format!("running {}: {why}", self.program.display()))?;
        if !out.status.success() {
            return Err(format!(
                "adb {} failed: {}{}",
                args.join(" "),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(out.stdout)
    }

    fn shell(&self, command: &str) -> Result<String, String> {
        self.run(&["shell", command])
            .map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    /// Connects, and waits until the guest's shell answers.
    fn connect(&self, wait: Duration) -> Result<(), String> {
        let deadline = Instant::now() + wait;
        let _ = Command::new(&self.program).arg("start-server").output();
        loop {
            let _ = Command::new(&self.program)
                .arg("connect")
                .arg(&self.serial)
                .output();
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
fn pr4_a_denial_of_a_media_services_shared_memory_is_found_in_the_log() {
    let log = "11-08 12:00:01.554  4650  4650 I auditd  : type=1400 audit(0.0:30): avc:  denied  { read write } for  comm=\"binder:772_3\" path=\"/memfd:AudioFlinger::Client (deleted)\" dev=\"tmpfs\" ino=120 scontext=u:r:untrusted_app:s0:c149,c256,c512,c768 tcontext=u:object_r:audioserver_tmpfs:s0 tclass=file permissive=0\n\
               11-08 12:00:02.000  4650  4650 I auditd  : type=1400 audit(0.0:31): avc:  denied  { read } for  comm=\"uiautomator\" scontext=u:r:shell:s0 tcontext=u:object_r:qemu_sf_lcd_density_prop:s0 tclass=file permissive=0\n\
               11-08 12:00:03.000  4650  4650 I AudioTrack: start\n";
    let found = denials(log, SHARED_MEMORY_TYPES);
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("audioserver_tmpfs"));
}

#[test]
fn pr4_an_app_plays_sound_with_selinux_enforcing() {
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
    let deadline = started + BUDGET;
    let dir = scratch();
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
    });
    std::fs::write(&config, settings.to_string()).unwrap();
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
                        let mut log = log.lock().unwrap();
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
    match adb.probe(&dir, "audio") {
        Ok(results) => {
            eprintln!("audio: {results:?}");
            problems.extend(audio_problems(&results));
        }
        Err(why) => problems.push(why),
    }

    // Every denial since the boot, from the guest's own log, where logd keeps the kernel's
    // audit messages.
    let guest_log = adb.shell("logcat -d -b all").unwrap_or_default();
    let _ = std::fs::write(dir.join("logcat.txt"), &guest_log);
    for line in denials(&guest_log, SHARED_MEMORY_TYPES) {
        problems.push(format!("SELinux refused shared memory: {line}"));
    }

    if !problems.is_empty() {
        fail(problems.join("\n"), &service, &log);
    }
}
