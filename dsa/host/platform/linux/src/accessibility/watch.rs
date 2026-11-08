//! What the AT-SPI provider shares between its threads, and the watch of each window's tree.
//!
//! A window asks the guest for its task's tree only once a client reads into it
//! ([`Shared::asked`]): a thread then watches the tree through the service's local API
//! ([`HostApiClient::watch_accessibility`]), applies what comes to the window's objects and
//! raises their events. Once no client has asked for the grace period and none listens for
//! events ([`Shared::listening`]), the thread ends, its connection closes and the guest stops
//! reading the task. A window nobody reads never dials the service, so the guest reads
//! nothing for it.
//!
//! A tree that breaks the rules is dropped and logged, and the watch starts over; nothing a
//! tree holds can end the compositor.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::accessibility::{
    ActionRequest, GuestAccessibility, NodeAction, PresenceChange, TreeError, TreeUpdate,
};
use dsa_host_core::host_api::HostApiClient;
use dsa_transport::Connector;

use super::objects::{Access, Event, Object, TitleCommand, WindowFacts};

/// Dials the service's local API, once for each watch.
pub type ApiConnect = Arc<dyn Fn() -> Box<dyn Connector> + Send + Sync>;

/// Where a title bar button's action goes: the compositor's main thread.
pub type Commands = Arc<dyn Fn(TitleCommand) + Send + Sync>;

/// Where the provider writes what it reports.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// Where events go: the accessibility bus, while the provider is on it.
pub trait EventSink: Send + Sync {
    fn emit(&self, events: &[Event]);
}

/// The build a watch names itself by.
const BUILD: &str = "dsa-compositor accessibility";

/// How long a watch waits for the service, and between two tries after it failed.
const CONNECT: Duration = Duration::from_secs(5);
const RETRY: Duration = Duration::from_secs(1);

/// How long a watch waits for news before it looks at its actions and its clients again.
const POLL: Duration = Duration::from_millis(20);

/// What the provider's threads share.
pub struct Shared {
    access: Mutex<Access>,
    connect: Option<ApiConnect>,
    commands: Commands,
    log: Log,
    sink: Mutex<Option<Arc<dyn EventSink>>>,
    /// Whether a screen reader runs, as the session's accessibility status says.
    screen_reader: AtomicBool,
    /// Whether a client on the accessibility bus has registered for events.
    listeners: AtomicBool,
    next_watch: AtomicU64,
}

impl Shared {
    /// The provider's state for `access`. Without `connect` no tree is ever watched; title
    /// bar actions go to `commands`.
    pub fn new(
        access: Access,
        connect: Option<ApiConnect>,
        commands: Commands,
        log: Log,
    ) -> Arc<Shared> {
        Arc::new(Shared {
            access: Mutex::new(access),
            connect,
            commands,
            log,
            sink: Mutex::new(None),
            screen_reader: AtomicBool::new(false),
            listeners: AtomicBool::new(false),
            next_watch: AtomicU64::new(0),
        })
    }

    /// The windows' objects, behind the one lock every thread takes briefly.
    pub fn lock(&self) -> MutexGuard<'_, Access> {
        self.access.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn log(&self, line: &str) {
        (self.log)(line);
    }

    /// Sends `events` to the clients, while the provider is on the accessibility bus.
    pub fn emit(&self, events: &[Event]) {
        if events.is_empty() {
            return;
        }
        let sink = self.sink.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(sink) = sink {
            sink.emit(events);
        }
    }

    /// The provider came onto the accessibility bus (`Some`) or left it (`None`).
    pub fn set_sink(&self, sink: Option<Arc<dyn EventSink>>) {
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = sink;
    }

