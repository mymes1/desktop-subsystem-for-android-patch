//! The window manager keeps its graphics renderer process running: it starts it, checks its
//! handshake, hands it its start, answers its frames' requests, and starts it again when it
//! ends, saying why in the log. The renderer here is a thread on the in-memory transport,
//! standing in for `dsa-gfx`.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::renderer::{
    describe_exit_code, describe_signal, open_renderer, restart_line, spawn_supervisor, Given,
    HeldTargets, Launched, Pending, RendererChild, RendererHandler, RendererLauncher, RendererLink,
    TextureBook, WindowManagerLink,
};
use dsa_protocol::renderer::types::{envelope::Body, FrameDone, Start, Started, Target};
use dsa_protocol::renderer::{decode_body, send};
use dsa_transport::{memory, Connector, Listener, TransportError};

const WAIT: Duration = Duration::from_secs(10);

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// One fake renderer: whether it was told to end, and whether it has.
#[derive(Default)]
struct Life {
    killed: AtomicBool,
    exited: AtomicBool,
    stopped_by_window_manager: AtomicBool,
    targets: Mutex<Vec<Target>>,
}

struct FakeChild {
    pid: u32,
    life: Arc<Life>,
}

impl RendererChild for FakeChild {
    fn id(&self) -> u32 {
        self.pid
    }
    fn exited(&mut self) -> Option<String> {
        self.life
            .exited
            .load(Ordering::SeqCst)
            .then(|| describe_signal(9))
    }
    fn kill(&mut self) {
        self.life.killed.store(true, Ordering::SeqCst);
    }
}

/// Starts fake renderers on threads, each of which asks for one frame's texture and then
/// serves until it is killed.
#[derive(Default)]
struct FakeLauncher {
    lives: Arc<Mutex<Vec<Arc<Life>>>>,
    next_pid: Arc<AtomicU32>,
}

fn fake_renderer(connector: &mut dyn Connector, life: Arc<Life>, pid: u32) {
    let Ok(mut connection) = connector.connect(WAIT) else {
        life.exited.store(true, Ordering::SeqCst);
        return;
    };
    let start = open_renderer(&mut *connection, "fake dsa-gfx").expect("the start");
    assert_eq!(start.display_width, 1280);
    let link = WindowManagerLink::new(connection.try_clone_sender().expect("a sender"));
    send(
        &mut *connection,
        Body::Started(Started {
            process_id: pid,
            ..Started::default()
        }),
    )
    .unwrap();
    // The presentation thread: one frame for display 3.
    let asking = Arc::clone(&link);
    let asked_life = Arc::clone(&life);
    let presenter = thread::spawn(move || {
        if let Some(target) = asking.target(3, 640, 480, true, 0) {
            asking.send(Body::FrameDone(FrameDone {
                display_id: 3,
                texture_id: target.texture_id,
                width: 640,
                height: 480,
            }));
            asked_life.targets.lock().unwrap().push(target);
        }
    });
    while !life.killed.load(Ordering::SeqCst) {
        match connection.recv(Some(Duration::from_millis(20))) {
            Ok(frame) => {
                if let Some(Body::Stop(_)) = link.take(decode_body(&frame).unwrap()) {
                    life.stopped_by_window_manager.store(true, Ordering::SeqCst);
                    break;
                }
            }
            Err(TransportError::Timeout) => {}
            Err(_) => break,
        }
    }
    link.close();
    let _ = presenter.join();
    connection.close();
    life.exited.store(true, Ordering::SeqCst);
}

impl RendererLauncher for FakeLauncher {
    fn launch(&mut self) -> Result<Launched, String> {
        let (mut listener, mut connector) = memory::pair();
        let life = Arc::new(Life::default());
        self.lives.lock().unwrap().push(Arc::clone(&life));
        let pid = 100 + self.next_pid.fetch_add(1, Ordering::SeqCst);
        let running = Arc::clone(&life);
        thread::spawn(move || fake_renderer(&mut connector, running, pid));
        let connection = listener.accept(WAIT).map_err(|e| e.to_string())?;
        Ok(Launched {
            process: Box::new(FakeChild { pid, life }),
            connection,
        })
    }
}

