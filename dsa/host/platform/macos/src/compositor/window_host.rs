//! An app's window host on macOS: the process that owns the app's task windows.
//!
//! Each Android app's bundle in `~/Applications/DSA Apps` runs a copy of this program, so
//! the Dock shows the app as itself, with its own icon, apart from every other app. The
//! window host connects to the window manager over a Unix socket and is told which windows
//! to show; each task's frames arrive in an `IOSurface` the window manager writes. The
//! windows' input goes straight to the service's local API.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsa_host_core::compositor::window_hosts::{
    connect_window_host, report_to_wire, to_host_from_wire, FromHost, ToHost,
};
use dsa_host_core::compositor::window_mode::WindowMode;
use dsa_host_core::files::{FileAction, FileTransfer};
use dsa_host_core::host_api::{HostApiClient, InputStream};
use dsa_host_core::ime::ImeCursor;
use dsa_host_core::input::InputEvent;
use dsa_host_unix::socket::UnixLocalConnector;
use dsa_protocol::window_host::types::{envelope::Body, Attach, FramePresented};
use dsa_protocol::window_host::{decode, encode_body};
use dsa_transport::{Connector, FrameSender};
use objc2::msg_send;
use objc2::runtime::AnyObject;

use super::metal::FrameRef;
use super::surface::Surface;
use super::window::{TaskWindow, WindowEvents};
use crate::input::view::InputSink;
use crate::objc::{class, on_main, string_of};
use crate::shell::app_stubs::PACKAGE_KEY;

/// The build named in the handshake.
pub const BUILD: &str = concat!("dsa-app-window ", env!("CARGO_PKG_VERSION"));
/// How long the connection's opening may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long input waits before it tries the service again after it could not reach it.
const INPUT_RETRY: Duration = Duration::from_secs(2);

/// The window manager's socket for a user whose DSA folder is `support`.
pub fn window_manager_socket(support: &Path) -> PathBuf {
    support.join("window-manager.sock")
}

/// The local API's socket for a user whose DSA folder is `support`.
pub fn host_api_socket(support: &Path) -> PathBuf {
    support.join("host.sock")
}

/// The user's DSA folder: `~/Library/Application Support/DSA`.
pub fn support_folder() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support/DSA"))
}

/// The package this program shows: the `--package` argument, else the `DSAPackage` key of
/// the bundle it runs from.
pub fn package_of(args: &[String]) -> Option<String> {
    if let Some(at) = args.iter().position(|a| a == "--package") {
        return args.get(at + 1).cloned();
    }
    unsafe {
        let bundle: *mut AnyObject = msg_send![class(c"NSBundle").ok()?, mainBundle];
        let value: *mut AnyObject = msg_send![
            bundle,
            objectForInfoDictionaryKey: &*crate::objc::ns_string(PACKAGE_KEY)
        ];
        let package = string_of(value);
        (!package.is_empty()).then_some(package)
    }
}

type Reports = Arc<Mutex<Box<dyn FrameSender>>>;

fn report(reports: &Reports, message: FromHost) {
    if let Ok(mut sender) = reports.lock() {
        let _ = sender.send(&encode_body(report_to_wire(&message)));
    }
}

/// Sends the windows' input to the service until every sender is gone.
fn serve_input(mut connector: UnixLocalConnector, events: Receiver<InputEvent>) {
    let mut stream: Option<InputStream> = None;
    let mut retry_at: Option<Instant> = None;
    while let Ok(event) = events.recv() {
        for _ in 0..2 {
            if stream.is_none() {
                if retry_at.is_some_and(|at| Instant::now() < at) {
                    break;
                }
                match HostApiClient::connect(&mut connector, BUILD, CONNECT_TIMEOUT)
                    .and_then(|client| client.stream_input())
                {
                    Ok(connected) => {
                        stream = Some(connected);
                        retry_at = None;
                    }
                    Err(error) => {
                        eprintln!("dsa-app-window: input cannot reach Android: {error}");
                        retry_at = Some(Instant::now() + INPUT_RETRY);
                        break;
                    }
                }
            }
            let Some(connected) = stream.as_mut() else {
                break;
            };
            match connected.send_input(&event) {
                Ok(()) => break,
                Err(_) => stream = None,
            }
        }
    }
}

struct Events {
    task_id: i32,
    reports: Reports,
}

impl WindowEvents for Events {
    fn resized(&mut self, pixels: (u32, u32)) {
        report(
            &self.reports,
            FromHost::Resized {
                task_id: self.task_id,
                width: pixels.0,
                height: pixels.1,
            },
        );
    }

    fn close_requested(&mut self) {
        report(
            &self.reports,
            FromHost::Closed {
                task_id: self.task_id,
            },
        );
    }

    fn focused(&mut self, focused: bool) {
        report(
            &self.reports,
            FromHost::Focus {
                task_id: self.task_id,
                focused,
            },
        );
    }

