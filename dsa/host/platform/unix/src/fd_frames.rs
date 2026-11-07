//! Frames over a Unix socket that can carry file descriptors with them (`SCM_RIGHTS`): the
//! connection between the window manager and its graphics renderer process.
//!
//! The window manager makes a socket pair and hands one end to the renderer it starts. Each
//! frame is a 4-byte little-endian length and the bytes; a frame sent with descriptors carries
//! them on its first byte, so they arrive with it. The receiving side keeps every descriptor
//! that arrives, in order, in its [`FdQueue`], and the code that reads a message that says it
//! carries one takes it from there. The renderer on Linux sends each GPU image's dmabuf, and
//! each frame read back in a memfd, this way.
//!
//! Both ends implement the transport's [`Connection`], so the handshake and the messages are
//! the same as on every other connection. Several threads may send at once: every sender of
//! one connection writes under one lock, so frames never interleave.

use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsa_transport::{check_frame_len, Connection, FrameSender, TransportError};

/// The most descriptors one frame carries.
pub const MAX_FDS: usize = 8;

/// The descriptors that arrived on a connection, oldest first.
pub type FdQueue = Arc<Mutex<VecDeque<OwnedFd>>>;

/// Sends frames, with or without descriptors, on one connection. Clones share the
/// connection and its lock.
#[derive(Clone)]
pub struct FdSender {
    stream: Arc<Mutex<UnixStream>>,
}

/// One end of a connection that carries descriptors.
pub struct FdConnection {
    stream: UnixStream,
    sender: FdSender,
    pending: Vec<u8>,
    fds: FdQueue,
}

#[cfg(target_os = "linux")]
const SEND_FLAGS: libc::c_int = libc::MSG_NOSIGNAL;
// macOS has no MSG_NOSIGNAL; its programs ignore SIGPIPE instead.
#[cfg(not(target_os = "linux"))]
const SEND_FLAGS: libc::c_int = 0;

fn would_retry(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

/// Writes all of `data` to `fd`, the first write carrying `fds`.
fn send_all(fd: RawFd, data: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<()> {
    if fds.len() > MAX_FDS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a frame carries at most {MAX_FDS} descriptors"),
        ));
    }
    let mut sent = 0usize;
    let mut first = true;
    while sent < data.len() {
        let rest = &data[sent..];
        let mut iov = libc::iovec {
            iov_base: rest.as_ptr() as *mut libc::c_void,
            iov_len: rest.len(),
        };
        // Room for the descriptors, aligned for the control header.
        let mut control = [0u64; 16];
        // SAFETY: a zeroed msghdr is valid; its fields are set below.
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1 as _;
        if first && !fds.is_empty() {
            let bytes = std::mem::size_of_val(fds) as u32;
            // SAFETY: the control buffer (128 bytes) holds the `space` bytes CMSG_SPACE asks
            // for one header and at most MAX_FDS descriptors.
            unsafe {
                let space = libc::CMSG_SPACE(bytes) as usize;
                message.msg_control = control.as_mut_ptr().cast();
                message.msg_controllen = space as _;
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(bytes) as _;
                let data = libc::CMSG_DATA(header).cast::<RawFd>();
                for (index, fd) in fds.iter().enumerate() {
                    std::ptr::write_unaligned(data.add(index), fd.as_raw_fd());
                }
            }
        }
        // SAFETY: a message whose buffers outlive the call.
        let written = unsafe { libc::sendmsg(fd, &message, SEND_FLAGS) };
        if written < 0 {
            let error = io::Error::last_os_error();
            if would_retry(&error) {
                continue;
            }
            return Err(error);
        }
        first = false;
        sent += written as usize;
    }
    Ok(())
}

