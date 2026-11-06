//! The guest's power hints on Linux: how hard the host runs the VM's processors.
//!
//! A KVM guest's virtual processors are threads of its crosvm process, scheduled like any
//! program's. So the performance the guest's power hints ask for becomes the nice value of
//! every thread of that process (on Linux each thread has its own):
//!
//! - `Full`: above normal priority, while Android launches an app, runs a game, or the user
//!   touches it;
//! - `Normal`: as Linux runs any program;
//! - `Efficient`: below normal priority, while the guest's display is off, it is in Doze, or
//!   low power is on, and no task window is shown.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Weak;
use std::time::Instant;

use dsa_host_core::power_hints::{Performance, PowerHints};
use dsa_host_core::session::{GuestPower, Session};

/// The nice value of a performance.
pub fn nice_of(performance: Performance) -> i32 {
    match performance {
        Performance::Full => -5,
        Performance::Normal => 0,
        Performance::Efficient => 10,
    }
}

/// Sets the nice value of every thread of process `pid`.
fn renice(pid: u32, nice: i32) -> Result<(), String> {
    let tasks = std::fs::read_dir(format!("/proc/{pid}/task"))
        .map_err(|e| format!("reading the threads of the VM's process {pid}: {e}"))?;
    for task in tasks.flatten() {
        let Some(tid) = task
            .file_name()
            .to_str()
            .and_then(|t| t.parse::<u32>().ok())
        else {
            continue;
        };
        // SAFETY: setpriority on a thread ID; a thread that has ended is an error, ignored.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, tid, nice) };
    }
    Ok(())
}

/// Follows the guest's power hints until the session ends.
pub fn follow(hints: Receiver<GuestPower>, session: Weak<Session>, user: String) {
    let mut policy = PowerHints::new();
    let mut applied: Option<(String, Performance)> = None;
    loop {
        let now = Instant::now();
        let event = match policy.next_change(now) {
            Some(at) => hints.recv_timeout(at.saturating_duration_since(now)),
            None => hints.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match event {
            Ok(GuestPower::Hint(hint)) => policy.hint(hint, Instant::now()),
            Ok(GuestPower::WindowsShown(shown)) => policy.windows_shown(shown),
            Ok(GuestPower::HostInput) => policy.host_input(Instant::now()),
            Ok(GuestPower::Stopped) => {
                policy.clear();
                applied = None;
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let wanted = policy.performance(Instant::now());
        let Some(vm_id) = session.upgrade().and_then(|s| s.vm_id()) else {
            continue;
        };
        if applied.as_ref() == Some(&(vm_id.clone(), wanted)) {
            continue;
        }
        let Some(pid) = vm_id.parse().ok().and_then(crate::launcher::vm_process) else {
            continue;
        };
        match renice(pid, nice_of(wanted)) {
            Ok(()) => {
                eprintln!(
                    "{user}: power: the VM runs {wanted:?} ({})",
                    policy.explain(Instant::now())
                );
                applied = Some((vm_id, wanted));
            }
            Err(why) => eprintln!("{user}: power: {why}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_the_guests_hints_set_how_hard_its_processors_run() {
        assert!(nice_of(Performance::Full) < nice_of(Performance::Normal));
        assert!(nice_of(Performance::Normal) < nice_of(Performance::Efficient));
        assert_eq!(nice_of(Performance::Normal), 0);
    }
}
