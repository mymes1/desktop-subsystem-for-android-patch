//! The window manager's connection to its renderer process carries frames, and with some
//! frames a descriptor (a dmabuf, a memfd), which arrives with its frame.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use dsa_host_unix::fd_frames::FdConnection;
use dsa_transport::{Connection, TransportError};

const WAIT: Duration = Duration::from_secs(5);

#[test]
fn nfr5_a_frame_and_its_descriptor_arrive_together_and_in_order() {
    let (a, b) = UnixStream::pair().unwrap();
    let mut window_manager = FdConnection::new(a).unwrap();
    let renderer = FdConnection::new(b).unwrap();
    let sender = renderer.sender();

    // A frame without, one with a socket's descriptor, and one without again.
    let (mut ours, theirs) = UnixStream::pair().unwrap();
    sender.send_with_fds(b"first", &[]).unwrap();
    sender.send_with_fds(b"image", &[theirs.as_fd()]).unwrap();
    drop(theirs);
    sender.send_with_fds(b"last", &[]).unwrap();

    assert_eq!(window_manager.recv(Some(WAIT)).unwrap(), b"first");
    assert_eq!(window_manager.recv(Some(WAIT)).unwrap(), b"image");
    let fd = window_manager
        .fds()
        .lock()
        .unwrap()
        .pop_front()
        .expect("the descriptor came with its frame");
    assert_eq!(window_manager.recv(Some(WAIT)).unwrap(), b"last");
    assert!(window_manager.fds().lock().unwrap().is_empty());

    // The descriptor is the same socket: what goes in at one end comes out at the other.
    let mut received = UnixStream::from(fd);
    received.write_all(b"pixels").unwrap();
    let mut read = [0u8; 6];
    ours.read_exact(&mut read).unwrap();
    assert_eq!(&read, b"pixels");
}

#[test]
fn nfr5_a_quiet_connection_times_out_and_a_closed_one_says_so() {
    let (a, b) = UnixStream::pair().unwrap();
    let mut window_manager = FdConnection::new(a).unwrap();
    let mut renderer = FdConnection::new(b).unwrap();
    assert_eq!(
        window_manager.recv(Some(Duration::from_millis(30))),
        Err(TransportError::Timeout)
    );
    // Sends from two places share the connection.
    let mut other = renderer.try_clone_sender().unwrap();
    other.send(b"one").unwrap();
    renderer.send(b"two").unwrap();
    assert_eq!(window_manager.recv(Some(WAIT)).unwrap(), b"one");
    assert_eq!(window_manager.recv(Some(WAIT)).unwrap(), b"two");
    // The renderer dies: its end closes.
    renderer.close();
    drop(other);
    drop(renderer);
    assert_eq!(window_manager.recv(Some(WAIT)), Err(TransportError::Closed));
}
