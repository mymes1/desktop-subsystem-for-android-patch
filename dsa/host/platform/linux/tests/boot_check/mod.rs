//! What the boot checks of the real image share: the programs they start, which stop with
//! the check whether it passes, fails or runs out of time, the guest log they keep, and
//! commands that may not run longer than they are given.
//!
//! The boot checks run one after another on one runner. A check that left its VM running
//! held the vsock ports, and the next check's service could not listen on them ("vsock port
//! 5000: Address already in use"). So each program a check starts runs in a process group of
//! its own, which also holds what that program starts (the service starts crosvm, the window
//! manager the renderer), and the whole group is stopped with it. A check that runs past its
//! time stops every group itself and ends, before the job's own timeout kills the test
//! process and leaves the groups behind.
#![allow(dead_code)] // Each check uses only part of this.

use std::io::Read;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// The process groups of the programs the checks started that have not been stopped yet.
static GROUPS: Mutex<Vec<libc::pid_t>> = Mutex::new(Vec::new());

/// How long a stopped program has to end by itself before its group is killed.
const GRACE: Duration = Duration::from_secs(60);

/// A program a check started, writing to its log, stopped with its whole process group when
/// dropped: SIGTERM, which has the service stop its VMs, then SIGKILL to anything of the
/// group still running.
pub struct Process {
    child: Child,
    pub log: PathBuf,
}

impl Process {
    pub fn spawn(command: &mut Command, log: PathBuf) -> Process {
        let out = std::fs::File::create(&log).unwrap();
        let child = command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .expect("starting a DSA program");
        groups().push(child.id() as libc::pid_t);
        Process { child, log }
    }

    /// What the program wrote so far.
    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let group = self.child.id() as libc::pid_t;
        // SAFETY: kill with a child's own process ID and a constant signal.
        unsafe {
            libc::kill(group, libc::SIGTERM);
        }
        let deadline = Instant::now() + GRACE;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        kill_group(group);
        let _ = self.child.kill();
        let _ = self.child.wait();
        groups().retain(|&g| g != group);
    }
}

fn groups() -> MutexGuard<'static, Vec<libc::pid_t>> {
    GROUPS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Sends SIGKILL to every process left in the process group `group`.
fn kill_group(group: libc::pid_t) {
    // SAFETY: kill with a negated process group ID, which names that group, and a constant
    // signal; the group is one a check made for a program of its own.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
}

/// Whether any process is left in the process group `group`.
fn group_alive(group: libc::pid_t) -> bool {
    // SAFETY: signal 0 only asks whether the group exists.
    unsafe { libc::kill(-group, 0) == 0 }
}

/// Stops every program the checks started: SIGTERM to each, up to `grace` for them to end,
/// then SIGKILL to what is left of their groups.
pub fn stop_everything(grace: Duration) {
    let all = groups().clone();
    for &group in &all {
        // SAFETY: as in kill_group, with SIGTERM.
        unsafe {
            libc::kill(-group, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && all.iter().any(|&group| group_alive(group)) {
        std::thread::sleep(Duration::from_millis(250));
    }
    for &group in &all {
        kill_group(group);
    }
    groups().clear();
}

/// Ends the check when it runs past `budget`: it says so, stops every program it started,
/// and ends the test process. Dropping the watchdog when the check is done stands it down.
pub struct Watchdog {
    _done: mpsc::Sender<()>,
}

impl Watchdog {
    pub fn start(check: &'static str, budget: Duration) -> Watchdog {
        let (done, wait) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = wait.recv_timeout(budget) {
                eprintln!(
                    "{check}: the check ran past its {} s budget; stopping the service, its VM and the other programs it started",
                    budget.as_secs()
                );
                stop_everything(Duration::from_secs(20));
                eprintln!(
                    "{check}: failed: it did not finish within {} s",
                    budget.as_secs()
                );
                std::process::exit(101);
            }
        });
        Watchdog { _done: done }
    }
}

/// The guest log's lines, also after a thread panicked while it held them, so a failing
/// check does not make the thread that reads the log panic on top of it.
pub fn lock(log: &Mutex<Vec<String>>) -> MutexGuard<'_, Vec<String>> {
    log.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a command run with [run_bounded] gave.
pub struct Bounded {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// How it ended, or None when it was stopped.
    pub status: Option<ExitStatus>,
    /// Whether it was stopped because it ran out of time, rather than because it had written
    /// `enough`.
    pub timed_out: bool,
    pub took: Duration,
}

/// Runs `command` until it ends, until it has written `enough` bytes to its standard output,
/// or for `limit` at most, and stops it in the last two cases.
pub fn run_bounded(
    command: &mut Command,
    limit: Duration,
    enough: usize,
) -> Result<Bounded, String> {
    let started = Instant::now();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|why| format!("starting {:?}: {why}", command.get_program()))?;
    let stdout: Arc<Mutex<Vec<u8>>> = Arc::default();
    let stderr: Arc<Mutex<Vec<u8>>> = Arc::default();
    // Each reader holds a sender and drops it when its pipe closes: the receiver then hears
    // that both are done.
    let (closed, pipes_closed) = mpsc::channel::<()>();
    let readers = [
        (
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
            Arc::clone(&stdout),
            closed.clone(),
        ),
        (
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
            Arc::clone(&stderr),
            closed,
        ),
    ];
    for (pipe, into, closed) in readers {
        let Some(mut pipe) = pipe else { continue };
        std::thread::spawn(move || {
            let mut chunk = [0u8; 64 * 1024];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => into
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend_from_slice(&chunk[..n]),
                }
            }
            drop(closed);
        });
    }
    let deadline = started + limit;
    let (status, timed_out) = loop {
        if let Ok(Some(status)) = child.try_wait() {
            // What it wrote last may still be in the pipes: read until they close, briefly.
            let _ = pipes_closed.recv_timeout(Duration::from_secs(5));
            break (Some(status), false);
        }
        if stdout.lock().unwrap_or_else(PoisonError::into_inner).len() >= enough {
            break (None, false);
        }
        if Instant::now() >= deadline {
            break (None, true);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = pipes_closed.recv_timeout(Duration::from_secs(1));
    }
    let stdout = std::mem::take(&mut *stdout.lock().unwrap_or_else(PoisonError::into_inner));
    let stderr = std::mem::take(&mut *stderr.lock().unwrap_or_else(PoisonError::into_inner));
    Ok(Bounded {
        stdout,
        stderr,
        status,
        timed_out,
        took: started.elapsed(),
    })
}
