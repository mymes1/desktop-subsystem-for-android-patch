//! The macOS host: the Android VM on Apple's Virtualization framework.
//!
//! On macOS no privilege is needed to run a VM: a process signed with the
//! `com.apple.security.virtualization` entitlement may create one. So the host is a per-user
//! launchd agent, `dsa-hostd`, which runs as the user, owns that user's VM, and serves the
//! local API on a Unix socket in the user's own folder (`dsa-host-unix`). There is no system
//! service between the user and their Android.
//!
//! The VM boots the same image as the Windows host: the kernel (`Image` on Apple silicon,
//! `bzImage` on Intel Macs) with the image's initrd and a bootconfig carrying each boot's
//! secret, the image's system disk read only and the user's disk writable. The image ships
//! its disks as VHDX; the framework reads raw images, so the host converts them once
//! ([`vhdx`]) and keeps the result.
//!
//! What can be checked anywhere (the kernel command line, the disk formats, the
//! configuration, the launchd agent) is platform neutral and tested on every build host. The
//! parts that call the framework are compiled on macOS only.

pub mod cmdline;
pub mod host_config;
pub mod launchd;
pub mod user_disk;
pub mod vhdx;
pub mod vm_config;

/// GUID partition tables, shared with the Windows host: the same disks, read the same way.
#[allow(dead_code)]
#[path = "../../windows/src/gpt.rs"]
pub(crate) mod gpt;

/// The pstore disk, shared with the Windows host. macOS attaches its disk part, without the
/// VHD footer.
#[allow(dead_code)]
#[path = "../../windows/src/pstore_disk.rs"]
pub(crate) mod pstore_disk;

#[cfg(target_os = "macos")]
pub mod android_vm;
#[cfg(target_os = "macos")]
pub mod fetch;
#[cfg(target_os = "macos")]
pub mod host_memory;
#[cfg(target_os = "macos")]
pub mod service;
#[cfg(target_os = "macos")]
pub mod vsock;
#[cfg(target_os = "macos")]
pub mod vz;

/// This program's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
