//! QMP, the QEMU Machine Protocol: JSON lines over the VM's Unix socket.
//!
//! The host uses it to change the running VM: plug memory in and take it back (virtio-mem),
//! pause and resume it (freezing, only when the user chose it), and ask the guest to power off
//! or end the VM.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// How long one command may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// A connection to a VM's QMP socket, in command mode.
pub struct Qmp {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

/// What QEMU answered: the command's `return`, or its error's description.
pub fn reply_of(line: &Value) -> Option<Result<Value, String>> {
    if let Some(value) = line.get("return") {
        return Some(Ok(value.clone()));
    }
    let error = line.get("error")?;
    Some(Err(error
        .get("desc")
        .and_then(Value::as_str)
        .unwrap_or("QEMU refused the command")
        .to_string()))
}

impl Qmp {
    /// Connects to the socket at `path`, waiting at most `wait` for QEMU to create it, and
    /// leaves capabilities negotiation.
    pub fn connect(path: &Path, wait: Duration) -> io::Result<Qmp> {
        let deadline = Instant::now() + wait;
        let stream = loop {
            match UnixStream::connect(path) {
                Ok(stream) => break stream,
                Err(error) if Instant::now() >= deadline => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("QEMU's control socket {}: {error}", path.display()),
                    ))
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        stream.set_read_timeout(Some(COMMAND_TIMEOUT))?;
        let writer = stream.try_clone()?;
        let mut qmp = Qmp {
            reader: BufReader::new(stream),
            writer,
        };
        // The greeting, then the end of negotiation.
        let greeting = qmp.read_line()?;
        if greeting.get("QMP").is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "QEMU's control socket did not greet with QMP",
            ));
        }
        qmp.execute("qmp_capabilities", None)?;
        Ok(qmp)
    }

    fn read_line(&mut self) -> io::Result<Value> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "QEMU closed its control socket",
            ));
        }
        serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Runs `command` with `arguments` and returns what it returned. Events that arrive
    /// meanwhile are skipped.
    pub fn execute(&mut self, command: &str, arguments: Option<Value>) -> io::Result<Value> {
        let mut request = json!({ "execute": command });
        if let Some(arguments) = arguments {
            request["arguments"] = arguments;
        }
        let mut text = request.to_string();
        text.push('\n');
        self.writer.write_all(text.as_bytes())?;
        loop {
            let line = self.read_line()?;
            if let Some(reply) = reply_of(&line) {
                return reply.map_err(|why| io::Error::other(format!("QEMU {command}: {why}")));
            }
        }
    }

    /// Asks virtio-mem for `bytes` above the VM's starting size.
    pub fn set_plugged(&mut self, bytes: u64) -> io::Result<()> {
        self.execute(
            "qom-set",
            Some(json!({
                "path": crate::qemu::MEMORY_DEVICE_PATH,
                "property": "requested-size",
                "value": bytes,
            })),
        )
        .map(|_| ())
    }

    /// Pauses every processor of the VM, keeping its memory.
    pub fn stop(&mut self) -> io::Result<()> {
        self.execute("stop", None).map(|_| ())
    }

    /// Resumes a paused VM.
    pub fn cont(&mut self) -> io::Result<()> {
        self.execute("cont", None).map(|_| ())
    }

    /// Presses the VM's power button, which asks the guest to power off.
    pub fn powerdown(&mut self) -> io::Result<()> {
        self.execute("system_powerdown", None).map(|_| ())
    }

    /// Ends the VM at once.
    pub fn quit(&mut self) -> io::Result<()> {
        // QEMU may close the socket before it answers.
        match self.execute("quit", None) {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::net::UnixListener;

    #[test]
    fn nfr2_qemu_errors_and_returns_are_told_apart_from_events() {
        assert_eq!(
            reply_of(&json!({"return": {}})),
            Some(Ok(json!({})))
        );
        assert_eq!(
            reply_of(&json!({"error": {"class": "GenericError", "desc": "no such device"}})),
            Some(Err("no such device".into()))
        );
        assert_eq!(reply_of(&json!({"event": "BALLOON_CHANGE"})), None);
    }

    #[test]
    fn nfr2_plugging_memory_asks_virtio_mem_for_the_new_size() {
        let dir = std::env::temp_dir().join(format!("dsa-qmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("qmp.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(b"{\"QMP\": {\"version\": {}, \"capabilities\": []}}\n")
                .unwrap();
            let mut seen = Vec::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            for _ in 0..2 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                seen.push(serde_json::from_str::<Value>(&line).unwrap());
                stream
                    .write_all(b"{\"event\": \"MEMORY_DEVICE_SIZE_CHANGE\"}\n{\"return\": {}}\n")
                    .unwrap();
            }
            let mut rest = Vec::new();
            let _ = stream.read_to_end(&mut rest);
            seen
        });
        let mut qmp = Qmp::connect(&path, Duration::from_secs(5)).unwrap();
        qmp.set_plugged(crate::qemu::plugged_bytes(512)).unwrap();
        drop(qmp);
        let seen = server.join().unwrap();
        assert_eq!(seen[0]["execute"], "qmp_capabilities");
        assert_eq!(seen[1]["execute"], "qom-set");
        assert_eq!(seen[1]["arguments"]["path"], "/machine/peripheral/dsa-mem");
        assert_eq!(seen[1]["arguments"]["property"], "requested-size");
        assert_eq!(seen[1]["arguments"]["value"], 512u64 * 1024 * 1024);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
