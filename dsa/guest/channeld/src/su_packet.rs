//! The private seqpacket connection between the daemon and the elevation helper.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Mutex;

const CHUNK: usize = 60 * 1024;

/// A bounded packet connection. Message boundaries are preserved by the kernel.
pub struct Packet(OwnedFd, Mutex<()>);

impl Packet {
    /// Take ownership of an already connected socket.
    ///
    /// # Safety
    /// The descriptor must be a unique, open seqpacket socket.
    pub unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self(OwnedFd::from_raw_fd(fd), Mutex::new(()))
    }

    pub fn connect(path: &str) -> io::Result<Self> {
        let path = std::ffi::CString::new(path)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket path contains NUL"))?;
        // SAFETY: socket takes no pointers.
        let fd =
            unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd was just created and is uniquely owned.
        let socket = unsafe { Self::from_raw_fd(fd) };
        // SAFETY: zero is valid for sockaddr_un and the path has a terminating NUL.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        let bytes = path.as_bytes_with_nul();
        if bytes.len() > address.sun_path.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "socket path is too long",
            ));
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (dest, byte) in address.sun_path.iter_mut().zip(bytes) {
            *dest = *byte as libc::c_char;
        }
        // SAFETY: address is a valid initialized Unix socket address.
        let result = unsafe {
            libc::connect(
                fd,
                (&address as *const libc::sockaddr_un).cast(),
                std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(socket)
    }

    pub fn peer_uid(&self) -> io::Result<u32> {
        peer_uid(self.0.as_raw_fd())
    }

    pub fn shutdown(&self) {
        // SAFETY: shutdown operates on this open socket and takes no pointers.
        unsafe { libc::shutdown(self.0.as_raw_fd(), libc::SHUT_RDWR) };
    }

    pub fn send(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() || bytes.len() > dsa_transport::MAX_FRAME_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid packet size",
            ));
        }
        let _send = self.1.lock().unwrap_or_else(|e| e.into_inner());
        for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
            let mut packet = Vec::with_capacity(8 + chunk.len());
            packet.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            packet.extend_from_slice(&((index * CHUNK) as u32).to_be_bytes());
            packet.extend_from_slice(chunk);
            // SAFETY: packet remains readable for the duration of send.
            let n = unsafe {
                libc::send(
                    self.0.as_raw_fd(),
                    packet.as_ptr().cast(),
                    packet.len(),
                    libc::MSG_NOSIGNAL,
                )
            };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            if n as usize != packet.len() {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "short packet"));
            }
        }
        Ok(())
    }

    pub fn recv(&self) -> io::Result<Vec<u8>> {
        let mut result = Vec::new();
        let mut packet = vec![0u8; CHUNK + 9];
        let mut expected = None;
        loop {
            // SAFETY: packet is writable for its full length.
            let n = unsafe {
                libc::recv(
                    self.0.as_raw_fd(),
                    packet.as_mut_ptr().cast(),
                    packet.len(),
                    0,
                )
            };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "relay disconnected",
                ));
            }
            let n = n as usize;
            if n <= 8 || n > CHUNK + 8 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid relay packet",
                ));
            }
            let total = u32::from_be_bytes(packet[..4].try_into().unwrap()) as usize;
            let offset = u32::from_be_bytes(packet[4..8].try_into().unwrap()) as usize;
            if total == 0
                || total > dsa_transport::MAX_FRAME_LEN
                || offset != result.len()
                || offset + n - 8 > total
                || expected.is_some_and(|value| value != total)
                || (offset + n - 8 < total && n - 8 != CHUNK)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid relay frame",
                ));
            }
            expected = Some(total);
            result.extend_from_slice(&packet[8..n]);
            if result.len() == total {
                return Ok(result);
            }
        }
    }
}

/// The uid the kernel attached to the other endpoint.
pub fn peer_uid(fd: RawFd) -> io::Result<u32> {
    // SAFETY: the buffer and its length match SO_PEERCRED's result.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    if len as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid peer credentials",
        ));
    }
    Ok(cred.uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr35_the_private_relay_preserves_packets() {
        let mut fd = [-1; 2];
        // SAFETY: fd points to two writable descriptors.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0, fd.as_mut_ptr()) },
            0
        );
        // SAFETY: socketpair returned two distinct owned descriptors.
        let (a, b) = unsafe { (Packet::from_raw_fd(fd[0]), Packet::from_raw_fd(fd[1])) };
        a.send(b"first").unwrap();
        a.send(b"second").unwrap();
        assert_eq!(b.recv().unwrap(), b"first");
        assert_eq!(b.recv().unwrap(), b"second");
    }

    #[test]
    fn fr35_a_large_app_configuration_crosses_the_relay_intact() {
        let mut fd = [-1; 2];
        // SAFETY: fd points to two writable descriptors.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0, fd.as_mut_ptr()) },
            0
        );
        // SAFETY: socketpair returned two distinct owned descriptors.
        let (sender, receiver) =
            unsafe { (Packet::from_raw_fd(fd[0]), Packet::from_raw_fd(fd[1])) };
        let message = vec![0x5a; CHUNK * 4 + 3];
        let expected = message.clone();
        let worker = std::thread::spawn(move || sender.send(&message).unwrap());
        assert_eq!(receiver.recv().unwrap(), expected);
        worker.join().unwrap();
    }
}
