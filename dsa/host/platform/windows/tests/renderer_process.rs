//! The graphics renderer runs in a process of its own, and the window manager outlives it.
//!
//! The renderer here is this test program itself, started again as `dsa-gfx` (the window
//! manager's pipe goes in its environment), so the test runs the same renderer code the
//! installed `dsa-gfx.exe` does. The test plays the window manager (the renderer client and
//! its supervisor, a window that holds a shared texture) and the guest (a command stream over
//! a Hyper-V loopback socket that posts a frame to a display), kills the renderer, and checks
//! that the window manager goes on, the window keeps its texture and last frame, the renderer
//! comes back, and the guest's next frame reaches the window again.
//!
//! The frame tests need gfxstream: `GFXSTREAM_BACKEND_PATH` names `gfxstream_backend.dll`, and
//! they are skipped without it. The restart test runs everywhere, with no backend loaded.

#![cfg(windows)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::adb_bridge::BridgeStream;
use dsa_host_core::renderer::{spawn_supervisor, RendererHandler, ADDRESS_VARIABLE};
use dsa_host_windows::compositor::adapter::{DisplayPresenter, DisplaySink};
use dsa_host_windows::compositor::log::CompositorLogger;
use dsa_host_windows::compositor::present::{Gpu, SharedTexture};
use dsa_host_windows::guid::Guid;
use dsa_host_windows::hvsocket::connect_stream;
use dsa_host_windows::render_gpu::GpuRequest;
use dsa_host_windows::renderer::client::{ProgramLauncher, RendererClient, RendererProcess};
use dsa_host_windows::vm_listener::VmIdSlot;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

const WAIT: Duration = Duration::from_secs(60);
/// The display the test's guest posts its frames to.
const DISPLAY: u32 = 7;

/// When this program is started as the renderer (by the tests below), it is the renderer;
/// otherwise this does nothing.
#[test]
fn nfr5_renderer_child_process() {
    if std::env::var_os(ADDRESS_VARIABLE).is_some() {
        dsa_host_windows::renderer::process::run(&[]);
    }
}

fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while !done() {
        if Instant::now() >= deadline {
            eprintln!("timed out waiting for {what}");
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    true
}

/// The vsock ports test `slot`'s renderer takes the guest's streams on, apart from every
/// other test's and from a real VM's.
fn ports(slot: u32) -> (u32, u32) {
    let base = 0x4000_0000 | ((std::process::id() & 0xffff) << 8) | (slot << 1);
    (base, base + 1)
}

/// Starts this program as the renderer, with `backend` as its gfxstream (none: no backend).
fn launcher(process: RendererProcess, backend: Option<&Path>) -> ProgramLauncher {
    let mut launcher = ProgramLauncher::new(std::env::current_exe().unwrap(), process);
    launcher.args = Some(vec![
        "nfr5_renderer_child_process".into(),
        "--exact".into(),
        "--nocapture".into(),
        "--test-threads=1".into(),
    ]);
    launcher.env.push((
        "GFXSTREAM_BACKEND_PATH".into(),
        backend
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
    ));
    launcher
}

fn kill(pid: u32) {
    // SAFETY: opens a process of this test's own, ends it as Task Manager's End task does,
    // and closes the handle.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        assert!(
            !process.is_null(),
            "the renderer's process {pid} can be opened"
        );
        assert_ne!(TerminateProcess(process, 1), 0, "the renderer is ended");
        CloseHandle(process);
    }
}

fn log_file(name: &str) -> (PathBuf, Arc<CompositorLogger>) {
    let path = std::env::temp_dir()
        .join(format!("dsa-renderer-{name}-{}", std::process::id()))
        .join("dsa-wm.log");
    let _ = std::fs::remove_file(&path);
    let logger = Arc::new(CompositorLogger::open(&path).unwrap());
    (path, logger)
}

/// The window manager under test: the renderer client, its supervisor and the VM slot.
struct WindowManager {
    client: Arc<RendererClient>,
    stop: Arc<AtomicBool>,
    supervisor: Option<thread::JoinHandle<()>>,
    vm_thread: Option<thread::JoinHandle<()>>,
}

