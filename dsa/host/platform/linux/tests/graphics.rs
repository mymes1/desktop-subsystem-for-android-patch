//! The graphics path on the real image, end to end, with the host renderer on software Vulkan
//! (Mesa Lavapipe): the service boots the image under crosvm, and on that one boot the test
//! installs an OpenGL ES app (hello-gl2, which draws through ANGLE) and a Vulkan app
//! (hello-vulkan, through the remoting driver) through the local API, opens each in a task
//! window, and runs the fixed Vulkan CTS subset.
//!
//! Each app must get a task whose frames the host places in a window of its own, and what the
//! task's window shows, read through adb (`screencap`, or a `screenrecord` frame when the
//! capture comes back empty), must be what the app draws: a triangle of the app's colour, point
//! up, in the middle of a grey background, and the background must change from one capture to
//! the next, since both apps fade it on every frame. A blank, black or garbled window fails.
//! Neither app may be relaunched after it opens (the events log's `wm_relaunch_activity` and
//! `wm_relaunch_resume_activity`): Android starts at the density its window asks for, and
//! hello-vulkan, which does not handle a change of density, was relaunched before it had a
//! window when the two differed. hello-vulkan's theme also asks for full screen, so the agent
//! starts it at the whole display: started at the framework's phone-sized default, the
//! window filling the monitor resized it across Android's size classes (smallest width and
//! screen layout) right after it opened, and Android relaunched it for that.
//!
//! The CTS subset runs through `dsa/graphics/verify/verify.py cts --software-vulkan`, the same
//! command a PC runs, which keeps each case's result and the dEQP log in the test's folder.
//! It fails on any case that does not pass, except those the subset file excuses on Lavapipe
//! with a reason.
//!
//! Runs when CI gives it what it needs: `DSA_CROSVM` names the crosvm to run,
//! `DSA_WIDGET_TEST_IMAGE` the image folder built from the commit under test,
//! `DSA_WIDGET_TEST_KERNEL` the kernel to boot it with when the folder carries none,
//! `DSA_GRAPHICS_TEST_APPS` the folder with `hello-gl2.apk` and `hello-vulkan.apk`, and
//! `DSA_ADB` the adb to read the guest with (`adb` on the path when not set). The Vulkan CTS
//! APK must already be where `verify.py fetch-cts` puts it; `uv` (or the one `DSA_UV` names)
//! runs the script. It needs `/dev/kvm` and `/dev/vhost-vsock`. What it read stays in its
//! folder (`/tmp/graphics-test-<pid>`) for the job to keep.
#![cfg(target_os = "linux")]

mod boot_check;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use boot_check::adb::{empty_picture, Adb};
use boot_check::{lock, run_bounded, Process as Service, Watchdog};

use dsa_host_core::apk::ApkFile;
use dsa_host_core::guest_log::LogFilter;
use dsa_host_core::host_api::HostApiClient;
use dsa_host_core::tasks::{Bounds, Task};
use dsa_host_unix::socket::UnixLocalConnector;

/// The whole test, boot included, stays under 25 minutes: a boot takes two or three, the apps
/// one or two, and the CTS subset the rest on software Vulkan.
const BUDGET: Duration = Duration::from_secs(1500);
/// How long past its budget the test may run before it stops everything it started and
/// ends: inside the job step's 28 minutes, so the step never kills it with its VM running.
const OVERRUN: Duration = Duration::from_secs(60);
/// How long an app has to get a task, and its task to get its frames placed in a window.
const APP_WAIT: Duration = Duration::from_secs(90);
/// What the CTS needs besides running its cases: installing its APK of 200 MiB and reading
/// its log back.
const CTS_SETUP: Duration = Duration::from_secs(300);
/// The display the guest's whole freeform screen is posted to; a task window never shows it.
const WHOLE_SCREEN_DISPLAY: u32 = 0;

