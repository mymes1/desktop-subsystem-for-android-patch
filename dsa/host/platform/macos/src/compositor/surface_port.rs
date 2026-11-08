//! Handing a task's frame surfaces from the window manager to a window host as Mach ports.
//!
//! An `IOSurface` made global can be looked up by its ID from any process, which macOS keeps
//! only for compatibility and which lets any process read any task's frames. A surface can
//! instead travel as a Mach send right (`IOSurfaceCreateMachPort`), which only the process
//! it is sent to holds. Mach rights cannot cross a Unix socket, so the window manager has a
//! Mach service of its own, registered by its launchd agent (`MachServices`), named
//! [`crate::launchd::WM_LABEL`]:
//!
//! 1. A window host looks the service up, makes a port of its own, and sends the service a
//!    hello that carries a send right to that port.
//! 2. The window manager takes the hello only from a process of its own user, and files the
//!    port under the sender's process ID, which the kernel attaches to the message (its
//!    audit trailer) and which the sender cannot forge. The window host gave the same ID in
//!    its `Attach`.
//! 3. For each new surface the window manager sends the window host's port a message that
//!    carries the surface's right and the texture ID its `FrameTexture` names.
//!
//! Where the service is missing (the window manager runs by hand, outside launchd) the window
//! manager falls back to global surfaces and their IDs.

use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::surface::Surface;

type MachPort = u32;
type KernReturn = i32;

extern "C" {
    static bootstrap_port: MachPort;
    static mach_task_self_: MachPort;
    fn bootstrap_check_in(bp: MachPort, name: *const c_char, port: *mut MachPort) -> KernReturn;
    fn bootstrap_look_up(bp: MachPort, name: *const c_char, port: *mut MachPort) -> KernReturn;
    fn mach_port_allocate(task: MachPort, right: u32, name: *mut MachPort) -> KernReturn;
    fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
    fn mach_port_mod_refs(task: MachPort, name: MachPort, right: u32, delta: i32) -> KernReturn;
    fn mach_msg(
        message: *mut Header,
        option: i32,
        send_size: u32,
        receive_size: u32,
        receive_name: MachPort,
        timeout: u32,
        notify: MachPort,
    ) -> KernReturn;
    fn mach_msg_destroy(message: *mut Header);
}

const KERN_SUCCESS: KernReturn = 0;
const MACH_PORT_NULL: MachPort = 0;
const MACH_PORT_RIGHT_RECEIVE: u32 = 1;
const MACH_MSG_TYPE_MOVE_SEND: u8 = 17;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const MACH_MSG_TYPE_MAKE_SEND: u8 = 20;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
const MACH_MSG_PORT_DESCRIPTOR: u8 = 0;
const MACH_SEND_MSG: i32 = 0x1;
const MACH_RCV_MSG: i32 = 0x2;
const MACH_SEND_TIMEOUT: i32 = 0x10;
/// `MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT)`: the trailer carries the sender's
/// audit token.
const MACH_RCV_TRAILER_AUDIT: i32 = 3 << 24;

/// A window host's hello: the descriptor is its port.
const HELLO_ID: i32 = 0x4453_4101;
/// A surface for a window host: the descriptor is the surface, the value its texture ID.
const SURFACE_ID: i32 = 0x4453_4102;

/// How long a send to a window host may wait for room in its queue.
const SEND_TIMEOUT_MS: u32 = 1000;

/// `mach_msg_header_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Header {
    bits: u32,
    size: u32,
    remote: MachPort,
    local: MachPort,
    voucher: MachPort,
    id: i32,
}

/// `mach_msg_port_descriptor_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct PortDescriptor {
    name: MachPort,
    pad1: u32,
    pad2: u16,
    disposition: u8,
    kind: u8,
}

/// The one message both sides send: a header, one port, and a value. Packed to four bytes,
/// as the Mach headers lay messages out.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
struct PortMessage {
    header: Header,
    descriptor_count: u32,
    port: PortDescriptor,
    value: u64,
}

/// Room for a received message and its trailer, aligned for the header.
#[repr(C, align(8))]
struct ReceiveBuffer {
    bytes: [u8; 256],
}

fn task_self() -> MachPort {
    // SAFETY: a variable libSystem sets before main and never changes.
    unsafe { mach_task_self_ }
}

fn deallocate(port: MachPort) {
    if port != MACH_PORT_NULL {
        // SAFETY: drops one reference to a right this process holds.
        unsafe { mach_port_deallocate(task_self(), port) };
    }
}

fn service_name(name: &str) -> Result<CString, String> {
    CString::new(name).map_err(|_| "the Mach service name has a NUL in it".to_string())
}

