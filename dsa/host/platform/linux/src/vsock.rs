//! The host's end of vsock (`AF_VSOCK` over vhost-vsock): the guest's ports, served by address.
//!
//! Every VM on the machine reaches the host at CID 2, on the same ports, so one socket per
//! port serves all of them. A [`VsockHub`] accepts on a port and hands each connection to the
//! listener registered for the CID it comes from, which the kernel reports and the guest
//! cannot choose: vhost-vsock stamps every packet with the CID crosvm was given. A connection
//! from a CID nobody registered is closed at once.
//!
//! Each user's session has a [`CidListener`] for its VM's CID on each port it serves, so one
//! user's guest never reaches another user's session.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use dsa_host_core::adb_bridge::BridgeStream;
use dsa_transport::stream::{FramedConnection, RawStreamConnection, StreamSocket};
use dsa_transport::{Connection, Listener, TransportError};

/// Any CID: a listener bound to it accepts from every guest.
pub const VMADDR_CID_ANY: u32 = libc::VMADDR_CID_ANY;
/// This machine, through the vsock loopback transport (tests).
pub const VMADDR_CID_LOCAL: u32 = 1;

fn check(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

fn address(cid: u32, port: u32) -> libc::sockaddr_vm {
    // SAFETY: sockaddr_vm is plain data; zero is a valid starting value.
    let mut address: libc::sockaddr_vm = unsafe { zeroed() };
    address.svm_family = libc::AF_VSOCK as libc::sa_family_t;
    address.svm_cid = cid;
    address.svm_port = port;
    address
}

/// Turns a socket creation failure into `Unsupported` when the kernel has no vsock.
fn creation_error(error: io::Error) -> TransportError {
    match error.raw_os_error() {
        Some(libc::EAFNOSUPPORT) | Some(libc::EPROTONOSUPPORT) => TransportError::Unsupported(
            format!("this kernel has no vsock support ({error}); load the vhost_vsock module"),
        ),
        _ => error.into(),
    }
}

/// A connected vsock stream.
pub struct VsockStream(OwnedFd);

impl VsockStream {
    fn socket() -> Result<OwnedFd, TransportError> {
        // SAFETY: plain socket creation; the result is checked.
        let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        let fd = check(fd).map_err(creation_error)?;
        // SAFETY: fd was just created and nothing else owns it.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    /// Connects to `port` of the guest at `cid`, waiting at most `timeout`.
    pub fn connect(cid: u32, port: u32, timeout: Duration) -> io::Result<VsockStream> {
        let fd = VsockStream::socket().map_err(|e| io::Error::other(e.to_string()))?;
        set_nonblocking(fd.as_raw_fd(), true)?;
        let address = address(cid, port);
        // SAFETY: address is a valid sockaddr_vm of the size passed.
        let result = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_vm).cast(),
                size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error);
            }
            let mut poll = libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            let millis = timeout.as_millis().min(libc::c_int::MAX as u128) as libc::c_int;
            // SAFETY: one valid pollfd.
            if check(unsafe { libc::poll(&mut poll, 1, millis) })? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("vsock port {port} of the guest at {cid} did not answer"),
                ));
            }
            let mut status: libc::c_int = 0;
            let mut len = size_of::<libc::c_int>() as libc::socklen_t;
            // SAFETY: SO_ERROR writes one c_int.
            check(unsafe {
                libc::getsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&mut status as *mut libc::c_int).cast(),
                    &mut len,
                )
            })?;
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status));
            }
        }
        set_nonblocking(fd.as_raw_fd(), false)?;
        Ok(VsockStream(fd))
    }
}

fn set_nonblocking(fd: RawFd, nonblocking: bool) -> io::Result<()> {
    // SAFETY: F_GETFL/F_SETFL on a descriptor the caller owns.
    let flags = check(unsafe { libc::fcntl(fd, libc::F_GETFL) })?;
    let flags = if nonblocking {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    // SAFETY: as above.
    check(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) })?;
    Ok(())
}

impl Read for VsockStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: buf is valid for buf.len() bytes.
        let read = unsafe { libc::recv(self.fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(read as usize)
    }
}

impl Write for VsockStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // MSG_NOSIGNAL: a closed peer is an error to report, not a SIGPIPE.
        // SAFETY: buf is valid for buf.len() bytes.
        let written = unsafe {
            libc::send(
                self.fd(),
                buf.as_ptr().cast(),
                buf.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if written < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl StreamSocket for VsockStream {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        // A zero timeval means no timeout.
        let timeout = timeout.unwrap_or(Duration::ZERO);
        let value = libc::timeval {
            tv_sec: timeout.as_secs() as libc::time_t,
            tv_usec: timeout.subsec_micros() as libc::suseconds_t,
        };
        // SAFETY: the option value is a timeval that outlives the call.
        check(unsafe {
            libc::setsockopt(
                self.fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&value as *const libc::timeval).cast(),
                size_of::<libc::timeval>() as libc::socklen_t,
            )
        })?;
        Ok(())
    }

    fn shutdown(&mut self) {
        // SAFETY: shutting down an owned descriptor.
        unsafe { libc::shutdown(self.fd(), libc::SHUT_RDWR) };
    }

    fn try_clone(&self) -> io::Result<VsockStream> {
        Ok(VsockStream(self.0.try_clone()?))
    }
}

impl BridgeStream for VsockStream {
    fn try_clone_stream(&self) -> io::Result<Box<dyn BridgeStream>> {
        Ok(Box::new(StreamSocket::try_clone(self)?))
    }

    fn shutdown_stream(&self) {
        // SAFETY: shutting down an owned descriptor; every handle on it sees the end.
        unsafe { libc::shutdown(self.fd(), libc::SHUT_RDWR) };
    }
}

