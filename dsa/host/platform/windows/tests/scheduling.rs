//! How a process and a thread on a touch's path are scheduled.
#![cfg(windows)]

use dsa_host_windows::scheduling::{raise_this_process, raise_this_thread};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetPriorityClass, GetThreadPriority,
    ABOVE_NORMAL_PRIORITY_CLASS, THREAD_PRIORITY_HIGHEST,
};

#[test]
fn fr5_a_process_on_the_touch_path_runs_ahead_of_normal_programs() {
    raise_this_process().expect("raises");
    // SAFETY: the pseudo handle of this process.
    let class = unsafe { GetPriorityClass(GetCurrentProcess()) };
    assert_eq!(class, ABOVE_NORMAL_PRIORITY_CLASS);
}

#[test]
fn fr5_a_thread_on_the_touch_path_runs_at_high_priority() {
    // The other test raises this process's class while it runs, and a class change moves the
    // priority Windows reports for a thread. Raise the class first, as the window manager and
    // the window hosts do, so the result does not depend on which test runs first.
    raise_this_process().expect("raises");
    std::thread::spawn(|| {
        raise_this_thread();
        // SAFETY: the pseudo handle of this thread.
        let priority = unsafe { GetThreadPriority(GetCurrentThread()) };
        assert!(
            priority >= THREAD_PRIORITY_HIGHEST,
            "the thread runs at {priority}, below {THREAD_PRIORITY_HIGHEST}"
        );
    })
    .join()
    .unwrap();
}
