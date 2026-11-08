//! Running under the Windows Service Control Manager.
//!
//! [`run_as_service`] hands the process to the SCM, reports the service as running while
//! `body` works, turns a stop or shutdown request into the stop flag `body` watches, and
//! reports the service stopped, with an exit code, once `body` returns.
//!
//! [`WindowsServiceStarter`] starts a Windows service through the SCM if it is stopped,
//! waiting until it enters the running state.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_CALL_NOT_IMPLEMENTED, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_CANNOT_ACCEPT_CTRL,
    ERROR_SERVICE_DISABLED, ERROR_SERVICE_NOT_ACTIVE, NO_ERROR,
};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW,
    QueryServiceStatus, RegisterServiceCtrlHandlerExW, SetServiceStatus,
    StartServiceCtrlDispatcherW, StartServiceW, SC_HANDLE, SC_MANAGER_CONNECT,
    SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_AUTO_START, SERVICE_CHANGE_CONFIG,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_DISABLED,
    SERVICE_NO_CHANGE, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOP, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};

/// The service's work: runs until the flag is set.
pub type ServiceBody = Box<dyn FnOnce(Arc<AtomicBool>) -> Result<(), String> + Send>;

/// The exit code reported when the body fails. A service-specific code, so the SCM shows
/// it as the service's own failure.
const ERROR_SERVICE_SPECIFIC_ERROR: u32 = 1066;

/// How often a stopping service tells the SCM it is still stopping.
const STOP_PENDING_EVERY: Duration = Duration::from_secs(5);

/// The exit code a service reports as it stops. A stop someone asked for (setup stopping the
/// service to replace it, the Services console, shutdown) ends with success even when
/// stopping found something wrong, which the log keeps: an error code would read as the
/// service failing, and a service configured to restart on failure would come back while
/// setup is replacing its files. Only a service that ended on its own with an error says so.
pub fn stopped_exit_code(stop_requested: bool, failed: bool) -> u32 {
    u32::from(failed && !stop_requested)
}

struct State {
    body: Mutex<Option<ServiceBody>>,
    stop: Arc<AtomicBool>,
    handle: Mutex<SERVICE_STATUS_HANDLE>,
    result: Mutex<Option<Result<(), String>>>,
    /// Set by a stop or shutdown request from the SCM.
    stop_requested: AtomicBool,
    /// Set once the service has reported itself stopped.
    stopped: AtomicBool,
    /// The checkpoint of the next stop-pending report.
    checkpoint: AtomicU32,
    /// Held around a report that must not pass the stopped one.
    reporting: Mutex<()>,
}

// SAFETY: the status handle is an opaque value the SCM lets any thread use.
unsafe impl Send for State {}
unsafe impl Sync for State {}

static STATE: OnceLock<State> = OnceLock::new();

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn report(state: u32, exit_code: u32, wait_hint_ms: u32) {
    report_with_checkpoint(state, exit_code, wait_hint_ms, 0);
}

fn report_with_checkpoint(state: u32, exit_code: u32, wait_hint_ms: u32, checkpoint: u32) {
    let Some(shared) = STATE.get() else { return };
    let handle = *shared.handle.lock().unwrap_or_else(|e| e.into_inner());
    if handle.is_null() {
        return;
    }
    let accepted = if state == SERVICE_RUNNING {
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
    } else {
        0
    };
    let (win32, specific) = if exit_code == 0 {
        (NO_ERROR, 0)
    } else {
        (ERROR_SERVICE_SPECIFIC_ERROR, exit_code)
    };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: win32,
        dwServiceSpecificExitCode: specific,
        dwCheckPoint: checkpoint,
        dwWaitHint: wait_hint_ms,
    };
    // SAFETY: a handle from RegisterServiceCtrlHandlerExW and a valid status.
    unsafe { SetServiceStatus(handle, &status) };
}

