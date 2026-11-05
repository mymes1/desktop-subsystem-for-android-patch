//! The user's device lender's sockets: where the host's service hands on the connections the
//! user's guest makes to its device ports (sound, video, cameras, sensors).
//!
//! The process that owns the VM accepts the guest's vsock connections, and the user's device
//! lender (`dsa-devices`) serves them in the user's own session. Between the two is one Unix
//! socket per port, in a folder only the user can enter. The lender takes a connection only
//! from the service (root, on Linux) or the user themself, and the service hands a guest's
//! connection only to a socket the user owns, in a folder the user owns, so one user's guest
//! never reaches another user's devices.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use dsa_transport::stream::{accept_with_timeout, FramedConnection, StreamSocket};
use dsa_transport::{Connection, Listener, TransportError};

use crate::socket::peer_uid;

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

/// The socket a lender serves `port` on, in the folder `dir`.
pub fn port_socket(dir: &Path, port: u32) -> PathBuf {
    dir.join(format!("{port}.sock"))
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

    /// Where it listens.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DeviceSocketListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Whether a peer may hand the lender a guest's connection: the service (root), or the user.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dsa-device-socket-{name}-{}", std::process::id()))
    }

    #[test]
    fn fr26_only_the_service_and_the_user_reach_the_lender() {
        assert!(peer_allowed(0, 1000));
        assert!(peer_allowed(1000, 1000));
        assert!(!peer_allowed(1001, 1000));
    }

    #[test]
    fn fr26_a_socket_of_another_user_is_not_used() {
        let dir = scratch("owner");
        let path = port_socket(&dir, 5568);
        let listener = DeviceSocketListener::bind(&path).unwrap();
        // SAFETY: geteuid cannot fail.
        let me = unsafe { libc::geteuid() };
        assert!(owned_by(&path, me).is_ok());
        assert!(owned_by(&path, me + 1).is_err());
        assert!(owned_by(&dir.join("missing.sock"), me).is_err());
        drop(listener);
        assert!(!path.exists(), "the socket outlived its listener");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fr26_the_lenders_folder_is_the_users_alone() {
        let dir = scratch("mode");
        let listener = DeviceSocketListener::bind(&port_socket(&dir, 5572)).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let mode = fs::symlink_metadata(listener.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        drop(listener);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fr26_the_lender_takes_framed_connections_from_its_user() {
        let dir = scratch("framed");
        let path = port_socket(&dir, 5580);
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

    #[test]
    fn fr26_a_file_that_is_not_a_socket_is_never_replaced() {
        let dir = scratch("file");
        fs::create_dir_all(&dir).unwrap();
        let path = port_socket(&dir, 5569);
        fs::write(&path, b"keep").unwrap();
        assert!(DeviceSocketListener::bind(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"keep");
        let _ = fs::remove_dir_all(dir);
    }
}
