//! The control channel's transport on macOS: vsock connections the Virtualization framework
//! hands over as file descriptors.
//!
//! The guest dials the host's ports (the control channel, the log port, file sharing, the
//! graphics and device ports) as it does on Hyper-V: AF_VSOCK to the host, CID 2. The
//! framework delivers each connection to the listener of its port as a connected stream
//! socket; [`VsockListener`] takes them from the port's channel, so one listener serves
//! every boot of the user's VM. The host dials the guest's ports (adb, Android's files)
//! through [`crate::vz::VirtualMachine::connect`].

use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dsa_host_core::adb_bridge::BridgeStream;
use dsa_transport::stream::{FramedConnection, RawStreamConnection, StreamSocket};
use dsa_transport::{Connection, Listener, TransportError};

/// One vsock connection. The descriptor is a connected stream socket, which `UnixStream`
/// reads, writes, times and shuts down whatever its family.
pub struct VsockStream(UnixStream);

impl VsockStream {
    pub fn from_fd(fd: OwnedFd) -> VsockStream {
        VsockStream(UnixStream::from(fd))
    }
}

impl Read for VsockStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for VsockStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl StreamSocket for VsockStream {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(timeout)
    }
    fn shutdown(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
    fn try_clone(&self) -> io::Result<Self> {
        self.0.try_clone().map(VsockStream)
    }
}

impl BridgeStream for VsockStream {
    fn try_clone_stream(&self) -> io::Result<Box<dyn BridgeStream>> {
        Ok(Box::new(VsockStream(self.0.try_clone()?)))
    }
    fn shutdown_stream(&self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

/// How a listener frames what arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Length-prefixed frames, as the control channel and the log port speak.
    Framed,
    /// A plain byte stream, as the file server's 9P is.
    Raw,
}

/// The guest's connections to one host port.
pub struct VsockListener {
    connections: Arc<Mutex<Receiver<OwnedFd>>>,
    framing: Framing,
}

impl VsockListener {
    pub fn new(connections: Arc<Mutex<Receiver<OwnedFd>>>, framing: Framing) -> VsockListener {
        VsockListener {
            connections,
            framing,
        }
    }
}

impl Listener for VsockListener {
    fn accept(&mut self, timeout: Duration) -> Result<Box<dyn Connection>, TransportError> {
        let fd = {
            let connections = self.connections.lock().unwrap_or_else(|e| e.into_inner());
            match connections.recv_timeout(timeout) {
                Ok(fd) => fd,
                Err(RecvTimeoutError::Timeout) => return Err(TransportError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(TransportError::Closed),
            }
        };
        let stream = VsockStream::from_fd(fd);
        Ok(match self.framing {
            Framing::Framed => Box::new(FramedConnection::new(stream)),
            Framing::Raw => Box::new(RawStreamConnection::new(stream)),
        })
    }
}
