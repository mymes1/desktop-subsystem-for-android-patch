//! What the macOS host's VM is made of, before the framework sees it.
//!
//! The Virtualization framework's own objects are built from this in `vz`, on macOS only;
//! this module decides everything that can be decided anywhere, so it is tested on every
//! build host:
//! - the kernel the image ships for the Mac's architecture: the uncompressed `Image` on
//!   Apple silicon (the framework's Linux boot loader takes no compressed kernel there), the
//!   `bzImage` on Intel Macs;
//! - the disks, in the order the guest names them: the image's system disk read only
//!   (`vda`), the user's disk (`vdb`), the pstore disk (`vdc`). Android finds its partitions
//!   by their GPT names, not by these names;
//! - the vsock ports the host listens on and the memory the balloon leaves the guest.

use std::path::{Path, PathBuf};

/// The architecture of the guest, which is the Mac's: the framework runs guests of its own
/// architecture only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestArch {
    Arm64,
    X86_64,
}

impl GuestArch {
    /// The architecture this host was built for.
    pub const HOST: GuestArch = if cfg!(target_arch = "aarch64") {
        GuestArch::Arm64
    } else {
        GuestArch::X86_64
    };

    /// The name of the kernel an image ships for this architecture.
    pub fn kernel_name(self) -> &'static str {
        match self {
            GuestArch::Arm64 => "Image",
            GuestArch::X86_64 => "bzImage",
        }
    }

    /// The architecture an image's `image.json` names.
    pub fn of_manifest(arch: &str) -> Option<GuestArch> {
        match arch {
            "arm64" | "aarch64" => Some(GuestArch::Arm64),
            "x86_64" => Some(GuestArch::X86_64),
            _ => None,
        }
    }
}

/// Whether `kernel` starts like a kernel the framework boots on `arch`: an arm64 `Image`
/// (its magic `ARM\x64` at offset 56; a compressed kernel has none) or an x86 `bzImage`
/// (`HdrS` at offset 0x202).
pub fn kernel_fits(kernel: &[u8], arch: GuestArch) -> bool {
    match arch {
        GuestArch::Arm64 => kernel.get(56..60) == Some(&b"ARM\x64"[..]),
        GuestArch::X86_64 => kernel.get(0x202..0x206) == Some(&b"HdrS"[..]),
    }
}

/// One disk of the VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub path: PathBuf,
    pub read_only: bool,
}

/// Everything the framework's VM is made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacVmConfig {
    pub kernel: PathBuf,
    /// The boot's initrd: the image's, with the boot's secret appended as bootconfig.
    pub initrd: PathBuf,
    pub cmdline: String,
    /// The memory the VM is given, in MiB: as far as it may grow.
    pub memory_max_mb: u64,
    /// The memory the guest starts with, in MiB; the balloon holds the rest back.
    pub memory_start_mb: u64,
    pub processors: u32,
    /// In the order the guest sees them.
    pub disks: Vec<Disk>,
    /// Where the guest's console is written. Without it the VM has no console.
    pub console_log: Option<PathBuf>,
    /// Whether the VM has a network adapter, behind the Mac's NAT.
    pub network: bool,
    /// The adapter's MAC address (`aa:bb:cc:dd:ee:ff`), kept per user so Android sees the
    /// same network each boot. `None` lets the framework pick one.
    pub mac_address: Option<String>,
    /// The vsock ports the guest connects to on the host.
    pub listen_ports: Vec<u32>,
}

/// The least memory Android runs in.
pub const MIN_MEMORY_MB: u64 = 512;

