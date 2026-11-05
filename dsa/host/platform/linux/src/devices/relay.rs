//! The guest's device ports (sound, cameras, sensors), carried from each user's VM to that
//! user's own device lender.
//!
//! Every VM reaches the host on the same vsock ports, and only one program can listen on a
//! port, so the service listens and tells the VMs apart by the CID the kernel stamps on each
//! connection. For each user's VM it passes the device ports' connections on, byte for byte,
//! to a Unix socket in that user's runtime folder (`/run/user/<uid>/dsa/devices/<port>.sock`),
//! where the user's `dsa-devices` serves them inside the user's desktop session. So any
//! number of users can be signed in at once, each guest reaches only its own user's sound,
//! cameras and sensors, and nothing of one session reaches another.
//!
//! The lender's sockets take only the service (root) and the user themself. The service
//! connects only to a socket the user owns, in the user's own folder.

use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use dsa_transport::stream::{accept_with_timeout, FramedConnection, StreamSocket};
use dsa_transport::{Connection, Listener, TransportError};

/// The ports the service relays to each user's device lender.
pub const RELAYED_PORTS: [u32; 3] = [
    dsa_host_core::audio::AUDIO_PORT,
    dsa_host_core::camera::CAMERA_PORT,
    dsa_host_core::sensors::SENSOR_PORT,
];

/// The folder of a user's device sockets, below their runtime folder.
pub fn socket_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("dsa").join("devices")
}

/// The socket the user's lender serves `port` on.
pub fn device_socket(runtime_dir: &Path, port: u32) -> PathBuf {
    socket_dir(runtime_dir).join(format!("{port}.sock"))
}

/// The runtime folder systemd gives the user with `uid`.
pub fn user_runtime_dir(uid: u32) -> PathBuf {
    PathBuf::from(format!("/run/user/{uid}"))
}

/// The user ID of the process at the other end of `stream`.
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    // SAFETY: zero is a valid ucred; getsockopt fills it in.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED writes one ucred into a buffer of the size given.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}

/// A Unix stream as the transport's socket.
struct UnixSocket(UnixStream);

impl Read for UnixSocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for UnixSocket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl StreamSocket for UnixSocket {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(timeout)
    }
    fn shutdown(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
    fn try_clone(&self) -> io::Result<UnixSocket> {
        Ok(UnixSocket(self.0.try_clone()?))
    }
}

/// The lender's end of one relayed port: the connections the service passes on from the
/// user's guest, framed as the guest frames them.
pub struct DeviceSocketListener {
    listener: UnixListener,
    path: PathBuf,
    owner: u32,
}

impl DeviceSocketListener {
    /// Listens at `path`, in a folder only this user can enter. A socket left by an earlier
    /// run is replaced; any other file there is an error.
    pub fn bind(path: &Path) -> io::Result<DeviceSocketListener> {
        use std::os::unix::fs::FileTypeExt;
        if let Some(dir) = path.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_socket() => fs::remove_file(path)?,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a socket", path.display()),
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        // SAFETY: geteuid cannot fail.
        let owner = unsafe { libc::geteuid() };
        Ok(DeviceSocketListener {
            listener,
            path: path.to_path_buf(),
            owner,
        })
    }
}

impl Drop for DeviceSocketListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Whether a peer may hand the lender a guest's connection: the service, or the user.
pub fn peer_allowed(peer: u32, owner: u32) -> bool {
    peer == 0 || peer == owner
}

impl Listener for DeviceSocketListener {
    fn accept(&mut self, timeout: Duration) -> Result<Box<dyn Connection>, TransportError> {
        loop {
            let (stream, _) = accept_with_timeout(timeout, || self.listener.accept())?;
            stream.set_nonblocking(false)?;
            match peer_uid(&stream) {
                Ok(uid) if peer_allowed(uid, self.owner) => {
                    return Ok(Box::new(FramedConnection::new(UnixSocket(stream))));
                }
                // Anyone else is turned away, and the lender waits for the next.
                _ => drop(stream),
            }
        }
    }
}

/// Checks that `path` is a socket the user with `uid` owns, in a folder they own, so the
/// service never hands a guest's connection to another user's program.
pub fn owned_by(path: &Path, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::FileTypeExt;
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.file_type().is_socket() {
        return Err(format!("{} is not a socket", path.display()));
    }
    let dir = path.parent().map(fs::symlink_metadata);
    let dir_owner = match dir {
        Some(Ok(dir)) if dir.is_dir() => dir.uid(),
        _ => return Err(format!("{} is not in a folder", path.display())),
    };
    if meta.uid() != uid || dir_owner != uid {
        return Err(format!(
            "{} does not belong to the user {uid}; it is not used",
            path.display()
        ));
    }
    Ok(())
}