/// Sends `port` (with `disposition`) and `value` to `remote`. False when it did not go; the
/// right is then still the caller's.
fn send(remote: MachPort, id: i32, port: MachPort, disposition: u8, value: u64) -> bool {
    let mut message = PortMessage {
        header: Header {
            bits: MACH_MSGH_BITS_COMPLEX | MACH_MSG_TYPE_COPY_SEND,
            size: std::mem::size_of::<PortMessage>() as u32,
            remote,
            local: MACH_PORT_NULL,
            voucher: MACH_PORT_NULL,
            id,
        },
        descriptor_count: 1,
        port: PortDescriptor {
            name: port,
            pad1: 0,
            pad2: 0,
            disposition,
            kind: MACH_MSG_PORT_DESCRIPTOR,
        },
        value,
    };
    let size = std::mem::size_of::<PortMessage>() as u32;
    // SAFETY: a whole message of `size` bytes that lives through the call.
    let sent = unsafe {
        mach_msg(
            std::ptr::addr_of_mut!(message).cast::<Header>(),
            MACH_SEND_MSG | MACH_SEND_TIMEOUT,
            size,
            0,
            MACH_PORT_NULL,
            SEND_TIMEOUT_MS,
            MACH_PORT_NULL,
        )
    };
    sent == KERN_SUCCESS
}

/// A message taken from `port`: its ID, the port it carries, its value, and the sender's
/// user and process IDs from the audit trailer. `None` when the receive failed. A message of
/// another shape is destroyed, with any right it carried.
fn receive(port: MachPort) -> Option<Result<Received, ()>> {
    let mut buffer = ReceiveBuffer { bytes: [0; 256] };
    let header = buffer.bytes.as_mut_ptr().cast::<Header>();
    // SAFETY: the buffer is aligned for a header and holds `bytes.len()` bytes.
    let result = unsafe {
        mach_msg(
            header,
            MACH_RCV_MSG | MACH_RCV_TRAILER_AUDIT,
            0,
            buffer.bytes.len() as u32,
            port,
            0,
            MACH_PORT_NULL,
        )
    };
    if result != KERN_SUCCESS {
        return None;
    }
    // SAFETY: the kernel wrote a header at the start of the buffer.
    let head = unsafe { std::ptr::read(header) };
    let size = head.size as usize;
    let whole = std::mem::size_of::<PortMessage>();
    let complex = head.bits & MACH_MSGH_BITS_COMPLEX != 0;
    // The audit trailer: type, size, sequence number, the security token (2 words), then the
    // audit token (8 words), whose second word is the effective user ID and sixth the
    // process ID.
    let trailer = size.next_multiple_of(4);
    if !complex || size != whole || trailer + 52 > buffer.bytes.len() {
        // SAFETY: a message the kernel just delivered into this buffer.
        unsafe { mach_msg_destroy(header) };
        return Some(Err(()));
    }
    // SAFETY: a message of exactly this shape is in the buffer.
    let message = unsafe { std::ptr::read_unaligned(header.cast::<PortMessage>()) };
    let word = |index: usize| {
        let at = trailer + index * 4;
        u32::from_ne_bytes([
            buffer.bytes[at],
            buffer.bytes[at + 1],
            buffer.bytes[at + 2],
            buffer.bytes[at + 3],
        ])
    };
    let descriptor = message.port;
    let count = message.descriptor_count;
    if count != 1 || descriptor.kind != MACH_MSG_PORT_DESCRIPTOR {
        // SAFETY: as above.
        unsafe { mach_msg_destroy(header) };
        return Some(Err(()));
    }
    Some(Ok(Received {
        id: head.id,
        port: descriptor.name,
        value: message.value,
        uid: word(5 + 1),
        pid: word(5 + 5),
    }))
}

struct Received {
    id: i32,
    port: MachPort,
    value: u64,
    uid: u32,
    pid: u32,
}

/// The window manager's side: the window hosts' ports, by their process IDs.
pub struct SurfacePorts {
    hosts: Mutex<HashMap<u32, MachPort>>,
    arrived: Condvar,
}

impl SurfacePorts {
    /// Takes the window manager's Mach service from launchd and listens on it. An error when
    /// launchd has no such service for this process, as when it was started by hand.
    pub fn check_in(name: &str) -> Result<Arc<SurfacePorts>, String> {
        let c_name = service_name(name)?;
        let mut service = MACH_PORT_NULL;
        // SAFETY: a NUL-terminated name and an out pointer that outlive the call.
        let result = unsafe { bootstrap_check_in(bootstrap_port, c_name.as_ptr(), &mut service) };
        if result != KERN_SUCCESS {
            return Err(format!(
                "launchd has no Mach service {name} for the window manager (error {result}); \
                 frames are shared by global surface IDs"
            ));
        }
        let ports = Arc::new(SurfacePorts {
            hosts: Mutex::new(HashMap::new()),
            arrived: Condvar::new(),
        });
        let listening = Arc::clone(&ports);
        std::thread::Builder::new()
            .name("dsa-wm-surface-ports".into())
            .spawn(move || listening.listen(service))
            .map_err(|e| format!("the surface service could not start: {e}"))?;
        Ok(ports)
    }

