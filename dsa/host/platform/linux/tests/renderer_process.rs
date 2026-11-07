//! The graphics renderer runs in a process of its own (`dsa-gfx`), and the compositor
//! outlives it.
//!
//! The renderer here is this test program itself, started again as `dsa-gfx` with its end of
//! the socket pair at descriptor 3, so the test runs the same renderer code the installed
//! `dsa-gfx` does. The test plays the compositor (the renderer client, its supervisor and the
//! frames the windows draw from) and the guest (a process pipe and a command stream on the
//! Unix sockets a virtual machine monitor proxies vsock to), kills the renderer, and checks
//! that the compositor goes on, the window keeps its last frame, the renderer comes back, the
//! guest registers its process again and makes its gralloc buffer's ColorBuffer again under
//! the same handle and size, and its next frame reaches the window.
//!
//! The frame test needs gfxstream: `GFXSTREAM_BACKEND_PATH` names `libgfxstream_backend.so`,
//! and it is skipped without it. The restart test runs everywhere, with no backend loaded.

#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::renderer::{spawn_supervisor, RendererHandler, ADDRESS_VARIABLE};
use dsa_host_linux::compositor::frames::{LatestFrames, Shown};
use dsa_host_linux::compositor::gfxstream::{FrameSink, GpuImages};
use dsa_host_linux::compositor::wayland::Wake;
use dsa_host_linux::renderer::client::RendererClient;
use dsa_host_unix::renderer_launch::{CurrentFds, UnixLauncher};

const WAIT: Duration = Duration::from_secs(60);
const DISPLAY: u32 = 7;

