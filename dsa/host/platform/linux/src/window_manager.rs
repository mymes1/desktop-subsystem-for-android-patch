//! Keeps the user's window manager running while their Android runs: the program that shows
//! Android's windows and renders for it, in the user's own graphical session.
//!
//! The service starts it as the user, with what it needs to reach their session: their runtime
//! folder, the Wayland display that is open there, or the X display when there is none.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use dsa_host_core::window_manager::WindowManagerHost;

use crate::users::Account;

/// The Wayland socket in `runtime_dir`, if a compositor serves one.
pub fn wayland_display(runtime_dir: &Path) -> Option<String> {
    let mut names: Vec<String> = std::fs::read_dir(runtime_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("wayland-") && !name.ends_with(".lock"))
        .collect();
    names.sort();
    names.into_iter().next()
}

/// The configured window manager of one user.
pub struct CommandWindowManager {
    account: Account,
    command: Vec<String>,
    child: Option<Child>,
}

impl CommandWindowManager {
    /// `command` is the program and its arguments.
    pub fn new(account: Account, command: Vec<String>) -> CommandWindowManager {
        CommandWindowManager {
            account,
            command,
            child: None,
        }
    }
}

impl WindowManagerHost for CommandWindowManager {
    fn running(&mut self) -> bool {
        match self.child.as_mut().map(|child| child.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                self.child = None;
                false
            }
            None => false,
        }
    }

    fn start(&mut self) -> Result<(), String> {
        let (program, args) = self
            .command
            .split_first()
            .ok_or("the window manager's command is empty")?;
        let runtime = format!("/run/user/{}", self.account.uid);
        if !Path::new(&runtime).exists() {
            return Err(format!(
                "{} is not signed in to a desktop session, where Android's windows are shown",
                self.account.name
            ));
        }
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.account.run_as(&mut command, None);
        command.env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={runtime}/bus"),
        );
        match wayland_display(Path::new(&runtime)) {
            Some(display) => {
                command.env("WAYLAND_DISPLAY", display);
            }
            None => {
                command.env("DISPLAY", ":0");
            }
        }
        let child = command
            .spawn()
            .map_err(|e| format!("starting the window manager {program}: {e}"))?;
        self.child = Some(child);
        Ok(())
    }
}

impl Drop for CommandWindowManager {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr3_the_window_manager_finds_the_users_wayland_display() {
        let dir = std::env::temp_dir().join(format!("dsa-runtime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(wayland_display(&dir), None);
        std::fs::write(dir.join("wayland-0.lock"), b"").unwrap();
        std::fs::write(dir.join("wayland-0"), b"").unwrap();
        std::fs::write(dir.join("bus"), b"").unwrap();
        assert_eq!(wayland_display(&dir), Some("wayland-0".into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
