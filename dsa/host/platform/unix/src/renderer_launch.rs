//! Starting the graphics renderer process (`dsa-gfx`) from the window manager, on Linux and
//! macOS.
//!
//! The window manager makes a socket pair, starts the renderer with one end at descriptor 3
//! (`--window-manager 3`), and keeps the other as its connection ([`FdConnection`]). On Linux
//! the kernel ends the renderer when the window manager's supervising thread ends
//! (`PR_SET_PDEATHSIG`); everywhere, the renderer ends when its connection closes. So a
//! renderer never outlives the window manager that hands it its work.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use dsa_host_core::renderer::{
    describe_signal, Launched, RendererChild, RendererLauncher, ADDRESS_VARIABLE, RENDERER_PROGRAM,
    WINDOW_MANAGER_ARGUMENT,
};

use crate::fd_frames::{FdConnection, FdQueue};

/// The descriptor the renderer finds its end of the connection at.
pub const RENDERER_FD: i32 = 3;

/// The descriptors the current renderer's connection received, for the window manager's
/// handler to take from.
pub type CurrentFds = Arc<Mutex<Option<FdQueue>>>;

/// A started renderer.
struct UnixChild(Child);

impl RendererChild for UnixChild {
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn exited(&mut self) -> Option<String> {
        match self.0.try_wait() {
            Ok(Some(status)) => Some(match (status.code(), status.signal()) {
                (Some(0), _) => "it exited normally (exit code 0)".to_string(),
                (Some(code), _) => format!("it exited with code {code}"),
                (None, Some(signal)) => describe_signal(signal),
                (None, None) => "it ended".to_string(),
            }),
            Ok(None) => None,
            Err(error) => Some(format!("its state cannot be read: {error}")),
        }
    }

    fn kill(&mut self) {
        let _ = self.0.kill();
    }
}

impl Drop for UnixChild {
    fn drop(&mut self) {
        // A renderer still running when the window manager lets go of it is ended and
        // reaped, so it never lingers.
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// Starts the renderer program.
pub struct UnixLauncher {
    pub program: PathBuf,
    /// Variables the renderer gets beside the window manager's own.
    pub env: Vec<(String, String)>,
    /// The renderer's arguments instead of `--window-manager 3`, which then goes in the
    /// environment: a test runs the renderer inside its own test program this way.
    pub args: Option<Vec<String>>,
    /// Where the started renderer's received descriptors go.
    pub fds: CurrentFds,
}

impl UnixLauncher {
    pub fn new(program: PathBuf, fds: CurrentFds) -> UnixLauncher {
        UnixLauncher {
            program,
            env: Vec::new(),
            args: None,
            fds,
        }
    }

    /// The renderer installed beside this program.
    pub fn installed(fds: CurrentFds) -> UnixLauncher {
        let program = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(RENDERER_PROGRAM)))
            .unwrap_or_else(|| PathBuf::from(RENDERER_PROGRAM));
        UnixLauncher::new(program, fds)
    }
}

impl RendererLauncher for UnixLauncher {
    fn launch(&mut self) -> Result<Launched, String> {
        if !self.program.is_file() {
            return Err(format!("{} is not installed", self.program.display()));
        }
        let (ours, theirs) =
            UnixStream::pair().map_err(|e| format!("no socket pair for the renderer: {e}"))?;
        let theirs_fd = theirs.as_raw_fd();
        let address = RENDERER_FD.to_string();
        let mut command = Command::new(&self.program);
        match &self.args {
            Some(args) => {
                command.args(args).env(ADDRESS_VARIABLE, &address);
            }
            None => {
                command.arg(WINDOW_MANAGER_ARGUMENT).arg(&address);
            }
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        for (key, value) in &self.env {
            command.env(key, value);
        }
        // SAFETY: the closure runs in the child between fork and exec and calls only
        // async-signal-safe functions.
        unsafe {
            command.pre_exec(move || {
                if theirs_fd == RENDERER_FD {
                    let flags = libc::fcntl(RENDERER_FD, libc::F_GETFD);
                    if flags < 0
                        || libc::fcntl(RENDERER_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                } else if libc::dup2(theirs_fd, RENDERER_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                #[cfg(target_os = "linux")]
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|e| format!("{} did not start: {e}", self.program.display()))?;
        drop(theirs);
        let connection =
            FdConnection::new(ours).map_err(|e| format!("the renderer's connection: {e}"))?;
        *self.fds.lock().unwrap_or_else(|e| e.into_inner()) = Some(connection.fds());
        Ok(Launched {
            process: Box::new(UnixChild(child)),
            connection: Box::new(connection),
        })
    }
}