    fn listen(&self, service: MachPort) {
        // SAFETY: plain libc call.
        let own_uid = unsafe { libc::geteuid() };
        loop {
            let Some(received) = receive(service) else {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            let Ok(received) = received else {
                continue;
            };
            if received.id != HELLO_ID || received.uid != own_uid || received.pid == 0 {
                // Not a window host of this user: its right is dropped.
                deallocate(received.port);
                continue;
            }
            let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(old) = hosts.insert(received.pid, received.port) {
                deallocate(old);
            }
            self.arrived.notify_all();
        }
    }

    /// Sends `surface` to the window host whose process is `pid`, as texture `texture_id`,
    /// waiting at most `wait` for that window host's hello. False when it cannot be sent;
    /// the surface must then go another way.
    pub fn send(&self, pid: u32, texture_id: u64, surface: &Surface, wait: Duration) -> bool {
        // SAFETY: `surface` keeps its IOSurfaceRef alive through the call.
        unsafe { self.send_raw(pid, texture_id, surface.as_raw(), wait) }
    }

    /// [`SurfacePorts::send`] for a surface known by its `IOSurfaceRef`, which the caller
    /// keeps alive through the call: the graphics renderer process gets each task window's
    /// surface this way.
    ///
    /// # Safety
    ///
    /// `surface` must be a live `IOSurfaceRef` for the whole call.
    pub unsafe fn send_raw(
        &self,
        pid: u32,
        texture_id: u64,
        surface: *mut std::ffi::c_void,
        wait: Duration,
    ) -> bool {
        let deadline = Instant::now() + wait;
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        let remote = loop {
            if let Some(port) = hosts.get(&pid) {
                break *port;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            hosts = self
                .arrived
                .wait_timeout(hosts, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        };
        drop(hosts);
        // SAFETY: a live IOSurfaceRef, as the caller promises.
        let right = unsafe { Surface::mach_port_of(surface) };
        if right == MACH_PORT_NULL {
            return false;
        }
        if send(
            remote,
            SURFACE_ID,
            right,
            MACH_MSG_TYPE_MOVE_SEND,
            texture_id,
        ) {
            return true;
        }
        deallocate(right);
        // The window host is gone or stuck: its port is of no more use.
        self.forget(pid);
        false
    }

    /// The window host whose process is `pid` went away.
    pub fn forget(&self, pid: u32) {
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(port) = hosts.remove(&pid) {
            deallocate(port);
        }
    }
}

/// The window host's side: says hello to the window manager's service and hands each surface
/// that arrives to `arrived`, with its texture ID. An error when the service is not there, as
/// when the window manager runs by hand; the surfaces then come by their global IDs.
pub fn receive_surfaces(
    name: &str,
    arrived: impl Fn(u64, Surface) + Send + 'static,
) -> Result<(), String> {
    let c_name = service_name(name)?;
    let mut service = MACH_PORT_NULL;
    // SAFETY: a NUL-terminated name and an out pointer that outlive the call.
    let result = unsafe { bootstrap_look_up(bootstrap_port, c_name.as_ptr(), &mut service) };
    if result != KERN_SUCCESS {
        return Err(format!(
            "the window manager's Mach service {name} is not there"
        ));
    }
    let mut own = MACH_PORT_NULL;
    // SAFETY: an out pointer that outlives the call.
    let made = unsafe { mach_port_allocate(task_self(), MACH_PORT_RIGHT_RECEIVE, &mut own) };
    if made != KERN_SUCCESS {
        deallocate(service);
        return Err(format!(
            "no Mach port for the frame surfaces (error {made})"
        ));
    }
    let said = send(service, HELLO_ID, own, MACH_MSG_TYPE_MAKE_SEND, 0);
    deallocate(service);
    if !said {
        // SAFETY: drops the receive right made above.
        unsafe { mach_port_mod_refs(task_self(), own, MACH_PORT_RIGHT_RECEIVE, -1) };
        return Err("the window manager's Mach service did not take the hello".into());
    }
    std::thread::Builder::new()
        .name("dsa-app-window-surfaces".into())
        .spawn(move || loop {
            let Some(received) = receive(own) else {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            let Ok(received) = received else {
                continue;
            };
            if received.id != SURFACE_ID {
                deallocate(received.port);
                continue;
            }
            let surface = Surface::from_mach_port(received.port);
            deallocate(received.port);
            match surface {
                Ok(surface) => arrived(received.value, surface),
                Err(why) => eprintln!("dsa-app-window: {why}"),
            }
        })
        .map_err(|e| format!("the frame surfaces cannot be received: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr3_a_surface_message_is_laid_out_as_mach_expects() {
        assert_eq!(std::mem::size_of::<Header>(), 24);
        assert_eq!(std::mem::size_of::<PortDescriptor>(), 12);
        assert_eq!(std::mem::size_of::<PortMessage>(), 48);
    }
}
