//! The transport over the connections the Virtualization framework hands the host.
//!
//! The framework gives each of the guest's connections to a host port as a connected
//! stream socket. A socket pair stands in for one here: one end goes to the port's channel,
//! as the listener delegate sends it, and the other is the guest's side.
#![cfg(target_os = "macos")]

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dsa_host_macos::vsock::{Framing, VsockListener, VsockStream};
use dsa_host_macos::vz::PortChannels;
use dsa_transport::stream::FramedConnection;
use dsa_transport::{Connection, Connector, Listener, TransportError};

/// The guest's side: each connect makes a pair and hands the host's end to the channel.
struct GuestDialer(Sender<OwnedFd>);

impl Connector for GuestDialer {
    fn connect(&mut self, _timeout: Duration) -> Result<Box<dyn Connection>, TransportError> {
        let (host, guest) = UnixStream::pair()?;
        self.0
            .send(OwnedFd::from(host))
            .map_err(|_| TransportError::Closed)?;
        Ok(Box::new(FramedConnection::new(VsockStream::from_fd(
            OwnedFd::from(guest),
        ))))
    }
}

fn vsock_pair() -> (Box<dyn Listener>, Box<dyn Connector>) {
    let (tx, rx) = mpsc::channel();
    (
        Box::new(VsockListener::new(Arc::new(Mutex::new(rx)), Framing::Framed)),
        Box::new(GuestDialer(tx)),
    )
}

dsa_transport::transport_conformance!(vsock_pair());

#[test]
fn pr1_each_port_has_one_channel_across_boots() {
    let channels = PortChannels::default();
    let first = channels.receiver(5000);
    let again = channels.receiver(5000);
    assert!(Arc::ptr_eq(&first, &again));
    assert!(!Arc::ptr_eq(&first, &channels.receiver(5001)));
}

#[test]
fn fr29_the_file_server_port_carries_a_plain_byte_stream() {
    let (tx, rx) = mpsc::channel();
    let mut listener = VsockListener::new(Arc::new(Mutex::new(rx)), Framing::Raw);
    let (host, mut guest) = UnixStream::pair().unwrap();
    tx.send(OwnedFd::from(host)).unwrap();
    let mut connection = listener.accept(Duration::from_secs(5)).unwrap();
    std::io::Write::write_all(&mut guest, b"9P2000").unwrap();
    let got = connection.recv(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(got, b"9P2000");
}

#[test]
fn nfr5_a_listener_with_no_guest_times_out() {
    let (_tx, rx) = mpsc::channel::<OwnedFd>();
    let mut listener = VsockListener::new(Arc::new(Mutex::new(rx)), Framing::Framed);
    assert!(matches!(
        listener.accept(Duration::from_millis(50)),
        Err(TransportError::Timeout)
    ));
}
