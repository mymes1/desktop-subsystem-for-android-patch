//! The graphics renderer runs in a process of its own (`dsa-gfx`), and the window manager
//! outlives it.
//!
//! The renderer here is this test program itself, started again as `dsa-gfx` with its end of
//! the socket pair at descriptor 3. The test plays the window manager (the supervisor, and a
//! window that hands the renderer a global `IOSurface` for its display, as the window manager
//! does without its launchd Mach service) and the guest (a process pipe and a command stream
//! on the relay sockets), kills the renderer, and checks that the window manager goes on, the
//! window keeps its surface and last frame, the renderer comes back, the guest registers its
//! process again and makes its gralloc buffer's ColorBuffer again under the same handle and
//! size, and its next frame reaches the window.
//!
//! The frame test needs gfxstream: `GFXSTREAM_BACKEND_PATH` names
//! `libgfxstream_backend.dylib`, and it is skipped without it. The restart test runs
//! everywhere, with no backend loaded.

#![cfg(target_os = "macos")]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::renderer::{
    spawn_supervisor, Given, RendererHandler, RendererLink, TextureBook, ADDRESS_VARIABLE,
};
use dsa_host_macos::compositor::surface::Surface;
use dsa_host_unix::renderer_launch::{CurrentFds, UnixLauncher};
use dsa_protocol::renderer::types::{envelope::Body, Start, Started, Target};

const WAIT: Duration = Duration::from_secs(60);
const DISPLAY: u32 = 7;

/// When this program is started as the renderer (by the tests below), it is the renderer;
/// otherwise this does nothing.
#[test]
fn nfr5_renderer_child_process() {
    if std::env::var_os(ADDRESS_VARIABLE).is_some() {
        dsa_host_macos::gfx::process::run(&[]);
    }
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while !done() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    true
}

/// The window manager under test: one window for `DISPLAY`, with a global surface.
struct Window {
    sockets: PathBuf,
    surface: Mutex<Option<Surface>>,
    made: AtomicUsize,
    book: Mutex<TextureBook<usize>>,
    asked: AtomicUsize,
    shown: AtomicUsize,
    starts: AtomicU64,
    pid: Mutex<Option<u32>>,
    log: Mutex<Vec<String>>,
}

impl Window {
    fn socket(&self, port: u32) -> PathBuf {
        self.sockets.join(format!("{port}.sock"))
    }
}

impl RendererHandler for Window {
    fn start(&self) -> Start {
        Start {
            display_width: 64,
            display_height: 64,
            command_socket: self.socket(5566).display().to_string(),
            process_pipe_socket: self.socket(5567).display().to_string(),
            frames_on_gpu: true,
            ..Start::default()
        }
    }

    fn connected(&self, started: &Started, link: &RendererLink) {
        for note in &started.notes {
            self.log(note);
        }
        self.book.lock().unwrap().clear();
        *self.pid.lock().unwrap() = Some(link.process_id());
        self.starts.fetch_add(1, Ordering::SeqCst);
    }

    fn message(&self, body: Body, link: &RendererLink) {
        match body {
            Body::TargetRequest(request) => {
                let mut target = Target {
                    request: request.request,
                    drop: true,
                    width: request.width,
                    height: request.height,
                    ..Target::default()
                };
                if request.display_id == DISPLAY {
                    self.asked.fetch_add(1, Ordering::SeqCst);
                    let mut surface = self.surface.lock().unwrap();
                    if surface.as_ref().map(Surface::size) != Some((request.width, request.height))
                    {
                        *surface = Surface::create(request.width, request.height, true).ok();
                        self.made.fetch_add(1, Ordering::SeqCst);
                    }
                    if let Some(surface) = surface.as_ref() {
                        let given = self.book.lock().unwrap().give(
                            DISPLAY,
                            Some(surface.as_raw() as usize),
                            &request.held_textures,
                        );
                        match given {
                            Given::Same(id) => {
                                target.drop = false;
                                target.texture_id = id;
                            }
                            Given::New(id) => {
                                target.drop = false;
                                target.texture_id = id;
                                target.handle = u64::from(surface.id());
                            }
                            Given::Drop => {}
                        }
                    }
                }
                link.send(Body::Target(target));
            }
            Body::FrameDone(done) if done.display_id == DISPLAY => {
                self.shown.fetch_add(1, Ordering::SeqCst);
            }
            Body::Note(note) => self.log(&note.text),
            _ => {}
        }
    }

