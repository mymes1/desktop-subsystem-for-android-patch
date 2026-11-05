//! The Linux compositor: one Wayland toplevel per Android task.
//!
//! - [`toplevel`]: a task window's sizes, scale, mode and title bar, with no Wayland in it.
//! - [`frames`]: the task frames the renderer reads back, on their way to the windows.
//! - [`wayland`]: the toplevels, their buffers and the seat (Linux only).
//! - [`gfxstream`]: the host renderer and the guest's graphics streams (Linux only).
//! - [`session`]: what ties them to the DSA service (Linux only).

pub mod frames;
pub mod toplevel;

#[cfg(target_os = "linux")]
pub mod gfxstream;
#[cfg(target_os = "linux")]
pub mod session;
#[cfg(target_os = "linux")]
pub mod wayland;
