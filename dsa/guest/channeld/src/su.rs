//! An unprivileged client for the guest's elevation helper.

#[cfg(any(target_os = "android", target_os = "linux"))]
fn main() {
    let status = match client() {
        Ok(status) => status,
        Err(why) => {
            eprintln!("su: {why}");
            1
        }
    };
    std::process::exit(status);
}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
fn main() {
    eprintln!("su: this client runs only in the Android guest");
    std::process::exit(1);
}

#[cfg(any(target_os = "android", target_os = "linux"))]
fn client() -> Result<i32, Box<dyn std::error::Error>> {
    use dsa_guest_channeld::su_args::{parse, SuCommand};
    use dsa_guest_channeld::su_wire::{self, Frame};
    use std::io::{self, IsTerminal, Read, Write};
    use std::os::unix::net::UnixStream;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = parse(&args)?;
    let interactive = io::stdin().is_terminal();
    let mut start = vec![u8::from(interactive)];
    // SAFETY: zero is a valid initial winsize and ioctl writes the structure.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if interactive {
        unsafe { libc::ioctl(0, libc::TIOCGWINSZ as _, &mut size) };
    }
    match command {
        SuCommand::Shell => start.push(0),
        SuCommand::Command(command) => {
            start.push(1);
            start.extend_from_slice(&size.ws_row.to_be_bytes());
            start.extend_from_slice(&size.ws_col.to_be_bytes());
            start.extend_from_slice(command.as_bytes());
        }
    }
    if start[1] == 0 {
        start.extend_from_slice(&size.ws_row.to_be_bytes());
        start.extend_from_slice(&size.ws_col.to_be_bytes());
    }
    let mut socket = UnixStream::connect("/dev/socket/dsa_su")?;
    su_wire::write_frame(
        &mut socket,
        &Frame {
            kind: su_wire::START,
            payload: start,
        },
    )?;
    let _terminal = if interactive {
        Some(Terminal::raw()?)
    } else {
        None
    };
    let mut input_socket = socket.try_clone()?;
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        let mut bytes = [0; 8192];
        loop {
            match input.read(&mut bytes) {
                Ok(0) => {
                    let _ = su_wire::write_frame(
                        &mut input_socket,
                        &Frame {
                            kind: su_wire::STDIN_EOF,
                            payload: Vec::new(),
                        },
                    );
                    break;
                }
                Ok(n) => {
                    if su_wire::write_frame(
                        &mut input_socket,
                        &Frame {
                            kind: su_wire::STDIN,
                            payload: bytes[..n].to_vec(),
                        },
                    )
                    .is_err()
                    {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
    loop {
        let frame = su_wire::read_frame(&mut socket)?;
        match frame.kind {
            su_wire::STDOUT => io::stdout().lock().write_all(&frame.payload)?,
            su_wire::STDERR => io::stderr().lock().write_all(&frame.payload)?,
            su_wire::EXIT if frame.payload.len() == 4 => {
                let status = i32::from_be_bytes(frame.payload[..].try_into()?);
                return Ok(status.clamp(0, 255));
            }
            _ => return Err("the elevation helper sent an invalid response".into()),
        }
    }
}

#[cfg(any(target_os = "android", target_os = "linux"))]
struct Terminal(libc::termios);

#[cfg(any(target_os = "android", target_os = "linux"))]
impl Terminal {
    fn raw() -> std::io::Result<Self> {
        // SAFETY: tcgetattr writes one initialized termios structure.
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut original) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut raw = original;
        // SAFETY: raw points to a valid termios, and fd 0 is the terminal.
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(original))
    }
}

#[cfg(any(target_os = "android", target_os = "linux"))]
impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: the saved attributes came from this terminal.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) };
    }
}
