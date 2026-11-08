//! The design of Android's own screens on the real image, end to end: the service boots the
//! image under crosvm, the test opens Settings in task windows of a tall and a short height
//! through the local API, and reads what Android drew through adb (developer mode): the view
//! hierarchy (`uiautomator dump`) and the screen (`screencap`).
//!
//! It checks that Settings' home page opens with its large title expanded in a window 800 dp
//! tall and folds it when the list scrolls, that it opens folded in a window under 600 dp
//! tall, and that the rows of a Settings list are grouped in rounded cards: the first row of a
//! group has rounded corners, and a row joined to the one above it square ones.
//!
//! Runs when CI gives it what it needs: `DSA_CROSVM` names the crosvm to run,
//! `DSA_WIDGET_TEST_IMAGE` the image folder built from the commit under test,
//! `DSA_WIDGET_TEST_KERNEL` the kernel to boot it with when the folder carries none, and
//! `DSA_ADB` the adb to read the guest with (`adb` on the path when not set). It needs
//! `/dev/kvm` and `/dev/vhost-vsock`. The pictures and hierarchies it read stay in its folder
//! (`/tmp/screens-test-<pid>`) for the job to keep.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsa_host_core::apk_info::inflate::inflate;
use dsa_host_core::guest_log::LogFilter;
use dsa_host_core::host_api::HostApiClient;
use dsa_host_unix::socket::UnixLocalConnector;

const SETTINGS: &str = "com.android.settings";
const SETTINGS_HOME: &str = "com.android.settings/.homepage.SettingsHomepageActivity";
/// A Settings page with several groups of rows.
const DISPLAY_SETTINGS: &str = "android.settings.DISPLAY_SETTINGS";
/// The window heights checked, in dp: one well above the 600 dp the large title needs, and
/// one below it.
const TALL_DP: u32 = 800;
const SHORT_DP: u32 = 520;
const WIDTH_DP: u32 = 480;
/// The whole test, boot included, stays under twelve minutes.
const BUDGET: Duration = Duration::from_secs(690);

