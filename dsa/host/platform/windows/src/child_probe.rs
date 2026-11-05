//! Running this program again, briefly, to ask something of a driver stack.
//!
//! Some questions the window manager has to answer (which video formats the GPU decodes, which
//! cameras the computer has) load a driver stack and its DLLs to answer. A DLL loaded once
//! stays loaded, with its heap, for the life of the process, so the window manager asks a
//! child copy of itself instead: the child loads the stack, prints the answer and ends, and
//! its memory goes with it. The stack loads into the window manager only once the guest uses
//! the device.

/// What a child printed: its standard output and its standard error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildOutput {
    pub stdout: String,
    pub stderr: String,
}

#[cfg(windows)]
pub use system::run_self;

#[cfg(windows)]
mod system {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::ChildOutput;

    /// No console window for the child.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Runs this program with `args`, waits at most `timeout` for it to end, and returns what
    /// it printed. A child that takes longer is ended, and nothing it printed is used.
    pub fn run_self(args: &[String], timeout: Duration) -> Result<ChildOutput, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        let mut child = Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("the child process did not start: {e}"))?;
        let mut stdout = child.stdout.take().ok_or("the child process has no output")?;
        let mut stderr = child.stderr.take().ok_or("the child process has no output")?;
        // Both pipes are drained while the child runs, so neither fills and stalls it.
        let out = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            text
        });
        let err = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "the child process did not finish in {} s",
                        timeout.as_secs()
                    ));
                }
            }
        }
        let stdout = out
            .join()
            .map_err(|_| "the child process's output was lost".to_string())?;
        let stderr = err.join().unwrap_or_default();
        Ok(ChildOutput { stdout, stderr })
    }
}
