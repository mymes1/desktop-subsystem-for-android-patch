//! Task windows on macOS: one `NSWindow` per Android task.
//!
//! Each app's windows belong to its window host ([`window_host`]), a copy of
//! `dsa-app-window` inside the app's own bundle, so the Dock shows each app as itself. The
//! window manager runs gfxstream's host renderer ([`renderer`]), which reads each task's
//! frames back as BGRA; it writes them into a shared `IOSurface` per task ([`surface`],
//! [`frame_gate`]) and the window host shows them.
//!
//! Each window's content view is backed by a `CAMetalLayer`. A frame is copied into the
//! layer's next drawable and presented; the drawable is the size of the frame's content and
//! the layer never scales it, so while the guest catches up with a resize the window shows
//! the last frame one to one rather than stretched ([`sizing`]).

pub mod frame_gate;
pub mod frames;
pub mod sizing;

#[cfg(target_os = "macos")]
pub mod metal;
#[cfg(target_os = "macos")]
pub mod renderer;
#[cfg(target_os = "macos")]
pub mod surface;
#[cfg(target_os = "macos")]
pub mod window;
#[cfg(target_os = "macos")]
pub mod window_host;