/// Marks `fd` close-on-exec, so a descriptor that arrived is not handed to a program this
/// process starts.
fn close_on_exec(fd: RawFd) {
    // SAFETY: plain fcntl calls on a descriptor this process owns.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

impl FdSender {
    /// Sends `frame` with `fds`, which the peer receives as descriptors of its own. The
    /// caller's descriptors stay open.
    pub fn send_with_fds(
        &self,
        frame: &[u8],
        fds: &[BorrowedFd<'_>],
    ) -> Result<(), TransportError> {
        check_frame_len(frame.len())?;
        let mut data = Vec::with_capacity(4 + frame.len());
        data.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        data.extend_from_slice(frame);
        let stream = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        send_all(stream.as_raw_fd(), &data, fds).map_err(TransportError::from)
    }
}

impl FrameSender for FdSender {
    fn send(&mut self, frame: &[u8]) -> Result<(), TransportError> {
        self.send_with_fds(frame, &[])
    }
}

impl FdConnection {
    /// The connection on `stream`, one end of a socket pair.
    pub fn new(stream: UnixStream) -> io::Result<FdConnection> {
        let sender = FdSender {
            stream: Arc::new(Mutex::new(stream.try_clone()?)),
        };
        Ok(FdConnection {
            stream,
            sender,
            pending: Vec::new(),
            fds: Arc::default(),
        })
    }

    /// A sender on this connection, which may send descriptors.
    pub fn sender(&self) -> FdSender {
        self.sender.clone()
    }

    /// Where the descriptors that arrive go.
    pub fn fds(&self) -> FdQueue {
        Arc::clone(&self.fds)
    }

    /// A whole frame from what has arrived, if one is there.
    fn take_frame(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        if self.pending.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([
            self.pending[0],
            self.pending[1],
            self.pending[2],
            self.pending[3],
        ]) as usize;
        check_frame_len(len)?;
        if self.pending.len() < 4 + len {
            return Ok(None);
        }
        let frame = self.pending[4..4 + len].to_vec();
        self.pending.drain(..4 + len);
        Ok(Some(frame))
    }

    /// Reads what has arrived, waiting at most `wait` (`None`: as long as it takes). False
    /// when nothing came in time.
    fn read_some(&mut self, wait: Option<Duration>) -> Result<bool, TransportError> {
        let fd = self.stream.as_raw_fd();
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = wait.map_or(-1, |wait| wait.as_millis().min(i32::MAX as u128) as i32);
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready == 0 {
            return Ok(false);
        }
        if ready < 0 {
            let error = io::Error::last_os_error();
            if would_retry(&error) {
                return Ok(true);
            }
            return Err(error.into());
        }
        let mut buffer = vec![0u8; 64 * 1024];
        let mut iov = libc::iovec {
            iov_base: buffer.as_mut_ptr().cast(),
            iov_len: buffer.len(),
        };
        let mut control = [0u64; 64];
        // SAFETY: a zeroed msghdr is valid; its fields are set below.
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1 as _;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = std::mem::size_of_val(&control) as _;
        // SAFETY: a message whose buffers outlive the call.
        let read = unsafe { libc::recvmsg(fd, &mut message, 0) };
        if read < 0 {
            let error = io::Error::last_os_error();
            if would_retry(&error) {
                return Ok(true);
            }
            return Err(error.into());
        }
        // The descriptors first: they belong to the bytes that came with them.
        // SAFETY: walks the control messages the kernel wrote into `control`.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                if (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                {
                    let data = libc::CMSG_DATA(header).cast::<RawFd>();
                    let bytes = (*header).cmsg_len as usize - (data as usize - header as usize);
                    let count = bytes / std::mem::size_of::<RawFd>();
                    let mut fds = self.fds.lock().unwrap_or_else(|e| e.into_inner());
                    for index in 0..count {
                        let raw = std::ptr::read_unaligned(data.add(index));
                        close_on_exec(raw);
                        fds.push_back(OwnedFd::from_raw_fd(raw));
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
        if read == 0 {
            return Err(TransportError::Closed);
        }
        self.pending.extend_from_slice(&buffer[..read as usize]);
        Ok(true)
    }
}

impl Connection for FdConnection {
    fn send(&mut self, frame: &[u8]) -> Result<(), TransportError> {
        self.sender.send_with_fds(frame, &[])
    }

    fn recv(&mut self, timeout: Option<Duration>) -> Result<Vec<u8>, TransportError> {
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            if let Some(frame) = self.take_frame()? {
                return Ok(frame);
            }
            let wait = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
            if !self.read_some(wait)? && wait.is_some() {
                return Err(TransportError::Timeout);
            }
        }
    }

    fn close(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    fn try_clone_sender(&self) -> Option<Box<dyn FrameSender>> {
        Some(Box::new(self.sender.clone()))
    }
}

/// The end of a socket pair a started program finds at descriptor `fd`.
///
/// # Safety
/// `fd` must be an open Unix socket descriptor this process owns and nothing else uses.
pub unsafe fn stream_at(fd: RawFd) -> UnixStream {
    close_on_exec(fd);
    UnixStream::from_raw_fd(fd)
}