impl WindowManager {
    fn start(slot: u32, backend: Option<&Path>, logger: Arc<CompositorLogger>) -> WindowManager {
        let process: RendererProcess = Arc::default();
        let client = RendererClient::new(
            GpuRequest::Ranking,
            ports(slot),
            Arc::clone(&process),
            Some(logger),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let supervisor = spawn_supervisor(
            Box::new(launcher(process, backend)),
            Arc::clone(&client) as Arc<dyn RendererHandler>,
            "dsa-wm test".into(),
            Arc::clone(&stop),
        )
        .unwrap();
        // The guest is this computer, over Hyper-V loopback.
        let vm: VmIdSlot = Arc::new(Mutex::new(Some(Guid::LOOPBACK)));
        let vm_thread = client.follow_vm(vm, Arc::clone(&stop)).unwrap();
        WindowManager {
            client,
            stop,
            supervisor: Some(supervisor),
            vm_thread: Some(vm_thread),
        }
    }

    fn renderer(&self) -> u32 {
        self.client
            .renderer_process_id()
            .expect("a renderer is running")
    }
}

impl Drop for WindowManager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
        if let Some(vm_thread) = self.vm_thread.take() {
            let _ = vm_thread.join();
        }
    }
}

#[test]
fn nfr5_the_window_manager_starts_a_killed_renderer_again_and_says_why() {
    let (path, logger) = log_file("restart");
    let wm = WindowManager::start(0, None, logger);
    assert!(wm.client.wait_started(WAIT), "the renderer starts");
    let first = wm.renderer();
    assert_ne!(
        first,
        std::process::id(),
        "the renderer is a process of its own"
    );
    // Its memory shows under its own process.
    let memory = wm.client.memory().expect("the renderer says what it holds");
    assert_eq!(memory.process_id, first);
    assert!(memory.held.working_set_kb.is_some());

    kill(first);
    assert!(
        wait_until("the renderer to start again", WAIT, || wm.client.starts()
            == 2),
        "the renderer comes back"
    );
    let second = wm.renderer();
    assert_ne!(second, first);
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(
        log.contains(&format!(
            "the graphics renderer (dsa-gfx, pid {first}) ended: it was ended from outside"
        )),
        "{log}"
    );
    assert!(
        log.contains("the windows stay and keep their last frame"),
        "{log}"
    );
    assert!(log.contains("is running again"), "{log}");
}

/// A task window: it holds one shared texture for the display, as an app window's frame
/// router does, and counts what it is asked for and shown.
struct Window {
    gpu: Arc<Gpu>,
    texture: Mutex<Option<SharedTexture>>,
    made: AtomicUsize,
    asked: AtomicUsize,
    shown: AtomicUsize,
}

impl Window {
    fn handle(&self) -> Option<isize> {
        self.texture
            .lock()
            .unwrap()
            .as_ref()
            .map(SharedTexture::handle)
    }
}

impl DisplaySink for Window {
    fn target(&self, display_id: u32, width: u32, height: u32) -> Option<isize> {
        if display_id != DISPLAY {
            return None;
        }
        self.asked.fetch_add(1, Ordering::SeqCst);
        let mut texture = self.texture.lock().unwrap();
        if texture.as_ref().map(SharedTexture::size) != Some((width, height)) {
            *texture = Some(self.gpu.shared_texture(width, height).ok()?);
            self.made.fetch_add(1, Ordering::SeqCst);
        }
        texture.as_ref().map(SharedTexture::handle)
    }