    /// Whether the provider is on the accessibility bus, where clients can read the windows.
    pub fn connected(&self) -> bool {
        self.sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    pub fn set_screen_reader(&self, on: bool) {
        self.screen_reader.store(on, Ordering::Relaxed);
    }

    pub fn set_listeners(&self, on: bool) {
        self.listeners.store(on, Ordering::Relaxed);
    }

    /// Whether a client listens for events: then every watched window keeps its tree, since
    /// the client hears its changes without asking again.
    pub fn listening(&self) -> bool {
        self.screen_reader.load(Ordering::Relaxed) || self.listeners.load(Ordering::Relaxed)
    }

    /// The window of `task_id` opened, or what it shows changed.
    pub fn sync(&self, task_id: i32, facts: WindowFacts) {
        let events = self.lock().sync(task_id, facts);
        self.emit(&events);
    }

    /// The window of `task_id` closed; its watch ends with it.
    pub fn close(&self, task_id: i32) {
        let events = self.lock().close(task_id);
        self.emit(&events);
    }

    /// A client read into the window of `task_id`: the guest is asked for its tree if no
    /// watch runs yet.
    pub fn asked(self: &Arc<Self>, task_id: i32) {
        let Some(connect) = self.connect.clone() else {
            return;
        };
        let (id, actions) = {
            let mut access = self.lock();
            let Some(window) = access.window_mut(task_id) else {
                return;
            };
            window.presence.asked(Instant::now());
            if window.watch.is_some() {
                return;
            }
            let id = self.next_watch.fetch_add(1, Ordering::Relaxed) + 1;
            let (sender, actions) = mpsc::channel();
            window.watch = Some(id);
            access.attach_actions(task_id, sender);
            (id, actions)
        };
        self.log(&format!(
            "task {task_id}: an accessibility client reads the window; watching its \
             accessibility tree"
        ));
        let shared = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("dsa-compositor-accessibility".into())
            .spawn(move || watch(shared, task_id, id, connect, actions));
        if let Err(why) = spawned {
            self.log(&format!("could not watch the accessibility tree: {why}"));
            let mut access = self.lock();
            if let Some(window) = access.window_mut(task_id) {
                window.watch = None;
                window.actions = None;
                window.presence.end();
            }
        }
    }

    /// Ends every window's watch: no client can be reading any longer.
    pub fn end_watches(&self) {
        let mut access = self.lock();
        for task_id in access.task_ids() {
            if let Some(window) = access.window_mut(task_id) {
                window.presence.end();
            }
        }
    }

    /// Carries out action `index` of `object`: a node's goes to Android, a button's to the
    /// compositor. An error says why it could not be.
    pub fn perform(&self, object: Object, index: i32) -> Result<(), String> {
        let command = self.lock().perform(object, index)?;
        if let Some(command) = command {
            (self.commands)(command);
        }
        Ok(())
    }

    /// Asks Android to do `action` on the node `object`.
    pub fn request(&self, object: Object, action: NodeAction, text: String) -> Result<(), String> {
        self.lock().request(object, action, text)
    }

    /// Whether watch `id` should end, cleaning up after it when it should: the window has
    /// closed, another watch took its place, or no client wants the tree any longer.
    fn should_stop(&self, task_id: i32, id: u64) -> bool {
        let listening = self.listening();
        let events = {
            let mut access = self.lock();
            let Some(window) = access.window_mut(task_id) else {
                return true;
            };
            if window.watch != Some(id) {
                return true;
            }
            let stop = window.presence.poll(Instant::now(), listening)
                == Some(PresenceChange::Stop)
                || !window.presence.active();
            if !stop {
                return false;
            }
            window.watch = None;
            window.actions = None;
            access.clear(task_id)
        };
        self.log(&format!(
            "task {task_id}: no accessibility client reads the window; stopped watching its \
             accessibility tree"
        ));
        self.emit(&events);
        true
    }

    /// Takes an update for watch `id`.
    fn apply(&self, task_id: i32, id: u64, update: TreeUpdate) -> Result<(), TreeError> {
        let events = {
            let mut access = self.lock();
            if access.window(task_id).and_then(|window| window.watch) != Some(id) {
                return Ok(());
            }
            access.apply(task_id, update)?
        };
        self.emit(&events);
        Ok(())
    }

    /// Forgets the tree of watch `id`, which a new watch reads again whole.
    fn reset(&self, task_id: i32, id: u64) {
        let events = {
            let mut access = self.lock();
            if access.window(task_id).and_then(|window| window.watch) != Some(id) {
                return;
            }
            access.clear(task_id)
        };
        self.emit(&events);
    }

    /// Waits `wait`, unless watch `id` should end first. Whether it should.
    fn wait(&self, task_id: i32, id: u64, wait: Duration) -> bool {
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if self.should_stop(task_id, id) {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.should_stop(task_id, id)
    }
}

/// Watch `id` of the task's tree: watches it through the service until it should end, and
/// starts over when the connection ends or a tree cannot be taken.
fn watch(
    shared: Arc<Shared>,
    task_id: i32,
    id: u64,
    connect: ApiConnect,
    actions: Receiver<ActionRequest>,
) {
    let mut failures = 0u32;
    loop {
        if shared.should_stop(task_id, id) {
            return;
        }
        let mut connector = connect();
        let stream = HostApiClient::connect(&mut *connector, BUILD, CONNECT)
            .and_then(|client| client.watch_accessibility(task_id));
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(why) => {
                failures += 1;
                if failures == 1 {
                    shared.log(&format!(
                        "task {task_id}: could not watch the accessibility tree: {why}"
                    ));
                }
                if shared.wait(task_id, id, RETRY) {
                    return;
                }
                continue;
            }
        };
        failures = 0;
        loop {
            if shared.should_stop(task_id, id) {
                return;
            }
            let mut broken = false;
            loop {
                match actions.try_recv() {
                    Ok(action) => {
                        if stream.perform(&action).is_err() {
                            broken = true;
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    // The window took another watch, or closed.
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            if broken {
                break;
            }
            match stream.next(POLL) {
                Ok(None) => {}
                Ok(Some(GuestAccessibility::Tree(update))) => {
                    match shared.apply(task_id, id, update) {
                        Ok(()) => {}
                        // A change that missed its tree: watch again, for a full tree.
                        Err(TreeError::Epoch { .. }) => break,
                        Err(TreeError::Malformed(why)) => {
                            shared.log(&format!(
                                "task {task_id}: dropped an accessibility tree: {why}"
                            ));
                            shared.reset(task_id, id);
                            break;
                        }
                    }
                }
                Ok(Some(GuestAccessibility::ActionResult(result))) => {
                    if let Err(why) = result.outcome {
                        shared.log(&format!(
                            "task {task_id}: an accessibility action was not done: {why}"
                        ));
                    }
                }
                Err(why) => {
                    shared.log(&format!(
                        "task {task_id}: the accessibility watch ended: {why}"
                    ));
                    shared.reset(task_id, id);
                    if shared.wait(task_id, id, RETRY) {
                        return;
                    }
                    break;
                }
            }
        }
    }
}