fn scratch() -> PathBuf {
    // Short: the service's socket goes inside, and a socket's path has at most 107 bytes.
    let dir = std::env::temp_dir().join(format!("screens-test-{}", std::process::id()));
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
    let compositor =
        std::fs::read_to_string(service.log.with_file_name("compositor.log")).unwrap_or_default();
    panic!(
        "{why}\n--- the service's output ---\n{}\n--- the window manager's output ---\n{compositor}\n--- the guest log's last lines ---\n{}",
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
        match HostApiClient::connect(&mut connector, "screens-test", Duration::from_secs(30)) {
            Ok(client) => return client,
            Err(why) if Instant::now() >= deadline => {
                panic!("the service did not answer on {}: {why}", socket.display())
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

/// A rectangle in display pixels, as uiautomator writes one: `[left,top][right,bottom]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Rect {
    fn parse(text: &str) -> Option<Rect> {
        let numbers: Vec<i32> = text
            .split(['[', ']', ','])
            .filter(|part| !part.is_empty())
            .map(|part| part.parse().ok())
            .collect::<Option<Vec<i32>>>()?;
        match numbers[..] {
            [left, top, right, bottom] => Some(Rect {
                left,
                top,
                right,
                bottom,
            }),
            _ => None,
        }
    }

    fn width(&self) -> i32 {
        self.right - self.left
    }

    fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// One node of a uiautomator dump, with the index of its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    parent: Option<usize>,
    class: String,
    text: String,
    id: String,
    bounds: Rect,
}

/// The value of attribute `name` in the attributes text of an XML element.
fn attribute(element: &str, name: &str) -> Option<String> {
    let start = element.find(&format!(" {name}=\""))? + name.len() + 3;
    let end = element[start..].find('"')? + start;
    Some(
        element[start..end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

/// The nodes of a uiautomator dump, in document order.
fn nodes(xml: &str) -> Vec<Node> {
    let mut found = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut at = 0;
    while let Some(next) = xml[at..].find('<') {
        let start = at + next;
        let Some(length) = xml[start..].find('>') else {
            break;
        };
        let tag = &xml[start..start + length + 1];
        at = start + length + 1;
        if tag.starts_with("</node") {
            open.pop();
        } else if tag.starts_with("<node") {
            let index = found.len();
            found.push(Node {
                parent: open.last().copied(),
                class: attribute(tag, "class").unwrap_or_default(),
                text: attribute(tag, "text").unwrap_or_default(),
                id: attribute(tag, "resource-id").unwrap_or_default(),
                bounds: attribute(tag, "bounds")
                    .and_then(|b| Rect::parse(&b))
                    .unwrap_or(Rect {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    }),
            });
            if !tag.ends_with("/>") {
                open.push(index);
            }
        }
    }
    found
}

/// The first node whose id is Settings' `name`.
fn by_id<'a>(nodes: &'a [Node], name: &str) -> Option<(usize, &'a Node)> {
    let id = format!("{SETTINGS}:id/{name}");
    nodes.iter().enumerate().find(|(_, node)| node.id == id)
}

/// The large title of Settings' home page: a text view with text above the search bar, at
/// least twice its height. Settings hides the search bar when its search app is off; then
/// the title is above the list, at least a sixth of its height. None when the home page shows
/// no such title (folded).
fn large_title(nodes: &[Node]) -> Result<Option<Rect>, String> {
    let (below, least) = match by_id(nodes, "search_action_bar") {
        Some((_, search)) => (search.bounds.top, 2 * search.bounds.height()),
        None => {
            let (_, list) = by_id(nodes, "main_content_scrollable_container").ok_or(
                "the dump has neither the home page's search bar nor its list \
                 (com.android.settings:id/search_action_bar, main_content_scrollable_container)",
            )?;
            (list.bounds.top, list.bounds.height() / 6)
        }
    };
    Ok(nodes
        .iter()
        .filter(|node| node.class.ends_with("TextView") && !node.text.is_empty())
        .map(|node| node.bounds)
        .find(|bounds| bounds.bottom <= below && bounds.height() > 0 && bounds.height() >= least))
}

/// The pixel at (x, y) of an RGBA picture `width` wide.
fn pixel(pixels: &[u8], width: usize, x: i32, y: i32) -> [u8; 3] {
    let at = (y as usize * width + x as usize) * 4;
    [pixels[at], pixels[at + 1], pixels[at + 2]]
}

fn differs(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter()
        .zip(b.iter())
        .any(|(a, b)| (*a as i16 - *b as i16).abs() > 12)
}

/// How a list row's card is drawn: whether its top corners are rounded (the page shows at
/// the corner) or square (the card goes on to the row above), or None when the row shows no
/// card on the page.
fn top_corner(pixels: &[u8], width: usize, row: Rect) -> Option<bool> {
    let middle = (row.top + row.bottom) / 2;
    let page = pixel(pixels, width, row.left + 1, middle);
    // The card's left edge: SettingsLib insets it from the row's side.
    let edge = (row.left + 1..row.left + row.width() / 3)
        .find(|&x| differs(pixel(pixels, width, x, middle), page))?;
    let card = pixel(pixels, width, edge + 4, middle);
    // A card is inset as far on the right: a row of text on the page (a group's heading) is
    // not.
    let mirror = row.right - 1 - (edge - row.left) - 4;
    if mirror <= edge || differs(pixel(pixels, width, mirror, middle), card) {
        return None;
    }
    let corner = pixel(pixels, width, edge + 1, row.top + 1);
    if !differs(corner, page) {
        Some(true)
    } else if !differs(corner, card) {
        Some(false)
    } else {
        None
    }
}

/// Whether the rows of the list in `nodes` are grouped in rounded cards, read from the
/// screen `pixels`: some row's card has rounded top corners and some row's square ones.
fn rounded_cards(nodes: &[Node], pixels: &[u8], width: usize) -> Result<(), String> {
    let (list, list_node) = by_id(nodes, "recycler_view")
        .ok_or("the dump has no list (com.android.settings:id/recycler_view)")?;
    let height = (pixels.len() / (width * 4)) as i32;
    let rows: Vec<Rect> = nodes
        .iter()
        .filter(|node| node.parent == Some(list))
        .map(|node| node.bounds)
        .filter(|bounds| bounds.width() * 10 >= list_node.bounds.width() * 9 && bounds.height() > 8)
        // Whole rows on the screen only.
        .filter(|bounds| {
            bounds.left >= 0
                && bounds.top >= 0
                && bounds.right <= width as i32
                && bounds.bottom <= height
        })
        .collect();
    let corners: Vec<Option<bool>> = rows
        .iter()
        .map(|&row| top_corner(pixels, width, row))
        .collect();
    let rounded = corners.iter().filter(|c| **c == Some(true)).count();
    let square = corners.iter().filter(|c| **c == Some(false)).count();
    if rounded == 0 || square == 0 {
        return Err(format!(
            "of {} rows, {rounded} start a rounded card and {square} join the card above; a list of \
             grouped cards has both (rows {rows:?}, corners {corners:?})",
            rows.len()
        ));
    }
    Ok(())
}

/// The pixels of a PNG with 8 bits per sample, as `(width, height, channels, rows)`.
fn decode_png(png: &[u8]) -> Result<(usize, usize, usize, Vec<u8>), String> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if png.len() < 8 || png[..8] != SIGNATURE {
        return Err("not a PNG".into());
    }
    let (mut at, mut header, mut data) = (8, None, Vec::new());
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        let kind = &png[at + 4..at + 8];
        let body = png
            .get(at + 8..at + 8 + len)
            .ok_or("a PNG chunk runs past the end")?;
        match kind {
            b"IHDR" => header = Some(body.to_vec()),
            b"IDAT" => data.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    let header = header.ok_or("the PNG has no header")?;
    let width = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
    let height = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
    let (depth, colour, interlace) = (header[8], header[9], header[12]);
    let channels = match colour {
        2 => 3,
        6 => 4,
        other => {
            return Err(format!(
                "the PNG has colour type {other}, which is not checked"
            ))
        }
    };
    if depth != 8 || interlace != 0 {
        return Err(format!(
            "the PNG has {depth} bits per sample and interlace {interlace}; 8 and none are checked"
        ));
    }
    if data.len() < 2 {
        return Err("the PNG has no image data".into());
    }
    let stride = width * channels;
    // A zlib stream: two bytes of header before the DEFLATE data.
    let raw = inflate(&data[2..], height * (stride + 1))?;
    if raw.len() < height * (stride + 1) {
        return Err("the PNG's image data is short".into());
    }
    let mut out = vec![0u8; height * stride];
    for y in 0..height {
        let filter = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for x in 0..stride {
            let a = if x >= channels {
                out[y * stride + x - channels]
            } else {
                0
            };
            let b = if y > 0 { out[(y - 1) * stride + x] } else { 0 };
            let c = if x >= channels && y > 0 {
                out[(y - 1) * stride + x - channels]
            } else {
                0
            };
            let predicted = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => {
                    let p = a as i16 + b as i16 - c as i16;
                    let pa = (p - a as i16).abs();
                    let pb = (p - b as i16).abs();
                    let pc = (p - c as i16).abs();
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                other => return Err(format!("the PNG uses filter {other}")),
            };
            out[y * stride + x] = line[x].wrapping_add(predicted);
        }
    }
    // RGBA, whatever the PNG carried.
    if channels == 3 {
        out = out
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
    }
    Ok((width, height, 4, out))
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

    /// The view hierarchy of the focused window, kept in `dir` as `name.xml`.
    fn dump(&self, dir: &Path, name: &str) -> Result<Vec<Node>, String> {
        self.shell("uiautomator dump /data/local/tmp/dsa-screens.xml")?;
        let xml = self.run(&["exec-out", "cat", "/data/local/tmp/dsa-screens.xml"])?;
        let _ = std::fs::write(dir.join(format!("{name}.xml")), &xml);
        Ok(nodes(&String::from_utf8_lossy(&xml)))
    }

    /// The screen, kept in `dir` as `name.png`, as `(width, RGBA pixels)`.
    fn screen(&self, dir: &Path, name: &str) -> Result<(usize, Vec<u8>), String> {
        let png = self.run(&["exec-out", "screencap", "-p"])?;
        let _ = std::fs::write(dir.join(format!("{name}.png")), &png);
        let (width, _, _, pixels) = decode_png(&png)?;
        Ok((width, pixels))
    }
}

/// Gives Settings' task a window `height_dp` tall, then opens its home page afresh in it, so
/// the page is laid out for that height: the home page keeps its views when its window is
/// resized. Returns the task's height in dp as Android has it.
fn open_settings_at(
    api: &mut HostApiClient,
    adb: &Adb,
    height_dp: u32,
    wait: Duration,
) -> Result<u32, String> {
    let deadline = Instant::now() + wait;
    let _ = adb.shell(&format!("am force-stop {SETTINGS}"));
    api.launch_app(SETTINGS)
        .map_err(|why| format!("opening Settings: {why}"))?;
    let task = loop {
        let tasks = api.tasks().map_err(|why| format!("listing tasks: {why}"))?;
        if let Some(task) = tasks
            .tasks
            .into_iter()
            .find(|task| task.package_name == SETTINGS)
        {
            break task;
        }
        if Instant::now() >= deadline {
            return Err("Settings' task never appeared".into());
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let dpi = if task.density_dpi == 0 {
        160
    } else {
        task.density_dpi
    };
    let px = |dp: u32| dp * dpi / 160;
    api.resize_task(task.task_id, px(WIDTH_DP), px(height_dp), false)
        .map_err(|why| format!("resizing Settings' task: {why}"))?;
    let height = loop {
        let tasks = api.tasks().map_err(|why| format!("listing tasks: {why}"))?;
        let bounds = tasks
            .tasks
            .iter()
            .find(|t| t.task_id == task.task_id)
            .map(|t| t.bounds)
            .ok_or("Settings' task went away")?;
        let height = (bounds.bottom - bounds.top).max(0) as u32 * 160 / dpi;
        if height.abs_diff(height_dp) <= 2 || Instant::now() >= deadline {
            break height;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    adb.shell(&format!(
        "am start -W --activity-clear-task -n {SETTINGS_HOME}"
    ))?;
    // The page lays out and draws its list.
    std::thread::sleep(Duration::from_secs(5));
    Ok(height)
}

#[test]
fn fr34_the_dump_parser_reads_nodes_their_parents_and_bounds() {
    let xml = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node index="0" text="" resource-id="com.android.settings:id/app_bar" class="android.widget.LinearLayout" bounds="[0,0][480,300]"><node index="0" text="Settings &amp; more" resource-id="" class="android.widget.TextView" bounds="[0,0][480,200]" /><node index="1" text="" resource-id="com.android.settings:id/search_action_bar" class="android.widget.LinearLayout" bounds="[16,208][464,264]"><node index="0" text="Search" resource-id="com.android.settings:id/search_bar_title" class="android.widget.TextView" bounds="[60,220][120,250]" /></node></node></hierarchy>"#;
    let found = nodes(xml);
    assert_eq!(found.len(), 4);
    assert_eq!(found[1].text, "Settings & more");
    assert_eq!(found[1].parent, Some(0));
    assert_eq!(found[3].parent, Some(2));
    assert_eq!(
        found[2].bounds,
        Rect {
            left: 16,
            top: 208,
            right: 464,
            bottom: 264
        }
    );
    // The title above the search bar, more than twice its height: expanded.
    assert_eq!(
        large_title(&found).unwrap(),
        Some(Rect {
            left: 0,
            top: 0,
            right: 480,
            bottom: 200
        })
    );
    // Without it, folded.
    let folded: Vec<Node> = found
        .iter()
        .filter(|n| n.text != "Settings & more")
        .cloned()
        .collect();
    assert_eq!(large_title(&folded).unwrap(), None);
}

#[test]
fn fr34_the_card_check_tells_rounded_cards_from_square_rows() {
    // A 100x60 screen in the page's grey, with a white card from x 10 to 90 over two rows
    // (y 0..30 and 30..60): the top row's top corners cut round, the second row's square.
    let (width, height) = (100usize, 60usize);
    let mut pixels = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let in_card = (10..90).contains(&x);
            let cut = y < 4 && (x < 14 || x >= 86);
            let colour = if in_card && !cut { 255 } else { 230 };
            pixels[(y * width + x) * 4..(y * width + x) * 4 + 4]
                .copy_from_slice(&[colour, colour, colour, 255]);
        }
    }
    let top = Rect {
        left: 0,
        top: 0,
        right: 100,
        bottom: 30,
    };
    let joined = Rect {
        left: 0,
        top: 30,
        right: 100,
        bottom: 60,
    };
    assert_eq!(top_corner(&pixels, width, top), Some(true));
    assert_eq!(top_corner(&pixels, width, joined), Some(false));
    let list = |rows: &[Rect]| {
        let mut all = vec![Node {
            parent: None,
            class: "androidx.recyclerview.widget.RecyclerView".into(),
            text: String::new(),
            id: format!("{SETTINGS}:id/recycler_view"),
            bounds: Rect {
                left: 0,
                top: 0,
                right: 100,
                bottom: 60,
            },
        }];
        all.extend(rows.iter().map(|&bounds| Node {
            parent: Some(0),
            class: "android.widget.LinearLayout".into(),
            text: String::new(),
            id: String::new(),
            bounds,
        }));
        all
    };
    assert!(rounded_cards(&list(&[top, joined]), &pixels, width).is_ok());
    // Every row a card of its own, or no card at all, is not a list of grouped cards.
    assert!(rounded_cards(&list(&[top]), &pixels, width).is_err());
}

#[test]
fn fr34_settings_opens_with_a_large_title_that_folds_and_lists_its_rows_in_rounded_cards() {
    let (Some(crosvm), Some(image)) = (
        std::env::var_os("DSA_CROSVM").map(PathBuf::from),
        std::env::var_os("DSA_WIDGET_TEST_IMAGE").map(PathBuf::from),
    ) else {
        eprintln!("skipped: DSA_CROSVM and DSA_WIDGET_TEST_IMAGE name no crosvm and image");
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
    // Android's renderer is started by the window manager, which shows Android's task windows
    // in the desktop session the job runs (a headless Wayland compositor).
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

    // A tall window: the home page opens with its large title, which folds as the list scrolls.
    let wait = Duration::from_secs(60);
    let height = match open_settings_at(&mut api, &adb, TALL_DP, wait) {
        Ok(height) => height,
        Err(why) => fail(why, &service, &log),
    };
    if height < 600 {
        fail(
            format!("Settings' window is {height} dp tall, not the {TALL_DP} dp asked for"),
            &service,
            &log,
        );
    }
    let tall = adb
        .dump(&dir, "home-tall")
        .unwrap_or_else(|why| fail(why, &service, &log));
    let _ = adb.screen(&dir, "home-tall");
    let title = match large_title(&tall) {
        Ok(Some(title)) => title,
        Ok(None) => fail(
            format!("Settings opened in a window {height} dp tall without its large title"),
            &service,
            &log,
        ),
        Err(why) => fail(why, &service, &log),
    };
    eprintln!("the large title is at {title:?} in a window {height} dp tall");
    let scroll = by_id(&tall, "main_content_scrollable_container")
        .map(|(_, node)| node.bounds)
        .unwrap_or_else(|| {
            fail(
                "the dump has no home page list (main_content_scrollable_container)".into(),
                &service,
                &log,
            )
        });
    let x = (scroll.left + scroll.right) / 2;
    if let Err(why) = adb.shell(&format!(
        "input swipe {x} {} {x} {} 600",
        scroll.bottom - 20,
        scroll.top + 20
    )) {
        fail(why, &service, &log);
    }
    std::thread::sleep(Duration::from_secs(2));
    let scrolled = adb
        .dump(&dir, "home-tall-scrolled")
        .unwrap_or_else(|why| fail(why, &service, &log));
    let _ = adb.screen(&dir, "home-tall-scrolled");
    match large_title(&scrolled) {
        Ok(None) => {}
        Ok(Some(still)) => fail(
            format!("the large title stayed at {still:?} when the list scrolled"),
            &service,
            &log,
        ),
        Err(why) => fail(why, &service, &log),
    }

    // A Settings list, in the same window: its rows are grouped in rounded cards.
    if let Err(why) = adb.shell(&format!("am start -W -a {DISPLAY_SETTINGS}")) {
        fail(why, &service, &log);
    }
    std::thread::sleep(Duration::from_secs(5));
    let page = adb
        .dump(&dir, "display")
        .unwrap_or_else(|why| fail(why, &service, &log));
    let (width, pixels) = adb
        .screen(&dir, "display")
        .unwrap_or_else(|why| fail(why, &service, &log));
    if let Err(why) = rounded_cards(&page, &pixels, width) {
        fail(format!("Settings' display page: {why}"), &service, &log);
    }

    // A short window: the home page opens folded.
    let height = match open_settings_at(&mut api, &adb, SHORT_DP, wait) {
        Ok(height) => height,
        Err(why) => fail(why, &service, &log),
    };
    if height >= 600 {
        fail(
            format!("Settings' window is {height} dp tall, not the {SHORT_DP} dp asked for"),
            &service,
            &log,
        );
    }
    let short = adb
        .dump(&dir, "home-short")
        .unwrap_or_else(|why| fail(why, &service, &log));
    let _ = adb.screen(&dir, "home-short");
    match large_title(&short) {
        Ok(None) => {}
        Ok(Some(title)) => fail(
            format!(
                "Settings opened in a window {height} dp tall with its large title at {title:?}"
            ),
            &service,
            &log,
        ),
        Err(why) => fail(why, &service, &log),
    }

    let _ = api.stop();
    drop(service);
    eprintln!(
        "the system screen test took {} s; its pictures are in {}",
        started.elapsed().as_secs(),
        dir.display()
    );
}