    fn scale_changed(&mut self, scale: f64) {
        report(
            &self.reports,
            FromHost::Dpi {
                task_id: self.task_id,
                dpi: (scale * 96.0).round() as u32,
            },
        );
        // Reported from inside a window call the state is busy with: the window then takes
        // the scale with its next frame instead.
        STATE.with(|state| {
            if let Ok(state) = state.try_borrow() {
                if let Some(window) = state.windows.get(&self.task_id) {
                    window.set_scale(scale);
                }
            }
        });
    }
}

type Cursors = Arc<Mutex<HashMap<i32, ImeCursor>>>;

struct Input {
    task_id: i32,
    events: Sender<InputEvent>,
    cursors: Cursors,
    files: Sender<PathBuf>,
}

impl InputSink for Input {
    fn input(&mut self, event: InputEvent) {
        let _ = self.events.send(event);
    }

    fn files_dropped(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            let _ = self.files.send(path);
        }
    }

    fn ime_cursor(&self) -> Option<ImeCursor> {
        self.cursors.lock().ok()?.get(&self.task_id).copied()
    }
}

#[derive(Default)]
struct State {
    windows: HashMap<i32, TaskWindow>,
    /// Each frame texture, by its ID: the task it is for and its surface.
    textures: HashMap<u64, (i32, Surface)>,
    reports: Option<Reports>,
    input: Option<Sender<InputEvent>>,
    files: Option<Sender<PathBuf>>,
    cursors: Cursors,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn show(task_id: i32, title: &str, size: (u32, u32), focused: bool) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(window) = state.windows.get(&task_id) {
            window.set_title(title);
            if focused {
                window.activate();
            }
            return;
        }
        let (Some(reports), Some(input), Some(files)) = (
            state.reports.clone(),
            state.input.clone(),
            state.files.clone(),
        ) else {
            return;
        };
        let events = Box::new(Events { task_id, reports });
        let sink = Box::new(Input {
            task_id,
            events: input,
            cursors: Arc::clone(&state.cursors),
            files,
        });
        match TaskWindow::open(task_id, title, size, events, sink) {
            Ok(window) => {
                if focused {
                    window.activate();
                }
                state.windows.insert(task_id, window);
            }
            Err(why) => {
                eprintln!("dsa-app-window: the window of task {task_id} did not open: {why}")
            }
        }
    });
}

fn handle(message: ToHost) {
    match message {
        ToHost::Show {
            task_id,
            title,
            width,
            height,
            focused,
        } => show(task_id, &title, (width, height), focused),
        ToHost::Retitle { task_id, title } => STATE.with(|state| {
            if let Some(window) = state.borrow().windows.get(&task_id) {
                window.set_title(&title);
            }
        }),
        ToHost::Resize {
            task_id,
            width,
            height,
        } => STATE.with(|state| {
            if let Some(window) = state.borrow_mut().windows.get_mut(&task_id) {
                window.resize_to((width, height));
            }
        }),
        ToHost::Close { task_id } => STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.windows.remove(&task_id);
            state.textures.retain(|_, (task, _)| *task != task_id);
            if state.windows.is_empty() {
                // The app has no window left: its Dock icon goes with this process.
                quit();
            }
        }),
        ToHost::TaskSize {
            task_id, min, max, ..
        } => STATE.with(|state| {
            if let Some(window) = state.borrow().windows.get(&task_id) {
                window.set_limits(min, max);
            }
        }),
        ToHost::ImeCursor { task_id, cursor } => STATE.with(|state| {
            if let Ok(mut cursors) = state.borrow().cursors.lock() {
                cursors.insert(task_id, cursor);
            }
        }),
        // macOS draws the title bar and its buttons in the system's own colors.
        ToHost::CaptionTone { .. } => {}
        ToHost::Mode {
            task_id,
            mode,
            resizable,
        } => STATE.with(|state| {
            if let Some(window) = state.borrow_mut().windows.get_mut(&task_id) {
                window.set_resizable(resizable);
                window.set_picture_in_picture(mode == WindowMode::PictureInPicture);
                window.set_full_screen(mode == WindowMode::Fullscreen);
            }
        }),
    }
}

fn texture(task_id: i32, texture_id: u64, handle: u64) {
    match u32::try_from(handle)
        .map_err(|_| format!("{handle} is not a frame surface ID"))
        .and_then(Surface::lookup)
    {
        Ok(surface) => STATE.with(|state| {
            state
                .borrow_mut()
                .textures
                .insert(texture_id, (task_id, surface));
        }),
        Err(why) => eprintln!("dsa-app-window: task {task_id}'s frames cannot be shown: {why}"),
    }
}