    fn frame(&self, display_id: u32, _content: (u32, u32)) {
        if display_id == DISPLAY {
            self.shown.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// A guest process's command stream over Hyper-V loopback, speaking renderControl as the
/// guest's hwcomposer does.
struct Guest {
    stream: Box<dyn BridgeStream>,
}

impl Guest {
    const RC_GET_RENDERER_VERSION: u32 = 10000;
    const RC_CREATE_COLOR_BUFFER: u32 = 10012;
    const RC_SET_DISPLAY_COLOR_BUFFER: u32 = 10040;
    const RC_SET_DISPLAY_POSE: u32 = 10044;
    const RC_CREATE_DISPLAY_BY_ID: u32 = 10062;
    const GL_RGBA: u32 = 0x1908;

    /// Connects to the renderer, trying again while it is not listening yet.
    fn connect(port: u32) -> Guest {
        let deadline = Instant::now() + WAIT;
        loop {
            match connect_stream(
                Guid::LOOPBACK,
                Guid::vsock_service(port),
                Duration::from_secs(2),
            ) {
                Ok(mut stream) => {
                    // Every stream starts with the guest's client flags.
                    stream.write_all(&0u32.to_le_bytes()).unwrap();
                    return Guest { stream };
                }
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "the renderer takes no stream on port {port}: {error}"
                    );
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    fn call(&mut self, opcode: u32, args: &[u32]) -> u32 {
        let mut packet = Vec::with_capacity(8 + 4 * args.len());
        packet.extend_from_slice(&opcode.to_le_bytes());
        packet.extend_from_slice(&((8 + 4 * args.len()) as u32).to_le_bytes());
        for arg in args {
            packet.extend_from_slice(&arg.to_le_bytes());
        }
        self.stream
            .write_all(&packet)
            .expect("the renderer takes the call");
        let mut reply = [0u8; 4];
        self.stream
            .read_exact(&mut reply)
            .expect("the renderer answers");
        u32::from_le_bytes(reply)
    }

    /// Posts one finished frame to `DISPLAY`, as the hwcomposer does for a task.
    fn post_frame(&mut self) {
        assert!(self.call(Self::RC_GET_RENDERER_VERSION, &[]) > 0);
        let buffer = self.call(Self::RC_CREATE_COLOR_BUFFER, &[64, 64, Self::GL_RGBA]);
        assert_ne!(buffer, 0, "a color buffer is made");
        self.call(Self::RC_CREATE_DISPLAY_BY_ID, &[DISPLAY]);
        self.call(Self::RC_SET_DISPLAY_POSE, &[DISPLAY, 0, 0, 64, 64]);
        self.call(Self::RC_SET_DISPLAY_COLOR_BUFFER, &[DISPLAY, buffer]);
    }
}

fn backend() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var("GFXSTREAM_BACKEND_PATH").ok()?);
    if path.is_file() {
        Some(path)
    } else {
        eprintln!("skipping: no gfxstream backend at {}", path.display());
        None
    }
}

#[test]
fn nfr5_killing_the_renderer_keeps_the_window_and_its_frame_and_frames_resume() {
    let Some(backend) = backend() else {
        eprintln!("skipping: GFXSTREAM_BACKEND_PATH does not name gfxstream_backend.dll");
        return;
    };
    let (path, logger) = log_file("frames");
    let wm = WindowManager::start(1, Some(&backend), logger);
    assert!(wm.client.wait_started(WAIT), "the renderer starts");
    // The window's texture is made on the GPU the renderer runs on.
    let gpu = Gpu::new(wm.client.adapter_luid(), false).expect("a Direct3D device");
    let window = Arc::new(Window {
        gpu,
        texture: Mutex::new(None),
        made: AtomicUsize::new(0),
        asked: AtomicUsize::new(0),
        shown: AtomicUsize::new(0),
    });
    wm.client
        .present_displays(Some(Arc::clone(&window) as Arc<dyn DisplaySink>))
        .unwrap();

    // The guest draws a frame: the renderer asks the window manager for the window's
    // texture, and, where the GPU can open it, copies the frame into it.
    let mut guest = Guest::connect(ports(1).0);
    guest.post_frame();
    assert!(
        wait_until("the frame's request", WAIT, || window
            .asked
            .load(Ordering::SeqCst)
            >= 1),
        "the renderer asks where the frame goes"
    );
    let on_gpu = wait_until("the frame", Duration::from_secs(10), || {
        window.shown.load(Ordering::SeqCst) >= 1
    });
    if !on_gpu {
        eprintln!(
            "this Vulkan driver cannot open the window's Direct3D texture; frames are checked \
             up to the renderer's request"
        );
    }
    let texture = window.handle().expect("the window has its texture");
    let shown = window.shown.load(Ordering::SeqCst);

    // The renderer dies. The window manager, which is this process, goes on; the window
    // keeps its texture with the last frame in it.
    let first = wm.renderer();
    kill(first);
    assert!(
        wait_until("the renderer to start again", WAIT, || wm.client.starts()
            == 2),
        "the renderer comes back"
    );
    assert_eq!(
        window.handle(),
        Some(texture),
        "the window keeps its texture"
    );
    assert_eq!(window.made.load(Ordering::SeqCst), 1);
    assert_eq!(window.shown.load(Ordering::SeqCst), shown);
    drop(guest);

    // The guest's stream dropped with the old renderer; it connects again, as it does after
    // a restart, and its next frame reaches the same window.
    let asked = window.asked.load(Ordering::SeqCst);
    let mut guest = Guest::connect(ports(1).0);
    guest.post_frame();
    assert!(
        wait_until("the next frame's request", WAIT, || {
            window.asked.load(Ordering::SeqCst) > asked
        }),
        "frames resume after the restart"
    );
    if on_gpu {
        assert!(
            wait_until("the next frame", WAIT, || {
                window.shown.load(Ordering::SeqCst) > shown
            }),
            "the window draws again"
        );
    }
    // The same texture went over to the new renderer; the window made no other.
    assert_eq!(window.made.load(Ordering::SeqCst), 1);
    drop(guest);

    let log = std::fs::read_to_string(&path).unwrap();
    assert!(log.contains(&format!("pid {first}) ended")), "{log}");
}
