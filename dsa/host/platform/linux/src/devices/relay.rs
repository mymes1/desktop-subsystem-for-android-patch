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

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use dsa_transport::stream::StreamSocket;
use dsa_transport::TransportError;

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

pub use dsa_host_unix::device_socket::{owned_by, peer_allowed, DeviceSocketListener};

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
}
