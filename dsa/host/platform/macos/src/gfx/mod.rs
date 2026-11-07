//! The graphics renderer in a process of its own on macOS: `dsa-gfx`.
//!
//! The window manager (`dsa-wm`) used to load gfxstream itself, so a crash in the renderer or
//! in MoltenVK under it ended the window manager and every app window with it. Now the window
//! manager starts `dsa-gfx` from beside itself ([`client`]); the renderer process
//! ([`process`]) loads `libgfxstream_backend.dylib`, serves the guest's graphics streams that
//! the agent relays to its Unix sockets, and copies each task's frames on the GPU into the
//! window manager's `IOSurface`s.
//!
//! A surface crosses once: as a Mach port through the window manager's Mach service (the
//! renderer says hello to it, as the window hosts do), or, where there is no service, as a
//! global surface found by its ID. Every later frame names it by its ID; nothing crosses the
//! CPU. When the renderer ends, the window manager and the app windows stay with their last
//! frame, and the window manager starts it again (see
//! [`dsa_host_core::renderer::supervise`]); the guest's streams drop with the old process and
//! the guest connects again.

pub mod client;
pub mod process;