/// Copies `from` to `to` until either ends, then ends both.
fn pump(mut from: impl Read, mut to: impl Write, end: impl FnOnce()) {
    let _ = io::copy(&mut from, &mut to);
    end();
}

/// Relays the connections `listener` takes from one user's guest on `port` to that user's
/// lender, on a thread of its own, until the listener is replaced or dropped (the user's
/// Android stopped). A connection the lender does not take is closed, which the guest's HAL
/// reads as a device that is not there.
pub fn relay(
    mut listener: crate::vsock::CidListener,
    port: u32,
    uid: u32,
    user: String,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name(format!("dsa-relay-{port}"))
        .spawn(move || {
            let socket = device_socket(&user_runtime_dir(uid), port);
            let mut warned = false;
            loop {
                let guest = match listener.accept_stream(Duration::from_millis(500)) {
                    Ok(stream) => stream,
                    Err(TransportError::Timeout) => continue,
                    Err(_) => return,
                };
                let lender = owned_by(&socket, uid).and_then(|()| {
                    UnixStream::connect(&socket).map_err(|e| format!("{}: {e}", socket.display()))
                });
                let lender = match lender {
                    Ok(lender) => {
                        warned = false;
                        lender
                    }
                    Err(why) => {
                        if !warned {
                            eprintln!(
                                "{user}: vsock port {port}: the user's device lender \
                                 (dsa-devices) is not running: {why}"
                            );
                            warned = true;
                        }
                        drop(guest);
                        continue;
                    }
                };
                let (guest_in, lender_in) = match (guest.try_clone(), lender.try_clone()) {
                    (Ok(g), Ok(l)) => (g, l),
                    _ => continue,
                };
                let lender_ender = lender.try_clone();
                let _ = thread::Builder::new()
                    .name(format!("dsa-relay-{port}-in"))
                    .spawn(move || {
                        // The guest closed: the lender hears the end, and its answer ends too.
                        pump(guest_in, lender, move || {
                            if let Ok(end) = lender_ender {
                                let _ = end.shutdown(std::net::Shutdown::Both);
                            }
                        })
                    });
                let lender_end = lender_in.try_clone();
                let _ = thread::Builder::new()
                    .name(format!("dsa-relay-{port}-out"))
                    .spawn(move || {
                        let mut guest = guest;
                        pump(lender_in, &mut guest, move || {
                            if let Ok(end) = lender_end {
                                let _ = end.shutdown(std::net::Shutdown::Both);
                            }
                        });
                        StreamSocket::shutdown(&mut guest);
                    });
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_each_users_device_ports_go_to_their_own_runtime_folder() {
        assert_eq!(
            device_socket(&user_runtime_dir(1000), dsa_host_core::camera::CAMERA_PORT),
            PathBuf::from("/run/user/1000/dsa/devices/5572.sock")
        );
        assert_ne!(
            device_socket(&user_runtime_dir(1000), dsa_host_core::audio::AUDIO_PORT),
            device_socket(&user_runtime_dir(1001), dsa_host_core::audio::AUDIO_PORT)
        );
    }

    #[test]
    fn fr26_only_the_service_and_the_user_reach_the_lender() {
        assert!(peer_allowed(0, 1000));
        assert!(peer_allowed(1000, 1000));
        assert!(!peer_allowed(1001, 1000));
    }

    #[test]
    fn fr26_a_socket_of_another_user_is_not_used() {
        let dir = std::env::temp_dir().join(format!("dsa-relay-{}", std::process::id()));
        let path = dir.join("5568.sock");
        let listener = DeviceSocketListener::bind(&path).unwrap();
        // SAFETY: geteuid cannot fail.
        let me = unsafe { libc::geteuid() };
        assert!(owned_by(&path, me).is_ok());
        assert!(owned_by(&path, me + 1).is_err());
        assert!(owned_by(&dir.join("missing.sock"), me).is_err());
        drop(listener);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fr26_the_lender_takes_framed_connections_from_its_user() {
        let dir = std::env::temp_dir().join(format!("dsa-relay-f-{}", std::process::id()));
        let path = dir.join("5580.sock");
        let mut listener = DeviceSocketListener::bind(&path).unwrap();
        let mut client = UnixStream::connect(&path).unwrap();
        let mut connection = listener.accept(Duration::from_secs(5)).unwrap();
        // A frame is a 4-byte length, then the bytes.
        client.write_all(&[3, 0, 0, 0, b'a', b'b', b'c']).unwrap();
        let frame = connection.recv(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(frame, b"abc");
        drop(listener);
        let _ = fs::remove_dir_all(dir);
    }
}