/// Tells the SCM the service is still stopping, with a new checkpoint each time, until it
/// has stopped: stopping every VM can take longer than one wait hint, and a service that
/// stays silent past it looks hung to the SCM and to setup waiting for it.
fn keep_reporting_stop_pending() {
    let spawned = std::thread::Builder::new()
        .name("dsa-stop-pending".into())
        .spawn(|| {
            let Some(shared) = STATE.get() else { return };
            loop {
                {
                    // Never after the stopped report: the SCM takes the last word it hears.
                    let _order = shared.reporting.lock().unwrap_or_else(|e| e.into_inner());
                    if shared.stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    let checkpoint = shared.checkpoint.fetch_add(1, Ordering::Relaxed) + 1;
                    report_with_checkpoint(
                        SERVICE_STOP_PENDING,
                        0,
                        (STOP_PENDING_EVERY * 4).as_millis() as u32,
                        checkpoint,
                    );
                }
                std::thread::sleep(STOP_PENDING_EVERY);
            }
        });
    if let Err(why) = spawned {
        eprintln!("the service cannot report that it is stopping: {why}");
    }
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            let first = STATE.get().is_some_and(|shared| {
                shared.stop.store(true, Ordering::Relaxed);
                !shared.stop_requested.swap(true, Ordering::Relaxed)
            });
            // Stopping every VM may take a while; say so, so the SCM waits.
            report(SERVICE_STOP_PENDING, 0, 60_000);
            if first {
                keep_reporting_stop_pending();
            }
            NO_ERROR
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR,
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
    let Some(shared) = STATE.get() else { return };
    // The name is ignored for a service that runs alone in its process.
    let name = wide("");
    // SAFETY: a NUL-terminated name and a handler with the right signature.
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control_handler), std::ptr::null())
    };
    if handle.is_null() {
        *shared.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(format!(
            "registering with the Service Control Manager failed: {}",
            std::io::Error::last_os_error()
        )));
        return;
    }
    *shared.handle.lock().unwrap_or_else(|e| e.into_inner()) = handle;
    report(SERVICE_START_PENDING, 0, 10_000);
    let body = shared.body.lock().unwrap_or_else(|e| e.into_inner()).take();
    report(SERVICE_RUNNING, 0, 0);
    let result = match body {
        Some(body) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            body(Arc::clone(&shared.stop))
        })) {
            Ok(res) => res,
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                Err(format!("the service panicked: {msg}"))
            }
        },
        None => Err("the service was started twice".into()),
    };
    if let Err(why) = &result {
        eprintln!("the service stopped with an error: {why}");
    }
    // A service that stopped itself (to restart into new files) set the flag, not the SCM;
    // it ends with success all the same, so the flag counts as a stop that was asked for.
    let asked =
        shared.stop_requested.load(Ordering::Relaxed) || shared.stop.load(Ordering::Relaxed);
    {
        let _order = shared.reporting.lock().unwrap_or_else(|e| e.into_inner());
        shared.stopped.store(true, Ordering::Relaxed);
        report(
            SERVICE_STOPPED,
            stopped_exit_code(asked, result.is_err()),
            0,
        );
    }
    *shared.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
}

/// Hands the process to the SCM and runs `body` as the service. Returns once the service
/// has stopped, with `body`'s result. Fails at once when the process was not started by
/// the SCM.
pub fn run_as_service(body: ServiceBody) -> Result<(), String> {
    let state = State {
        body: Mutex::new(Some(body)),
        stop: Arc::new(AtomicBool::new(false)),
        handle: Mutex::new(std::ptr::null_mut()),
        result: Mutex::new(None),
        stop_requested: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        checkpoint: AtomicU32::new(0),
        reporting: Mutex::new(()),
    };
    if STATE.set(state).is_err() {
        return Err("run_as_service was called twice".into());
    }
    let mut name = wide("");
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // SAFETY: a table ending in a null entry; it returns once the service has stopped.
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(format!(
            "not started by the Service Control Manager ({}); \
             run `dsa-hostd serve <config>` to serve in the foreground",
            std::io::Error::last_os_error()
        ));
    }
    STATE
        .get()
        .and_then(|shared| {
            shared
                .result
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
        })
        .unwrap_or_else(|| Err("the service ended without reporting a result".into()))
}

struct OwnedScHandle(SC_HANDLE);

impl Drop for OwnedScHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a valid service handle, closed once.
            unsafe { CloseServiceHandle(self.0) };
        }
    }
}

/// Starts the Windows service `name` through the Service Control Manager if it is not
/// already running, waiting until it enters the running state.
pub struct WindowsServiceStarter {
    name: String,
    timeout: Duration,
}

impl WindowsServiceStarter {
    pub fn new(name: &str, timeout: Duration) -> WindowsServiceStarter {
        WindowsServiceStarter {
            name: name.to_string(),
            timeout,
        }
    }

