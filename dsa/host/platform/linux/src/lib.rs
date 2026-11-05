//! The Linux host: the Android VM over KVM and vsock, the host's devices lent to the guest,
//! and the display, input and desktop shell.
//!
//! [`compositor`] shows each Android task in a Wayland toplevel of its own, with the task's
//! frames from gfxstream's host renderer, and runs the window rules the Windows host shares
//! (`dsa_host_core::compositor`). [`input`] turns the seat's keyboard, pointer, touch, pen,
//! input method and game controllers into the guest's input. [`shell`] gives the desktop
//! what it shows of the apps: launcher entries and icons, notifications, the clipboard and
//! dropped files.
//!
//! The rules are plain functions and types, tested on every host. The parts that talk to
//! the window system, the buses and the devices build on Linux only, so the workspace builds
//! everywhere.

pub mod compositor;
#[cfg(target_os = "linux")]
pub mod devices;
pub mod input;
pub mod shell;

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