/// When this program is started as the renderer (by the tests below), it is the renderer;
/// otherwise this does nothing.
#[test]
fn nfr5_renderer_child_process() {
    if std::env::var_os(ADDRESS_VARIABLE).is_some() {
        dsa_host_linux::renderer::process::run(&[]);
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

struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// The compositor under test.
struct Compositor {
    client: Arc<RendererClient>,
    frames: Arc<Mutex<LatestFrames>>,
    images: GpuImages,
    log: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    supervisor: Option<thread::JoinHandle<()>>,
    sockets: PathBuf,
}

impl Compositor {
    fn start(name: &str, backend: Option<&Path>) -> Compositor {
        let sockets = std::env::temp_dir().join(format!("dsa-gfx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sockets);
        std::fs::create_dir_all(&sockets).unwrap();
        let frames = Arc::new(Mutex::new(LatestFrames::new()));
        let images: GpuImages = Arc::default();
        let sink = FrameSink {
            frames: Arc::clone(&frames),
            waker: Arc::new(Wakes(AtomicUsize::new(0))),
            images: Some(Arc::clone(&images)),
        };
        let log = Arc::new(Mutex::new(Vec::new()));
        let lines = Arc::clone(&log);
        let fds: CurrentFds = Arc::default();
        let client = RendererClient::new(
            sink,
            Arc::clone(&fds),
            String::new(),
            Box::new(move |line: &str| lines.lock().unwrap().push(line.to_string())),
        );
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
        launcher.env.push((
            "DSA_GRAPHICS_SOCKET_DIR".into(),
            sockets.display().to_string(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let supervisor = spawn_supervisor(
            Box::new(launcher),
            Arc::clone(&client) as Arc<dyn RendererHandler>,
            "dsa-compositor test".into(),
            Arc::clone(&stop),
        )
        .unwrap();
        Compositor {
            client,
            frames,
            images,
            log,
            stop,
            supervisor: Some(supervisor),
            sockets,
        }
    }

    fn renderer(&self) -> u32 {
        self.client.renderer_process_id().expect("a renderer runs")
    }

    fn log(&self) -> String {
        self.log.lock().unwrap().join("\n")
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
        let _ = std::fs::remove_dir_all(&self.sockets);
    }
}

fn kill(pid: u32) {
    // SAFETY: ends a process this test started.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
}

#[test]
fn nfr5_the_compositor_starts_a_killed_renderer_again_and_says_why() {
    let compositor = Compositor::start("restart", None);
    assert!(wait_until(WAIT, || compositor.client.starts() == 1));
    let first = compositor.renderer();
    assert_ne!(
        first,
        std::process::id(),
        "the renderer is a process of its own"
    );
    let memory = compositor
        .client
        .memory()
        .expect("the renderer says what it holds");
    assert_eq!(memory.process_id, first);

    kill(first);
    assert!(
        wait_until(WAIT, || compositor.client.starts() == 2),
        "the renderer comes back: {}",
        compositor.log()
    );
    assert_ne!(compositor.renderer(), first);
    let log = compositor.log();
    assert!(
        log.contains(&format!(
            "the graphics renderer (dsa-gfx, pid {first}) ended: it was killed (SIGKILL)"
        )),
        "{log}"
    );
    assert!(log.contains("is running again"), "{log}");
}

/// One guest process on the renderer's Unix sockets: its process pipe and a command stream,
/// as in the Windows test.
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

    fn connect(sockets: &Path) -> Guest {
        let mut pipe = Guest::open(&sockets.join("gfx-5567.sock"));
        pipe.write_all(&100i32.to_le_bytes()).unwrap();
        let mut puid = [0u8; 8];
        pipe.read_exact(&mut puid).expect("the process gets its ID");
        let mut stream = Guest::open(&sockets.join("gfx-5566.sock"));
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

/// The window of `DISPLAY`: what it would draw next, taken as the compositor's main loop
/// takes it.
fn take_frame(compositor: &Compositor, last: &mut Option<Shown>) -> bool {
    let mut frames = compositor.frames.lock().unwrap().take();
    match frames.remove(&DISPLAY) {
        Some(frame) => {
            *last = Some(frame);
            true
        }
        None => false,
    }
}

#[test]
fn nfr5_killing_the_renderer_keeps_the_window_and_its_frame_and_frames_resume() {
    let Some(backend) = std::env::var_os("GFXSTREAM_BACKEND_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    else {
        eprintln!("skipping: GFXSTREAM_BACKEND_PATH does not name libgfxstream_backend.so");
        return;
    };
    let compositor = Compositor::start("frames", Some(&backend));
    assert!(wait_until(WAIT, || compositor.client.starts() == 1));

    let mut last = None;
    let mut guest = Guest::connect(&compositor.sockets);
    guest.post_frame();
    assert!(
        wait_until(WAIT, || take_frame(&compositor, &mut last)),
        "the guest's frame reaches the window: {}",
        compositor.log()
    );
    let images = compositor.images.lock().unwrap().len();
    let first_image_fd = compositor
        .images
        .lock()
        .unwrap()
        .first()
        .map(|(_, fd)| fd.as_raw_fd());

    let first = compositor.renderer();
    kill(first);
    assert!(
        wait_until(WAIT, || compositor.client.starts() == 2),
        "the renderer comes back: {}",
        compositor.log()
    );
    // The window keeps its last frame: the frame it holds, and the dmabufs its buffers were
    // made from, outlive the renderer that made them.
    assert!(last.is_some());
    if let Some(fd) = first_image_fd {
        // SAFETY: a descriptor the compositor's images hold.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::fstat(fd, &mut stat) }, 0, "the dmabuf stays");
    }
    drop(guest);

    // The guest registers its process again, makes its ColorBuffer again under the same
    // handle and size, and its next frame reaches the window.
    let mut guest = Guest::connect(&compositor.sockets);
    guest.post_frame();
    assert!(
        wait_until(WAIT, || take_frame(&compositor, &mut last)),
        "frames resume after the restart: {}",
        compositor.log()
    );
    if images > 0 {
        // On the GPU path the new renderer exports its own images for the display.
        assert!(compositor.images.lock().unwrap().len() > images);
    }
    let log = compositor.log();
    assert!(log.contains(&format!("pid {first}) ended")), "{log}");
}