    pub fn start(&self) -> Result<(), String> {
        // SAFETY: null machine and database names connect to the local active SCM database.
        let scm = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
        if scm.is_null() {
            return Err(format!(
                "opening the Service Control Manager failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let scm = OwnedScHandle(scm);
        let name_w = wide(&self.name);
        // SAFETY: valid SCM handle and null-terminated service name.
        let service =
            unsafe { OpenServiceW(scm.0, name_w.as_ptr(), SERVICE_START | SERVICE_QUERY_STATUS) };
        if service.is_null() {
            return Err(format!(
                "opening service {:?} failed: {}",
                self.name,
                std::io::Error::last_os_error()
            ));
        }
        let service = OwnedScHandle(service);
        let mut status = SERVICE_STATUS {
            dwServiceType: 0,
            dwCurrentState: 0,
            dwControlsAccepted: 0,
            dwWin32ExitCode: 0,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: 0,
        };
        // SAFETY: valid service handle and status pointer.
        if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
            return Err(format!(
                "querying service {:?} status failed: {}",
                self.name,
                std::io::Error::last_os_error()
            ));
        }
        if status.dwCurrentState == SERVICE_RUNNING {
            return Ok(());
        }
        if status.dwCurrentState != SERVICE_START_PENDING {
            // SAFETY: valid service handle, no arguments passed to start.
            if unsafe { StartServiceW(service.0, 0, std::ptr::null()) } == 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(ERROR_SERVICE_DISABLED as i32) {
                    return Err(format!(
                        "the service {:?} is held while setup replaces DSA's files; it starts \
                         again when setup is done",
                        self.name
                    ));
                }
                if err.raw_os_error() != Some(ERROR_SERVICE_ALREADY_RUNNING as i32) {
                    return Err(format!("starting service {:?} failed: {err}", self.name));
                }
            }
        }
        let deadline = Instant::now() + self.timeout;
        while Instant::now() < deadline {
            // SAFETY: valid service handle and status pointer.
            if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
                return Err(format!(
                    "querying service {:?} status failed: {}",
                    self.name,
                    std::io::Error::last_os_error()
                ));
            }
            if status.dwCurrentState == SERVICE_RUNNING {
                return Ok(());
            }
            if status.dwCurrentState == SERVICE_STOPPED {
                return Err(format!(
                    "service {:?} stopped (exit code {})",
                    self.name, status.dwWin32ExitCode
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "timed out waiting for service {:?} to start",
            self.name
        ))
    }
}

/// Opens the service `name` with `access`.
fn open_service(name: &str, access: u32) -> Result<OwnedScHandle, String> {
    // SAFETY: null machine and database names connect to the local active SCM database.
    let scm = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
    if scm.is_null() {
        return Err(format!(
            "opening the Service Control Manager failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let scm = OwnedScHandle(scm);
    let name_w = wide(name);
    // SAFETY: a valid SCM handle and a NUL-terminated service name.
    let service = unsafe { OpenServiceW(scm.0, name_w.as_ptr(), access) };
    if service.is_null() {
        return Err(format!(
            "opening service {name:?} failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(OwnedScHandle(service))
}

fn empty_status() -> SERVICE_STATUS {
    SERVICE_STATUS {
        dwServiceType: 0,
        dwCurrentState: 0,
        dwControlsAccepted: 0,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 0,
    }
}

fn query_state(service: &OwnedScHandle, name: &str) -> Result<u32, String> {
    let mut status = empty_status();
    // SAFETY: a valid service handle and status pointer.
    if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
        return Err(format!(
            "querying service {name:?} status failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(status.dwCurrentState)
}

/// Whether the service starts with Windows or cannot be started at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartType {
    Automatic,
    Disabled,
}

/// Sets how the service `name` starts. Setup holds the service with
/// [`StartType::Disabled`] from just before it stops it until its new files are in place:
/// the `dsa` command (which the settings app runs every few seconds) starts a stopped
/// service on its own, and would otherwise bring the old one back while setup replaces it.
pub fn set_start_type(name: &str, start: StartType) -> Result<(), String> {
    let service = open_service(name, SERVICE_CHANGE_CONFIG)?;
    let start = match start {
        StartType::Automatic => SERVICE_AUTO_START,
        StartType::Disabled => SERVICE_DISABLED,
    };
    // SAFETY: a valid service handle; every other setting stays as it is.
    let changed = unsafe {
        ChangeServiceConfigW(
            service.0,
            SERVICE_NO_CHANGE,
            start,
            SERVICE_NO_CHANGE,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if changed == 0 {
        return Err(format!(
            "changing how service {name:?} starts failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Whether the service `name` is running (or starting, or stopping) now.
pub fn is_running(name: &str) -> Result<bool, String> {
    let service = open_service(name, SERVICE_QUERY_STATUS)?;
    Ok(query_state(&service, name)? != SERVICE_STOPPED)
}

/// Waits up to `timeout` for the service `name` to be stopped.
pub fn wait_until_stopped(name: &str, timeout: Duration) -> Result<(), String> {
    let service = open_service(name, SERVICE_QUERY_STATUS)?;
    let deadline = Instant::now() + timeout;
    loop {
        if query_state(&service, name)? == SERVICE_STOPPED {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "service {name:?} did not stop within {} seconds",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Asks the service `name` to stop, if it runs, and waits up to `timeout` for it. Returns
/// whether it was running.
pub fn stop_and_wait(name: &str, timeout: Duration) -> Result<bool, String> {
    let service = open_service(name, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    if query_state(&service, name)? == SERVICE_STOPPED {
        return Ok(false);
    }
    let mut status = empty_status();
    // SAFETY: a valid service handle opened with the right to stop it.
    if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0 {
        let err = std::io::Error::last_os_error();
        // Already stopping, or stopped meanwhile: the wait below sees it through.
        let stopping = [ERROR_SERVICE_NOT_ACTIVE, ERROR_SERVICE_CANNOT_ACCEPT_CTRL]
            .iter()
            .any(|code| err.raw_os_error() == Some(*code as i32));
        if !stopping {
            return Err(format!("stopping service {name:?} failed: {err}"));
        }
    }
    drop(service);
    wait_until_stopped(name, timeout)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::stopped_exit_code;

    #[test]
    fn fr15_a_requested_stop_ends_with_success_so_the_scm_does_not_restart_it() {
        // Setup stopping the service, even when stopping an Android went wrong.
        assert_eq!(stopped_exit_code(true, true), 0);
        assert_eq!(stopped_exit_code(true, false), 0);
        // A service that ended on its own with an error says so.
        assert_eq!(stopped_exit_code(false, true), 1);
        assert_eq!(stopped_exit_code(false, false), 0);
    }
}