impl MacVmConfig {
    /// Checks what the framework would refuse less clearly.
    pub fn validate(&self) -> Result<(), String> {
        let mut paths: Vec<(&str, &Path)> =
            vec![("kernel", &self.kernel), ("initrd", &self.initrd)];
        for disk in &self.disks {
            paths.push(("disk", &disk.path));
        }
        if let Some(log) = &self.console_log {
            paths.push(("console log", log));
        }
        for (what, path) in paths {
            if !path.is_absolute() {
                return Err(format!("the {what} path {} must be absolute", path.display()));
            }
        }
        if self.memory_max_mb < MIN_MEMORY_MB {
            return Err(format!(
                "{} MiB of memory is too little for Android (at least {MIN_MEMORY_MB})",
                self.memory_max_mb
            ));
        }
        if self.memory_start_mb < MIN_MEMORY_MB || self.memory_start_mb > self.memory_max_mb {
            return Err(format!(
                "the VM cannot start with {} MiB: it must be between {MIN_MEMORY_MB} and {} MiB",
                self.memory_start_mb, self.memory_max_mb
            ));
        }
        if self.processors == 0 {
            return Err("the VM needs at least one processor".into());
        }
        if self.disks.is_empty() {
            return Err("the VM has no disk to boot Android from".into());
        }
        if let Some(mac) = &self.mac_address {
            if !is_mac_address(mac) {
                return Err(format!("{mac:?} is not a MAC address (aa:bb:cc:dd:ee:ff)"));
            }
        }
        Ok(())
    }
}

/// Whether `text` is six hexadecimal pairs separated by colons.
pub fn is_mac_address(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A locally administered unicast MAC address made from `seed` (the user's ID), so each
/// user's Android keeps one address.
pub fn mac_address_for(seed: &str) -> String {
    // FNV-1a: stable across builds and platforms, unlike the standard library's hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let bytes = hash.to_le_bytes();
    let first = (bytes[0] & 0xfc) | 0x02;
    format!(
        "{first:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
    )
}

/// What a VM boots from one image folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFiles {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    /// The image's own kernel command line, written for Hyper-V (see `cmdline`).
    pub kernel_cmdline: String,
    /// The shared system disk of an image of two disks.
    pub system_disk: Option<PathBuf>,
    /// The template of a user's disk: `data` of an image of two disks, `disk` of one.
    pub disk_template: PathBuf,
}

/// The first of `names` in `dir` that exists, else the first name.
fn first_present(dir: &Path, names: &[&str]) -> PathBuf {
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.exists())
        .unwrap_or_else(|| dir.join(names[0]))
}

/// Reads the image in `dir` for a guest of `arch`. Raw disks (`system.img`, `data.img`,
/// `disk.img`) are used when the folder has them; otherwise its VHDX disks, which the host
/// converts.
pub fn image_files(dir: &Path, arch: GuestArch) -> Result<ImageFiles, String> {
    let text = std::fs::read_to_string(dir.join("image.json"))
        .map_err(|e| format!("reading the image's image.json in {}: {e}", dir.display()))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("the image's image.json: {e}"))?;
    if let Some(image_arch) = manifest["arch"].as_str() {
        if GuestArch::of_manifest(image_arch) != Some(arch) {
            return Err(format!(
                "the image in {} is for {image_arch}, and this Mac runs {} guests only",
                dir.display(),
                match arch {
                    GuestArch::Arm64 => "arm64",
                    GuestArch::X86_64 => "x86_64",
                }
            ));
        }
    }
    let listed = |name: &str| manifest["files"].get(name).is_some() || dir.join(name).exists();
    let two_disks = listed("system.vhdx") || listed("system.img");
    let (system_disk, disk_template) = if two_disks {
        (
            Some(first_present(dir, &["system.img", "system.vhdx"])),
            first_present(dir, &["data.img", "data.vhdx"]),
        )
    } else {
        (None, first_present(dir, &["disk.img", "disk.vhdx"]))
    };
    Ok(ImageFiles {
        kernel: dir.join(arch.kernel_name()),
        initrd: dir.join("initrd.img"),
        kernel_cmdline: manifest["kernel_cmdline"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        system_disk,
        disk_template,
    })
}