/// Opens a plain byte stream to `port` of the guest at `cid`, such as its adbd for developer
/// mode's bridge.
pub fn connect_stream(cid: u32, port: u32, timeout: Duration) -> io::Result<Box<dyn BridgeStream>> {
    Ok(Box::new(VsockStream::connect(cid, port, timeout)?))
}

/// How many accepted connections may wait for their session.
const QUEUE: usize = 16;

/// One port of the host, served for every guest, each connection handed to its CID's listener.
pub struct VsockHub {
    port: u32,
    routes: Mutex<HashMap<u32, (u64, SyncSender<VsockStream>)>>,
    generations: AtomicU64,
}

impl VsockHub {
    /// Listens on `port` for connections addressed to `cid` (usually [`VMADDR_CID_ANY`]) and
    /// routes them from a thread of its own.
    pub fn bind(cid: u32, port: u32) -> Result<Arc<VsockHub>, TransportError> {
        let fd = VsockStream::socket()?;
        let address = address(cid, port);
        // SAFETY: address is a valid sockaddr_vm of the size passed.
        check(unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_vm).cast(),
                size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        })?;
        // SAFETY: listening on a bound socket.
        check(unsafe { libc::listen(fd.as_raw_fd(), libc::SOMAXCONN) })?;
        let hub = Arc::new(VsockHub {
            port,
            routes: Mutex::new(HashMap::new()),
            generations: AtomicU64::new(0),
        });
        let routing = Arc::clone(&hub);
        std::thread::Builder::new()
            .name(format!("dsa-vsock-{port}"))
            .spawn(move || routing.route(fd))
            .map_err(TransportError::from)?;
        Ok(hub)
    }

    /// The hub of `port` for every guest, made on first use.
    pub fn shared(port: u32) -> Result<Arc<VsockHub>, TransportError> {
        static HUBS: OnceLock<Mutex<HashMap<u32, Arc<VsockHub>>>> = OnceLock::new();
        let mut hubs = HUBS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(hub) = hubs.get(&port) {
            return Ok(Arc::clone(hub));
        }
        let hub = VsockHub::bind(VMADDR_CID_ANY, port)?;
        hubs.insert(port, Arc::clone(&hub));
        Ok(hub)
    }

    pub fn port(&self) -> u32 {
        self.port
    }

    fn route(&self, fd: OwnedFd) {
        loop {
            // SAFETY: sockaddr_vm is plain data; accept4 fills it in.
            let mut peer: libc::sockaddr_vm = unsafe { zeroed() };
            let mut len = size_of::<libc::sockaddr_vm>() as libc::socklen_t;
            // SAFETY: the peer buffer is a sockaddr_vm of the length passed.
            let accepted = unsafe {
                libc::accept4(
                    fd.as_raw_fd(),
                    (&mut peer as *mut libc::sockaddr_vm).cast(),
                    &mut len,
                    libc::SOCK_CLOEXEC,
                )
            };
            if accepted < 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EINTR) | Some(libc::ECONNABORTED) | Some(libc::EAGAIN) => continue,
                    _ => {
                        eprintln!("vsock port {}: accepting failed: {error}", self.port);
                        std::thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                }
            }
            // SAFETY: accept4 returned a descriptor nothing else owns.
            let stream = VsockStream(unsafe { OwnedFd::from_raw_fd(accepted) });
            let cid = peer.svm_cid;
            let route = self
                .routes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&cid)
                .map(|(_, sender)| sender.clone());
            match route {
                // A full queue means the session is not taking connections; this one goes.
                Some(route) => {
                    let _ = route.try_send(stream);
                }
                None => eprintln!(
                    "vsock port {}: closed a connection from the guest at {cid}, which no \
                     session runs",
                    self.port
                ),
            }
        }
    }

    /// Takes the connections that come from the guest at `cid`, framed (the control channel
    /// and the log port) or as plain bytes (`raw`, the file server). A listener registered
    /// for the same CID before is replaced.
    pub fn listener(self: &Arc<Self>, cid: u32, raw: bool) -> CidListener {
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        let generation = self.generations.fetch_add(1, Ordering::Relaxed);
        self.routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(cid, (generation, sender));
        CidListener {
            hub: Arc::clone(self),
            cid,
            generation,
            raw,
            receiver,
        }
    }
}

/// The connections of one port that come from one guest.
pub struct CidListener {
    hub: Arc<VsockHub>,
    cid: u32,
    generation: u64,
    raw: bool,
    receiver: Receiver<VsockStream>,
}

impl CidListener {
    /// The next connection from this listener's guest as the plain stream, for a port the
    /// service passes on whole (the device ports, relayed to the user's device lender).
    pub fn accept_stream(&mut self, timeout: Duration) -> Result<VsockStream, TransportError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(stream) => Ok(stream),
            Err(RecvTimeoutError::Timeout) => Err(TransportError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(TransportError::Closed),
        }
    }
}

impl Listener for CidListener {
    fn accept(&mut self, timeout: Duration) -> Result<Box<dyn Connection>, TransportError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(stream) => Ok(if self.raw {
                Box::new(RawStreamConnection::new(stream))
            } else {
                Box::new(FramedConnection::new(stream))
            }),
            Err(RecvTimeoutError::Timeout) => Err(TransportError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(TransportError::Closed),
        }
    }
}

impl Drop for CidListener {
    fn drop(&mut self) {
        let mut routes = self.hub.routes.lock().unwrap_or_else(|e| e.into_inner());
        // Only its own route: a newer listener for the same guest keeps its own.
        if routes
            .get(&self.cid)
            .is_some_and(|(generation, _)| *generation == self.generation)
        {
            routes.remove(&self.cid);
        }
    }
}