/// The window manager's side: one window, showing display 3 in texture 0x1234.
#[derive(Default)]
struct Window {
    connected: AtomicUsize,
    disconnected: AtomicUsize,
    frames: Mutex<Vec<(u32, u64)>>,
    log: Mutex<Vec<String>>,
    book: Mutex<TextureBook<isize>>,
}

impl RendererHandler for Window {
    fn start(&self) -> Start {
        Start {
            display_width: 1280,
            display_height: 720,
            ..Start::default()
        }
    }
    fn connected(&self, _: &Started, _: &RendererLink) {
        self.book.lock().unwrap().clear();
        self.connected.fetch_add(1, Ordering::SeqCst);
    }
    fn message(&self, body: Body, link: &RendererLink) {
        match body {
            Body::TargetRequest(request) => {
                let given = self.book.lock().unwrap().give(
                    request.display_id,
                    Some(0x1234),
                    request.held_texture,
                );
                let (texture_id, handle) = match given {
                    Given::New(id) => (id, 0x55),
                    Given::Same(id) => (id, 0),
                    Given::Drop => (0, 0),
                };
                link.send(Body::Target(Target {
                    request: request.request,
                    drop: given == Given::Drop,
                    texture_id,
                    handle,
                    width: request.width,
                    height: request.height,
                }));
            }
            Body::FrameDone(done) => self
                .frames
                .lock()
                .unwrap()
                .push((done.display_id, done.texture_id)),
            _ => {}
        }
    }
    fn disconnected(&self) {
        self.disconnected.fetch_add(1, Ordering::SeqCst);
    }
    fn log(&self, line: &str) {
        self.log.lock().unwrap().push(line.to_string());
    }
}

#[test]
fn nfr5_a_renderer_that_ends_is_started_again_and_frames_resume() {
    let launcher = FakeLauncher::default();
    let lives = Arc::clone(&launcher.lives);
    let window = Arc::new(Window::default());
    let stop = Arc::new(AtomicBool::new(false));
    let supervisor = spawn_supervisor(
        Box::new(launcher),
        Arc::clone(&window) as Arc<dyn RendererHandler>,
        "dsa-wm test".into(),
        Arc::clone(&stop),
    )
    .unwrap();

    wait_until("the first renderer's frame", || {
        window.frames.lock().unwrap().len() == 1
    });
    let first = Arc::clone(&lives.lock().unwrap()[0]);
    // The texture crossed once, with its handle.
    assert_eq!(first.targets.lock().unwrap()[0].handle, 0x55);

    // The renderer dies. The window manager notices, says why, and starts another, which
    // gets the texture over again and fills it.
    first.killed.store(true, Ordering::SeqCst);
    wait_until("the renderer to start again", || {
        window.connected.load(Ordering::SeqCst) == 2
    });
    wait_until("the second renderer's frame", || {
        window.frames.lock().unwrap().len() == 2
    });
    assert_eq!(window.disconnected.load(Ordering::SeqCst), 1);
    let second = Arc::clone(&lives.lock().unwrap()[1]);
    assert_eq!(second.targets.lock().unwrap()[0].handle, 0x55);
    let log = window.log.lock().unwrap().join("\n");
    assert!(
        log.contains("the graphics renderer (dsa-gfx, pid 100) ended: it was killed (SIGKILL)"),
        "{log}"
    );
    assert!(log.contains("restart 1"), "{log}");

    // The window manager stops: the renderer is told to, and ends.
    stop.store(true, Ordering::SeqCst);
    supervisor.join().unwrap();
    assert!(second.stopped_by_window_manager.load(Ordering::SeqCst));
    assert!(second.exited.load(Ordering::SeqCst));
}

/// A launcher whose renderer cannot start.
struct Broken(Arc<AtomicUsize>);

impl RendererLauncher for Broken {
    fn launch(&mut self) -> Result<Launched, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("dsa-gfx.exe is not installed beside dsa-wm.exe".into())
    }
}

#[test]
fn nfr5_a_renderer_that_cannot_start_is_tried_again_later_and_says_why() {
    let tries = Arc::new(AtomicUsize::new(0));
    let window = Arc::new(Window::default());
    let stop = Arc::new(AtomicBool::new(false));
    let supervisor = spawn_supervisor(
        Box::new(Broken(Arc::clone(&tries))),
        Arc::clone(&window) as Arc<dyn RendererHandler>,
        "dsa-wm test".into(),
        Arc::clone(&stop),
    )
    .unwrap();
    wait_until("a second try", || tries.load(Ordering::SeqCst) >= 2);
    stop.store(true, Ordering::SeqCst);
    supervisor.join().unwrap();
    let log = window.log.lock().unwrap().join("\n");
    assert!(
        log.contains("could not be started: dsa-gfx.exe is not installed"),
        "{log}"
    );
    // Not a busy loop: two tries take at least the shortest wait.
    assert!(tries.load(Ordering::SeqCst) < 50);
}

