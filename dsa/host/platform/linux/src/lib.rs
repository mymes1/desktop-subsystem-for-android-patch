//! The Linux host: the Android VM over KVM and vsock, and the host's devices lent to the
//! guest.
//!
//! Everything here is Linux only; on another system the crate is empty, so the workspace
//! builds everywhere.

#[cfg(target_os = "linux")]
pub mod devices;

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