    fn disconnected(&self) {
        *self.pid.lock().unwrap() = None;
    }

    fn log(&self, line: &str) {
        self.log.lock().unwrap().push(line.to_string());
    }
}

struct WindowManager {
    window: Arc<Window>,
    stop: Arc<AtomicBool>,
    supervisor: Option<thread::JoinHandle<()>>,
}

impl WindowManager {
    fn start(name: &str, backend: Option<&Path>) -> WindowManager {
        let sockets = std::env::temp_dir().join(format!("dsa-gfx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sockets);
        std::fs::create_dir_all(&sockets).unwrap();
        let window = Arc::new(Window {
            sockets,
            surface: Mutex::new(None),
            made: AtomicUsize::new(0),
            book: Mutex::new(TextureBook::new()),
            asked: AtomicUsize::new(0),
            shown: AtomicUsize::new(0),
            starts: AtomicU64::new(0),
            pid: Mutex::new(None),
            log: Mutex::new(Vec::new()),
        });
        let fds: CurrentFds = Arc::default();
        let mut launcher = UnixLauncher::new(std::env::current_exe().unwrap(), fds);
        launcher.args = Some(vec![
            "nfr5_renderer_child_process".into(),
            "--exact".into(),
            "--nocapture".into(),
            "--test-threads=1".into(),
        ]);
        launcher.env.push((
            "GFXSTREAM_BACKEND_PATH".into(),
            backend.map(|p| p.display().to_string()).unwrap_or_default(),
        ));
        // Without a backend, nothing at all is loaded.
        if backend.is_none() {
            launcher.env.push((
                "DSA_RENDERER".into(),
                "/nonexistent/libgfxstream_backend.dylib".into(),
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let supervisor = spawn_supervisor(
            Box::new(launcher),
            Arc::clone(&window) as Arc<dyn RendererHandler>,
            "dsa-wm test".into(),
            Arc::clone(&stop),
        )
        .unwrap();
        WindowManager {
            window,
            stop,
            supervisor: Some(supervisor),
        }
    }

    fn starts(&self) -> u64 {
        self.window.starts.load(Ordering::SeqCst)
    }

    fn renderer(&self) -> u32 {
        self.window.pid.lock().unwrap().expect("a renderer runs")
    }

    fn log(&self) -> String {
        self.window.log.lock().unwrap().join("\n")
    }
}

impl Drop for WindowManager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
        let _ = std::fs::remove_dir_all(&self.window.sockets);
    }
}

fn kill(pid: u32) {
    // SAFETY: ends a process this test started.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
}

#[test]
fn nfr5_the_window_manager_starts_a_killed_renderer_again_and_says_why() {
    let wm = WindowManager::start("restart", None);
    assert!(wait_until(WAIT, || wm.starts() == 1), "{}", wm.log());
    let first = wm.renderer();
    assert_ne!(first, std::process::id());
    kill(first);
    assert!(wait_until(WAIT, || wm.starts() == 2), "{}", wm.log());
    assert_ne!(wm.renderer(), first);
    let log = wm.log();
    assert!(
        log.contains(&format!(
            "the graphics renderer (dsa-gfx, pid {first}) ended: it was killed (SIGKILL)"
        )),
        "{log}"
    );
}

/// One guest process on the relay sockets, as in the Windows and Linux tests.
struct Guest {
    _pipe: UnixStream,
    stream: UnixStream,
}

impl Guest {
    const RC_GET_RENDERER_VERSION: u32 = 10000;
    const RC_OPEN_COLOR_BUFFER_2: u32 = 10025;
    const RC_SET_PUID: u32 = 10033;
    const RC_SET_DISPLAY_COLOR_BUFFER: u32 = 10040;
    const RC_SET_DISPLAY_POSE: u32 = 10044;
    const RC_CREATE_COLOR_BUFFER_WITH_HANDLE: u32 = 10048;
    const RC_CREATE_DISPLAY_BY_ID: u32 = 10062;
    const GL_RGBA: u32 = 0x1908;
    const BUFFER: u32 = 0x8000_0007;

    fn open(socket: &Path) -> UnixStream {
        let deadline = Instant::now() + WAIT;
        loop {
            match UnixStream::connect(socket) {
                Ok(stream) => return stream,
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "the renderer takes no stream on {}: {error}",
                        socket.display()
                    );
                    thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    fn connect(window: &Window) -> Guest {
        let mut pipe = Guest::open(&window.socket(5567));
        pipe.write_all(&100i32.to_le_bytes()).unwrap();
        let mut puid = [0u8; 8];
        pipe.read_exact(&mut puid).expect("the process gets its ID");
        let mut stream = Guest::open(&window.socket(5566));
        stream.write_all(&0u32.to_le_bytes()).unwrap();
        let mut guest = Guest {
            _pipe: pipe,
            stream,
        };
        let id = u64::from_le_bytes(puid);
        guest.send(Self::RC_SET_PUID, &[id as u32, (id >> 32) as u32]);
        guest
    }

    fn send(&mut self, opcode: u32, args: &[u32]) {
        let mut packet = Vec::with_capacity(8 + 4 * args.len());
        packet.extend_from_slice(&opcode.to_le_bytes());
        packet.extend_from_slice(&((8 + 4 * args.len()) as u32).to_le_bytes());
        for arg in args {
            packet.extend_from_slice(&arg.to_le_bytes());
        }
        self.stream
            .write_all(&packet)
            .expect("the renderer takes the call");
    }

    fn call(&mut self, opcode: u32, args: &[u32]) -> u32 {
        self.send(opcode, args);
        let mut reply = [0u8; 4];
        self.stream
            .read_exact(&mut reply)
            .expect("the renderer answers");
        u32::from_le_bytes(reply)
    }

    fn post_frame(&mut self) {
        assert!(self.call(Self::RC_GET_RENDERER_VERSION, &[]) > 0);
        if self.call(Self::RC_OPEN_COLOR_BUFFER_2, &[Self::BUFFER]) != 0 {
            self.send(
                Self::RC_CREATE_COLOR_BUFFER_WITH_HANDLE,
                &[64, 64, Self::GL_RGBA, Self::BUFFER],
            );
            assert_eq!(
                self.call(Self::RC_OPEN_COLOR_BUFFER_2, &[Self::BUFFER]),
                0,
                "the renderer makes the ColorBuffer again under the guest's handle"
            );
        }
        self.call(Self::RC_CREATE_DISPLAY_BY_ID, &[DISPLAY]);
        self.call(Self::RC_SET_DISPLAY_POSE, &[DISPLAY, 0, 0, 64, 64]);
        self.call(Self::RC_SET_DISPLAY_COLOR_BUFFER, &[DISPLAY, Self::BUFFER]);
    }
}

#[test]
fn nfr5_killing_the_renderer_keeps_the_window_and_its_frame_and_frames_resume() {
    let Some(backend) = std::env::var_os("GFXSTREAM_BACKEND_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    else {
        eprintln!("skipping: GFXSTREAM_BACKEND_PATH does not name libgfxstream_backend.dylib");
        return;
    };
    let wm = WindowManager::start("frames", Some(&backend));
    assert!(wait_until(WAIT, || wm.starts() == 1), "{}", wm.log());
    let window = Arc::clone(&wm.window);

    let mut guest = Guest::connect(&window);
    guest.post_frame();
    assert!(
        wait_until(WAIT, || window.shown.load(Ordering::SeqCst) >= 1),
        "the guest's frame reaches the window: {}",
        wm.log()
    );
    let surface = window.surface.lock().unwrap().as_ref().map(Surface::id);

    let first = wm.renderer();
    kill(first);
    assert!(wait_until(WAIT, || wm.starts() == 2), "{}", wm.log());
    // The window keeps its surface, with the last frame in it.
    assert_eq!(
        window.surface.lock().unwrap().as_ref().map(Surface::id),
        surface
    );
    assert_eq!(window.made.load(Ordering::SeqCst), 1);
    drop(guest);

    let shown = window.shown.load(Ordering::SeqCst);
    let mut guest = Guest::connect(&window);
    guest.post_frame();
    assert!(
        wait_until(WAIT, || window.shown.load(Ordering::SeqCst) > shown),
        "frames resume after the restart: {}",
        wm.log()
    );
    assert_eq!(window.made.load(Ordering::SeqCst), 1, "the same surface");
    drop(guest);
}