#[test]
fn nfr5_an_answer_reaches_the_thread_that_waits_and_a_late_one_is_dropped() {
    let pending: Arc<Pending<u32>> = Arc::new(Pending::new());
    let request = pending.begin();
    let answering = Arc::clone(&pending);
    let answer = thread::spawn(move || answering.complete(request, 7));
    assert_eq!(pending.wait(request, WAIT), Some(7));
    answer.join().unwrap();
    // Nobody answers: the wait gives up.
    let unanswered = pending.begin();
    assert_eq!(pending.wait(unanswered, Duration::from_millis(30)), None);
    pending.complete(unanswered, 8);
    // A closed connection wakes everyone at once.
    let request = pending.begin();
    pending.close();
    assert_eq!(pending.wait(request, WAIT), None);
}

#[test]
fn nfr5_a_texture_crosses_to_the_renderer_once_and_again_after_a_restart() {
    let mut book: TextureBook<isize> = TextureBook::new();
    let first = book.give(3, Some(0x10), 0);
    let Given::New(id) = first else {
        panic!("expected a new texture, got {first:?}");
    };
    assert_eq!(book.give(3, Some(0x10), id), Given::Same(id));
    // The answer that carried the texture came too late, so the renderer does not hold it:
    // it goes over again.
    let again = book.give(3, Some(0x10), 0);
    assert!(
        matches!(again, Given::New(other) if other != id),
        "{again:?}"
    );
    let Given::New(id) = again else {
        unreachable!()
    };
    // A resize makes another texture.
    assert!(matches!(book.give(3, Some(0x20), id), Given::New(other) if other != id));
    // A display without a window drops its frames.
    assert_eq!(book.give(4, None, 0), Given::Drop);
    // A new renderer gets every texture again.
    book.clear();
    assert!(matches!(book.give(3, Some(0x20), 0), Given::New(_)));
}

#[test]
fn nfr5_the_renderer_lets_go_of_a_texture_only_after_it_has_the_next() {
    let mut held: HeldTargets<isize> = HeldTargets::new();
    let target = |texture_id, handle| Target {
        texture_id,
        handle,
        ..Target::default()
    };
    let open = |t: &Target| (t.handle != 0).then_some(t.handle as isize);
    assert_eq!(held.take(3, &target(1, 0x40), open), (Some(0x40), None));
    // The same texture again: no handle comes, and the one held is used.
    assert_eq!(held.take(3, &target(1, 0), open), (Some(0x40), None));
    // A new texture: the old handle is let go of after the new one is open.
    assert_eq!(
        held.take(3, &target(2, 0x44), open),
        (Some(0x44), Some(0x40))
    );
    // Dropped: the held one goes.
    let dropped = Target {
        drop: true,
        ..Target::default()
    };
    assert_eq!(held.take(3, &dropped, open), (None, Some(0x44)));
    assert_eq!(held.forget(3), None);
}

#[test]
fn nfr5_the_log_says_why_the_renderer_ended_and_when_it_starts_again() {
    assert_eq!(
        describe_exit_code(0xC000_0005),
        "it crashed: an access violation (exit code 0xC0000005)"
    );
    assert!(describe_exit_code(1).contains("End task"));
    assert_eq!(describe_exit_code(7), "it exited with code 7");
    assert_eq!(
        describe_signal(11),
        "it crashed: a segmentation fault (SIGSEGV)"
    );
    assert_eq!(describe_signal(42), "it was ended by signal 42");
    assert_eq!(
        restart_line(77, "it aborted (SIGABRT)", Duration::from_millis(500), 3),
        "the graphics renderer (dsa-gfx, pid 77) ended: it aborted (SIGABRT); the windows stay \
         and keep their last frame; starting it again in 0.5 s (restart 3)"
    );
    assert!(restart_line(77, "x", Duration::ZERO, 1).contains("starting it again now"));
}