fn frame(task_id: i32, texture_id: u64, sequence: u64, content: (u32, u32)) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let State {
            windows,
            textures,
            reports,
            ..
        } = &mut *state;
        if let (Some(window), Some((_, surface))) =
            (windows.get_mut(&task_id), textures.get(&texture_id))
        {
            let (width, height) = surface.size();
            let shown = surface.read(|pixels, stride| {
                window.present(FrameRef {
                    pixels,
                    width,
                    height,
                    stride: stride as u32,
                    content,
                })
            });
            match shown {
                Ok(Ok(())) => {}
                Ok(Err(why)) | Err(why) => {
                    eprintln!("dsa-app-window: a frame of task {task_id} was not shown: {why}")
                }
            }
        }
        // Answered whether or not it was shown, so the window manager is never held up.
        if let Some(reports) = reports {
            if let Ok(mut sender) = reports.lock() {
                let _ = sender.send(&encode_body(Body::FramePresented(FramePresented {
                    task_id,
                    sequence,
                })));
            }
        }
    });
}

fn quit() {
    unsafe {
        if let Ok(app_class) = class(c"NSApplication") {
            let app: *mut AnyObject = msg_send![app_class, sharedApplication];
            let _: () = msg_send![app, terminate: std::ptr::null::<AnyObject>()];
        }
    }
}

/// What the window host runs with.
pub struct HostSetup {
    pub package: String,
    pub window_manager: PathBuf,
    pub host_api: PathBuf,
    /// The user started the program (from Launchpad or the Dock), not the window manager.
    pub launch: bool,
}

/// Runs the window host until the window manager lets it go. Must be called on the main
/// thread; it runs the application's run loop.
pub fn run(setup: HostSetup) -> Result<(), String> {
    if !crate::objc::is_main_thread() {
        return Err("the window host must run on the main thread".into());
    }
    let mut connection = UnixLocalConnector {
        path: setup.window_manager.clone(),
    }
    .connect(CONNECT_TIMEOUT)
    .map_err(|e| format!("the window manager is not running: {e}"))?;
    connect_window_host(
        &mut *connection,
        BUILD,
        Attach {
            package_name: setup.package.clone(),
            process_id: std::process::id(),
            launch: setup.launch,
        },
        CONNECT_TIMEOUT,
    )?;
    let sender = connection
        .try_clone_sender()
        .ok_or("the window manager's connection cannot be shared")?;
    let reports: Reports = Arc::new(Mutex::new(sender));

    let (input, input_events) = channel();
    let host_api = UnixLocalConnector {
        path: setup.host_api.clone(),
    };
    std::thread::Builder::new()
        .name("dsa-app-window-input".into())
        .spawn(move || serve_input(host_api, input_events))
        .map_err(|e| format!("the window input could not start: {e}"))?;

    // Files dropped on a window are opened in the app through the local API.
    let (files, dropped) = channel::<PathBuf>();
    let open_with = setup.host_api.clone();
    let package = setup.package.clone();
    std::thread::Builder::new()
        .name("dsa-app-window-files".into())
        .spawn(move || {
            for path in dropped {
                let mut connector = UnixLocalConnector {
                    path: open_with.clone(),
                };
                // Dropped on the app's window: shared with that app, as on every host.
                let opened = FileTransfer::read(&path, FileAction::Send).and_then(|mut file| {
                    file.package = package.clone();
                    HostApiClient::connect(&mut connector, BUILD, CONNECT_TIMEOUT)
                        .and_then(|client| client.open_file(&file))
                        .map_err(|e| e.to_string())
                });
                if let Err(why) = opened {
                    eprintln!("dsa-app-window: {} did not open: {why}", path.display());
                }
            }
        })
        .map_err(|e| format!("the file drop could not start: {e}"))?;

    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.reports = Some(reports);
        state.input = Some(input);
        state.files = Some(files);
    });

    std::thread::Builder::new()
        .name("dsa-app-window-messages".into())
        .spawn(move || loop {
            let frame = match connection.recv(None) {
                Ok(frame) => frame,
                Err(_) => {
                    // The window manager went away: so do the windows.
                    on_main(quit);
                    return;
                }
            };
            let Ok(envelope) = decode(&frame) else {
                continue;
            };
            let Some(body) = envelope.body else { continue };
            match body {
                Body::FrameTexture(t) => {
                    on_main(move || texture(t.task_id, t.texture_id, t.handle))
                }
                Body::Frame(f) => {
                    on_main(move || frame(f.task_id, f.texture_id, f.sequence, (f.width, f.height)))
                }
                body => {
                    if let Some(message) = to_host_from_wire(&body) {
                        on_main(move || handle(message));
                    }
                }
            }
        })
        .map_err(|e| format!("the window manager's messages could not be read: {e}"))?;

    unsafe {
        let app_class = class(c"NSApplication")?;
        let app: *mut AnyObject = msg_send![app_class, sharedApplication];
        // A regular app: in the Dock, with a menu bar, taking the keyboard.
        let _: bool = msg_send![app, setActivationPolicy: 0isize];
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
        let _: () = msg_send![app, run];
    }
    Ok(())
}