fn scratch() -> PathBuf {
    // Short: the service's socket goes inside, and a socket's path has at most 107 bytes.
    let dir = std::env::temp_dir().join(format!("graphics-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Fails the test with what the service and the window manager printed and the guest log's
/// last lines.
fn fail(why: String, service: &Service, log: &Mutex<Vec<String>>) -> ! {
    // The lock is let go before the panic, so the thread that reads the guest log is not
    // left with a poisoned lock and does not panic on top of this failure.
    let tail = {
        let log = lock(log);
        let _ = std::fs::write(service.log.with_file_name("guest.log"), log.join("\n"));
        log[log.len().saturating_sub(200)..].join("\n")
    };
    let compositor =
        std::fs::read_to_string(service.log.with_file_name("compositor.log")).unwrap_or_default();
    panic!(
        "{why}\n--- the service's output ---\n{}\n--- the window manager's output ---\n{compositor}\n--- the guest log's last lines ---\n{tail}",
        service.log(),
    )
}

fn client(socket: &Path, wait: Duration) -> HostApiClient {
    let mut connector = UnixLocalConnector {
        path: socket.to_path_buf(),
    };
    let deadline = Instant::now() + wait;
    loop {
        match HostApiClient::connect(&mut connector, "graphics-test", Duration::from_secs(30)) {
            Ok(client) => return client,
            Err(why) if Instant::now() >= deadline => {
                panic!("the service did not answer on {}: {why}", socket.display())
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

// ---------------------------------------------------------------- what a window must show

/// The green of hello-gl2's triangle (`gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0)`).
fn green(rgb: [u8; 3]) -> bool {
    let [r, g, b] = rgb;
    g >= 160 && r <= 90 && b <= 90
}

/// The dark red of hello-vulkan's triangle (0.67, 0.1, 0.2): (171, 26, 51) in a UNORM
/// swapchain and (214, 89, 124) in an sRGB one, which the sample prefers when it is offered.
fn dark_red(rgb: [u8; 3]) -> bool {
    let [r, g, b] = rgb.map(i32::from);
    r >= 120 && r >= g + 70 && r >= b + 50
}

/// A grey of the background both apps fade through: red, green and blue (nearly) equal.
fn grey(rgb: [u8; 3]) -> bool {
    let max = rgb.iter().max().unwrap();
    let min = rgb.iter().min().unwrap();
    max - min <= 20
}

/// What a task window showed, when it showed the app's triangle.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Triangle {
    /// The triangle's pixels, and the rectangle they span: `left, top, right, bottom`,
    /// inclusive.
    pixels: usize,
    span: (usize, usize, usize, usize),
    /// The background's mean grey level.
    background: f64,
}

/// Whether the RGBA picture `pixels`, `width` x `height`, is what both test apps draw: a
/// triangle of the colour `ink` takes, point up, about half the picture wide and in its middle
/// (the samples' vertices span -0.5 to 0.5 of the view), on a grey background that covers most
/// of the rest. Says what is wrong when it is not.
fn triangle_in(
    pixels: &[u8],
    width: usize,
    height: usize,
    ink: fn([u8; 3]) -> bool,
) -> Result<Triangle, String> {
    if width < 16 || height < 16 || pixels.len() < width * height * 4 {
        return Err(format!(
            "the window is {width}x{height}, too small to show anything"
        ));
    }
    if empty_picture(&pixels[..width * height * 4]) {
        return Err("the window is blank: every pixel is zero".into());
    }
    let (mut inked, mut greys, mut grey_sum) = (0usize, 0usize, 0f64);
    let (mut left, mut top, mut right, mut bottom) = (usize::MAX, usize::MAX, 0, 0);
    let mut row_ink = vec![0usize; height];
    for (y, row) in row_ink.iter_mut().enumerate() {
        for x in 0..width {
            let at = (y * width + x) * 4;
            let rgb = [pixels[at], pixels[at + 1], pixels[at + 2]];
            if ink(rgb) {
                inked += 1;
                *row += 1;
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = bottom.max(y);
            } else if grey(rgb) {
                greys += 1;
                grey_sum += f64::from(rgb[0]) + f64::from(rgb[1]) + f64::from(rgb[2]);
            }
        }
    }
    let area = (width * height) as f64;
    let ink_share = inked as f64 / area;
    let grey_share = greys as f64 / area;
    let mut problems = Vec::new();
    // The triangle is an eighth of the view; the view may leave part of the window to bars.
    if !(0.03..=0.35).contains(&ink_share) {
        problems.push(format!(
            "{:.1}% of the window is the triangle's colour, not between 3% and 35%",
            100.0 * ink_share
        ));
    }
    if grey_share < 0.4 {
        problems.push(format!(
            "{:.1}% of the window is grey background, less than 40%",
            100.0 * grey_share
        ));
    }
    if inked > 0 {
        let span_width = right - left + 1;
        let span_height = bottom - top + 1;
        let wide = span_width as f64 / width as f64;
        let tall = span_height as f64 / height as f64;
        if !(0.25..=0.75).contains(&wide) {
            problems.push(format!(
                "the triangle spans {:.0}% of the window's width, not about half",
                100.0 * wide
            ));
        }
        if !(0.15..=0.75).contains(&tall) {
            problems.push(format!(
                "the triangle spans {:.0}% of the window's height, not about half",
                100.0 * tall
            ));
        }
        let middle = (left + right) as f64 / 2.0 / width as f64;
        if !(0.3..=0.7).contains(&middle) {
            problems.push(format!(
                "the triangle's middle is {:.0}% across the window, not in its middle",
                100.0 * middle
            ));
        }
        // A triangle fills half the rectangle it spans; a rectangle or scattered pixels do not.
        let fill = inked as f64 / (span_width * span_height) as f64;
        if !(0.35..=0.65).contains(&fill) {
            problems.push(format!(
                "the triangle's colour fills {:.0}% of the rectangle it spans, not about half, so it is no triangle",
                100.0 * fill
            ));
        }
        // Point up: its top quarter is much narrower than its bottom quarter.
        let quarter = (span_height / 4).max(1);
        let mean = |rows: &[usize]| rows.iter().sum::<usize>() as f64 / rows.len() as f64;
        let upper = mean(&row_ink[top..top + quarter]);
        let lower = mean(&row_ink[bottom + 1 - quarter..=bottom]);
        if upper * 2.0 >= lower {
            problems.push(format!(
                "the triangle is {upper:.0} pixels wide near its top and {lower:.0} near its bottom; it must point up"
            ));
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    Ok(Triangle {
        pixels: inked,
        span: (left, top, right, bottom),
        background: grey_sum / (3.0 * greys as f64),
    })
}

/// The part of the RGBA screen `pixels`, `width` wide, inside `bounds`, as `(width, height,
/// pixels)`, cut to the screen.
fn crop(pixels: &[u8], width: usize, bounds: Bounds) -> (usize, usize, Vec<u8>) {
    let height = pixels.len() / (width * 4).max(1);
    let left = (bounds.left.max(0) as usize).min(width);
    let top = (bounds.top.max(0) as usize).min(height);
    let right = (bounds.right.max(0) as usize).clamp(left, width);
    let bottom = (bounds.bottom.max(0) as usize).clamp(top, height);
    let mut out = Vec::with_capacity((right - left) * (bottom - top) * 4);
    for y in top..bottom {
        out.extend_from_slice(&pixels[(y * width + left) * 4..(y * width + right) * 4]);
    }
    (right - left, bottom - top, out)
}

// ---------------------------------------------------------------- the apps

struct App {
    /// The name its results are kept under.
    key: &'static str,
    apk: &'static str,
    package: &'static str,
    /// What draws it, for the messages.
    api: &'static str,
    ink: fn([u8; 3]) -> bool,
    colour: &'static str,
}

/// Android's NDK samples hello-gl2 and hello-vulkan, which the workflow builds.
const APPS: [App; 2] = [
    App {
        key: "gles",
        apk: "hello-gl2.apk",
        package: "com.android.gl2jni",
        api: "OpenGL ES through ANGLE",
        ink: green,
        colour: "green",
    },
    App {
        key: "vulkan",
        apk: "hello-vulkan.apk",
        package: "com.android.hellovk",
        api: "Vulkan through the remoting driver",
        ink: dark_red,
        colour: "dark red",
    },
];

/// The task of `package` that was not among `before`, once it is listed.
fn new_task(
    api: &mut HostApiClient,
    package: &str,
    before: &[i32],
    deadline: Instant,
) -> Result<Task, String> {
    loop {
        let tasks = api.tasks().map_err(|why| format!("listing tasks: {why}"))?;
        if let Some(task) = tasks
            .tasks
            .into_iter()
            .find(|task| task.package_name == package && !before.contains(&task.task_id))
        {
            return Ok(task);
        }
        if Instant::now() >= deadline {
            return Err(format!("{package} never got a task"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The display the host places `task_id`'s frames from, once the hwcomposer has said which:
/// the frames of a task window, never the guest's whole screen.
fn placed_frames(api: &mut HostApiClient, task_id: i32, deadline: Instant) -> Result<u32, String> {
    loop {
        let tasks = api.tasks().map_err(|why| format!("listing tasks: {why}"))?;
        if let Some(&(_, display)) = tasks
            .frames
            .iter()
            .find(|(task, display)| *task == task_id && *display != WHOLE_SCREEN_DISPLAY)
        {
            return Ok(display);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the host never placed frames of task {task_id} in a window (frames {:?})",
                tasks.frames
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Reads what `task`'s window shows, kept in `dir` as `name.png` (the screen) and
/// `name-window.png` (the window), and checks it is `app`'s triangle.
fn read_window(
    api: &mut HostApiClient,
    adb: &Adb,
    dir: &Path,
    name: &str,
    app: &App,
    task_id: i32,
) -> Result<Triangle, String> {
    let bounds = api
        .tasks()
        .map_err(|why| format!("listing tasks: {why}"))?
        .tasks
        .into_iter()
        .find(|task| task.task_id == task_id)
        .map(|task| task.bounds)
        .ok_or_else(|| format!("{}'s task went away", app.package))?;
    let (width, mut screen) = adb.screen(dir, name)?;
    let (mut w, mut h, mut window) = crop(&screen, width, bounds);
    // A GPU buffer can map as zeros for screencap while the rest of the screen does not.
    if !window.is_empty() && empty_picture(&window) {
        let height = screen.len() / (width * 4);
        eprintln!("{name}: the window is all zeros in screencap; reading a screenrecord frame");
        screen = adb.recorded_screen(dir, name, width, height)?;
        (w, h, window) = crop(&screen, width, bounds);
    }
    let _ = std::fs::write(
        dir.join(format!("{name}-window.png")),
        dsa_host_linux::shell::png::encode(
            &dsa_host_linux::shell::png::Rgba {
                width: w as u32,
                height: h as u32,
                pixels: window.clone(),
            },
            false,
        ),
    );
    triangle_in(&window, w, h, app.ink).map_err(|why| {
        format!(
            "{}'s window ({w}x{h} at {bounds:?}) does not show its {} triangle: {why}",
            app.package, app.colour
        )
    })
}

/// Reads `app`'s window in task `task_id` until it shows the app's triangle, then again 3 s
/// later, when the background must have faded on: the app keeps presenting frames.
fn read_twice(
    api: &mut HostApiClient,
    adb: &Adb,
    dir: &Path,
    app: &App,
    task_id: i32,
    display: u32,
) -> Result<String, String> {
    // The first frames can still be on their way: up to six reads, 3 s apart.
    let mut first = Err(String::new());
    for attempt in 1..=6 {
        first = read_window(
            api,
            adb,
            dir,
            &format!("{}-{attempt}", app.key),
            app,
            task_id,
        );
        if first.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    let first = first?;
    std::thread::sleep(Duration::from_secs(3));
    let second = read_window(api, adb, dir, &format!("{}-again", app.key), app, task_id)?;
    if (second.background - first.background).abs() < 1.0 {
        return Err(format!(
            "{}'s window showed the same background ({:.0}) 3 s apart: the app stopped presenting frames",
            app.package, first.background
        ));
    }
    Ok(format!(
        "{} ({}) drew its {} triangle in task {}'s window (display {display}): {} pixels at {:?}, background {:.0} then {:.0}",
        app.package,
        app.api,
        app.colour,
        task_id,
        first.pixels,
        first.span,
        first.background,
        second.background
    ))
}

/// Installs `app` through the local API, opens it in a task window, and checks the window
/// shows what it draws, twice, 3 s apart, with the background faded on between the two.
fn check_app(
    socket: &Path,
    api: &mut HostApiClient,
    adb: &Adb,
    dir: &Path,
    apps: &Path,
    app: &App,
) -> Result<String, String> {
    let path = apps.join(app.apk);
    let mut apk = ApkFile::open(&path)?;
    let (name, size, libraries) = (apk.name.clone(), apk.size, apk.native_libraries.clone());
    let outcome = client(socket, Duration::from_secs(10))
        .install_apk_with_libraries(&name, size, &libraries, apk.reader())
        .map_err(|why| format!("{name} was not installed: {why}"))?;
    if !outcome.installed() {
        return Err(format!("{name} was not installed: {outcome}"));
    }
    let before: Vec<i32> = api
        .tasks()
        .map_err(|why| format!("listing tasks: {why}"))?
        .tasks
        .into_iter()
        .filter(|task| task.package_name == app.package)
        .map(|task| task.task_id)
        .collect();
    api.launch_app(app.package)
        .map_err(|why| format!("opening {}: {why}", app.package))?;
    let deadline = Instant::now() + APP_WAIT;
    let task = new_task(api, app.package, &before, deadline)?;
    let display = placed_frames(api, task.task_id, deadline)?;
    // The app sets up its surface and draws its first frames.
    std::thread::sleep(Duration::from_secs(5));
    let result = read_twice(api, adb, dir, app, task.task_id, display);
    // Android started at the density of the main monitor, which the task window asks for, so
    // the app was never given another one right after it opened, and was never relaunched.
    let relaunched = adb
        .shell("logcat -d -b events -s wm_relaunch_resume_activity wm_relaunch_activity")
        .map(|events| relaunches_of(&events, app.package))
        .map_err(|why| format!("reading the activity relaunches: {why}"));
    let density = adb
        .shell("getprop ro.sf.lcd_density")
        .map(|value| value.trim().to_string());
    let _ = api.close_task(task.task_id);
    let _ = adb.shell(&format!("am force-stop {}", app.package));
    let relaunched = match relaunched {
        Ok(lines) if lines.is_empty() => return result,
        Ok(lines) => format!(
            "{} was relaunched after it opened (Android started at {} dpi), so it lost its \
             state and drew its first frames twice:\n{}",
            app.package,
            density.unwrap_or_else(|why| format!("an unknown density: {why}")),
            lines.join("\n")
        ),
        Err(why) => why,
    };
    Err(match result {
        Ok(_) => relaunched,
        Err(window) => format!("{window}\n{relaunched}"),
    })
}

/// The lines of an events log that say Android relaunched an activity of `package`.
fn relaunches_of(events: &str, package: &str) -> Vec<String> {
    events
        .lines()
        .filter(|line| line.contains("wm_relaunch") && line.contains(&format!("{package}/")))
        .map(str::to_string)
        .collect()
}

#[test]
fn fr4_the_boot_check_finds_an_apps_relaunch_in_the_events_log() {
    let events = "11-09 10:00:01.000  512  540 I wm_relaunch_resume_activity: \
                  [0,1234,5,com.android.hellovk/.VulkanActivity,800]\n\
                  11-09 10:00:02.000  512  540 I wm_relaunch_activity: \
                  [0,99,6,com.example.other/.Main,800]\n";
    assert_eq!(relaunches_of(events, "com.android.hellovk").len(), 1);
    assert!(relaunches_of(events, "com.android.gl2jni").is_empty());
    assert!(relaunches_of("", "com.android.hellovk").is_empty());
}

// ---------------------------------------------------------------- the CTS subset

/// Runs the Vulkan CTS subset through verify.py for `limit` at most, with each case's result
/// kept in `results`. Returns what it printed, and fails when it did not pass.
fn run_cts(adb: &Adb, results: &Path, limit: Duration) -> Result<String, String> {
    let verify = std::env::var_os("DSA_GRAPHICS_VERIFY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../graphics/verify/verify.py")
        });
    let uv = std::env::var_os("DSA_UV").unwrap_or_else(|| "uv".into());
    let cases = limit.saturating_sub(CTS_SETUP).max(Duration::from_secs(60));
    let ran = run_bounded(
        Command::new(uv)
            .arg("run")
            .arg(&verify)
            .arg("cts")
            .arg("--serial")
            .arg(&adb.serial)
            .arg("--software-vulkan")
            .arg("--results")
            .arg(results)
            .arg("--timeout")
            .arg(cases.as_secs().to_string())
            .env("DSA_ADB", &adb.program),
        limit,
        usize::MAX,
    )?;
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::write(results.with_file_name("cts.log"), &said);
    match ran.status {
        Some(status) if status.success() => Ok(said),
        Some(status) => Err(format!("the Vulkan CTS subset failed ({status}):\n{said}")),
        None => Err(format!(
            "the Vulkan CTS subset did not finish within {} s and was stopped:\n{said}",
            limit.as_secs()
        )),
    }
}

// ---------------------------------------------------------------- tests

/// A picture `width` x `height` in the grey `level`, with a triangle in `ink` from
/// `(apex_x, top)` down to a base from `base_left` to `base_right` at `base`.
fn picture(
    (width, height): (usize, usize),
    level: u8,
    ink: [u8; 3],
    (apex_x, top): (usize, usize),
    (base_left, base_right, base): (usize, usize, usize),
) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let inside = y >= top && y <= base && {
                let t = (y - top) as f64 / (base - top) as f64;
                let from = apex_x as f64 + t * (base_left as f64 - apex_x as f64);
                let to = apex_x as f64 + t * (base_right as f64 - apex_x as f64);
                (from.min(to)..=from.max(to)).contains(&(x as f64))
            };
            let rgb = if inside { ink } else { [level; 3] };
            pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    pixels
}

#[test]
fn pr3_the_window_check_finds_each_apps_triangle_on_its_grey_background() {
    let size = (200, 300);
    let gl = picture(size, 90, [0, 255, 0], (100, 75), (50, 150, 225));
    let found = triangle_in(&gl, 200, 300, green).unwrap();
    assert_eq!(found.span, (50, 75, 150, 225));
    assert!((found.background - 90.0).abs() < 0.5);
    // hello-vulkan's colour, in a UNORM and in an sRGB swapchain, on a light grey.
    for red in [[171, 26, 51], [214, 89, 124]] {
        let vk = picture(size, 230, red, (100, 75), (50, 150, 225));
        assert!(triangle_in(&vk, 200, 300, dark_red).is_ok());
        // The other app's colour is not this one's.
        assert!(triangle_in(&vk, 200, 300, green).is_err());
    }
    assert!(!dark_red([128, 128, 128]) && !dark_red([0, 255, 0]));
    assert!(!green([128, 128, 128]) && !green([171, 26, 51]));
}

#[test]
fn pr3_a_blank_black_or_garbled_window_fails_the_window_check() {
    let (width, height) = (200, 300);
    // Nothing drawn at all, as a capture of a buffer that never arrived.
    let zeros = vec![0u8; width * height * 4];
    let why = triangle_in(&zeros, width, height, green).unwrap_err();
    assert!(why.contains("blank"), "{why}");
    // Only the background: the app cleared but never drew.
    let grey_only = picture((width, height), 120, [120; 3], (100, 75), (50, 150, 225));
    let why = triangle_in(&grey_only, width, height, green).unwrap_err();
    assert!(why.contains("triangle's colour"), "{why}");
    // A whole rectangle of the colour is no triangle.
    let mut block = grey_only.clone();
    for y in 75..225 {
        for x in 50..150 {
            block[(y * width + x) * 4..(y * width + x) * 4 + 3].copy_from_slice(&[0, 255, 0]);
        }
    }
    let why = triangle_in(&block, width, height, green).unwrap_err();
    assert!(why.contains("no triangle"), "{why}");
    // Upside down: the upright triangle with its rows in the opposite order.
    let upright = picture((width, height), 120, [0, 255, 0], (100, 75), (50, 150, 225));
    assert!(triangle_in(&upright, width, height, green).is_ok());
    let upside_down: Vec<u8> = upright
        .chunks_exact(width * 4)
        .rev()
        .flatten()
        .copied()
        .collect();
    let why = triangle_in(&upside_down, width, height, green).unwrap_err();
    assert!(why.contains("point up"), "{why}");
    // In a corner rather than the middle.
    let corner = picture((width, height), 120, [0, 255, 0], (30, 75), (5, 55, 225));
    let why = triangle_in(&corner, width, height, green).unwrap_err();
    assert!(why.contains("middle"), "{why}");
}

#[test]
fn pr3_a_window_is_cut_from_the_screen_by_its_tasks_bounds() {
    // A 4x3 screen whose pixels are numbered; the task covers columns 1..3 of rows 1..3.
    let screen: Vec<u8> = (0..12u8).flat_map(|i| [i, i, i, 255]).collect();
    let bounds = Bounds {
        left: 1,
        top: 1,
        right: 3,
        bottom: 3,
    };
    let (w, h, pixels) = crop(&screen, 4, bounds);
    assert_eq!((w, h), (2, 2));
    let firsts: Vec<u8> = pixels.chunks_exact(4).map(|p| p[0]).collect();
    assert_eq!(firsts, [5, 6, 9, 10]);
    // Bounds past the screen are cut to it.
    let past = Bounds {
        left: -5,
        top: 2,
        right: 99,
        bottom: 99,
    };
    let (w, h, _) = crop(&screen, 4, past);
    assert_eq!((w, h), (4, 1));
}

#[test]
fn pr3_the_cts_subset_and_a_gles_and_a_vulkan_app_render_in_task_windows_on_the_real_image() {
    let (Some(crosvm), Some(image), Some(apps)) = (
        std::env::var_os("DSA_CROSVM").map(PathBuf::from),
        std::env::var_os("DSA_WIDGET_TEST_IMAGE").map(PathBuf::from),
        std::env::var_os("DSA_GRAPHICS_TEST_APPS").map(PathBuf::from),
    ) else {
        eprintln!(
            "skipped: DSA_CROSVM, DSA_WIDGET_TEST_IMAGE and DSA_GRAPHICS_TEST_APPS name no crosvm, image and test apps"
        );
        return;
    };
    let kernel = std::env::var_os("DSA_WIDGET_TEST_KERNEL").map(PathBuf::from);
    let adb_program = std::env::var_os("DSA_ADB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("adb"));
    let started = Instant::now();
    let deadline = started + BUDGET;
    let _watchdog = Watchdog::start("the graphics check", BUDGET + OVERRUN);
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
    // Android's renderer is started by the window manager, which shows Android's task windows
    // in the desktop session the job runs (a headless Wayland compositor), and renders on the
    // software Vulkan driver the job installs.
    let _compositor = Service::spawn(
        Command::new(env!("CARGO_BIN_EXE_dsa-compositor")).env("DSA_SOCKET", &socket),
        dir.join("compositor.log"),
    );
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
    // The boot may take up to ten minutes; the rest is for the apps and the CTS.
    let boot = Duration::from_secs(600);
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
        blank_screencaps: Default::default(),
    };
    if let Err(why) = adb.connect(Duration::from_secs(120)) {
        fail(
            format!("adb did not reach the guest: {why}"),
            &service,
            &log,
        );
    }

    // Each check that fails is noted and the others still run, so one failure does not hide
    // the next; the test fails at the end with all of them.
    let mut failures: Vec<String> = Vec::new();
    for app in &APPS {
        match check_app(&socket, &mut api, &adb, &dir, &apps, app) {
            Ok(done) => eprintln!("{done}"),
            Err(why) => {
                eprintln!("{}: {why}", app.key);
                failures.push(why);
            }
        }
    }

    // The CTS subset gets what is left of the budget, less a minute to stop the VM.
    let left = deadline
        .saturating_duration_since(Instant::now())
        .saturating_sub(Duration::from_secs(60));
    let results = dir.join("cts");
    match run_cts(&adb, &results, left) {
        Ok(said) => eprintln!("{said}"),
        Err(why) => {
            eprintln!("{why}");
            failures.push(why);
        }
    }

    if !failures.is_empty() {
        fail(failures.join("\n\n"), &service, &log);
    }

    let _ = api.stop();
    drop(service);
    eprintln!(
        "the graphics check took {} s; what it read is in {}",
        started.elapsed().as_secs(),
        dir.display()
    );
}
