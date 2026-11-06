//! The host's vsock ports over vsock loopback (CID 1), standing in for a guest.
//!
//! Loopback needs the `vsock_loopback` kernel module, so this suite runs only with the
//! `vsock-loopback-tests` feature, which CI turns on after loading the module.
#![cfg(all(target_os = "linux", feature = "vsock-loopback-tests"))]

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use dsa_host_linux::vsock::{VsockHub, VsockStream, VMADDR_CID_ANY, VMADDR_CID_LOCAL};
use dsa_transport::stream::FramedConnection;
use dsa_transport::{Connection, Connector, Listener, TransportError};

fn fresh_port() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    // Spread by process so parallel runs on one machine do not collide.
    41_000 + (std::process::id() % 1000) * 64 + NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Connects as the guest would, from CID 1 through loopback.
struct LoopbackGuest {
    port: u32,
}

impl Connector for LoopbackGuest {
    fn connect(&mut self, timeout: Duration) -> Result<Box<dyn Connection>, TransportError> {
        let stream = VsockStream::connect(VMADDR_CID_LOCAL, self.port, timeout)?;
        Ok(Box::new(FramedConnection::new(stream)))
    }
}

fn vsock_loopback() -> (Box<dyn Listener>, Box<dyn Connector>) {
    let port = fresh_port();
    let hub = VsockHub::bind(VMADDR_CID_ANY, port).expect("binding the host's vsock port");
    let listener = hub.listener(VMADDR_CID_LOCAL, false);
    (Box::new(listener), Box::new(LoopbackGuest { port }))
}

dsa_transport::transport_conformance!(vsock_loopback());

#[test]
fn pr1_a_connection_from_a_guest_no_session_runs_is_closed() {
    let port = fresh_port();
    let hub = VsockHub::bind(VMADDR_CID_ANY, port).unwrap();
    // The session is for another VM's address, so the loopback "guest" is nobody's.
    let mut other = hub.listener(0x0044_5341, false);
    let mut stream = VsockStream::connect(VMADDR_CID_LOCAL, port, Duration::from_secs(5)).unwrap();
    let mut byte = [0u8; 1];
    std::thread::sleep(Duration::from_millis(200));
    let _ = stream.write_all(b"x");
    assert_eq!(stream.read(&mut byte).unwrap_or(0), 0);
    assert!(matches!(
        other.accept(Duration::from_millis(200)),
        Err(TransportError::Timeout)
    ));
}

#[test]
fn fr29_the_file_server_port_carries_plain_bytes() {
    let port = fresh_port();
    let hub = VsockHub::bind(VMADDR_CID_ANY, port).unwrap();
    let mut listener = hub.listener(VMADDR_CID_LOCAL, true);
    let mut stream = VsockStream::connect(VMADDR_CID_LOCAL, port, Duration::from_secs(5)).unwrap();
    stream.write_all(b"9P2000").unwrap();
    let mut connection = listener.accept(Duration::from_secs(5)).unwrap();
    let received = connection.recv(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(&received[..], b"9P2000");
}

#[test]
fn pr1_a_newer_session_for_the_same_guest_takes_its_connections() {
    let port = fresh_port();
    let hub = VsockHub::bind(VMADDR_CID_ANY, port).unwrap();
    let old = hub.listener(VMADDR_CID_LOCAL, false);
    let mut new = hub.listener(VMADDR_CID_LOCAL, false);
    // The old listener going does not take the new one's route with it.
    drop(old);
    let mut guest = LoopbackGuest { port };
    let mut connection = guest.connect(Duration::from_secs(5)).unwrap();
    connection.send(b"hello").unwrap();
    let mut accepted = new.accept(Duration::from_secs(5)).unwrap();
    assert_eq!(
        &accepted.recv(Some(Duration::from_secs(5))).unwrap()[..],
        b"hello"
    );
}
