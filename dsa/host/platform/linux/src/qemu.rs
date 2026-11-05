//! The Android VM as a QEMU command line, under KVM.
//!
//! The VMM is QEMU, launched as its own process (`qemu-system-x86_64` or
//! `qemu-system-aarch64`). Every distribution packages it, it boots a Linux kernel directly
//! on both architectures, and it has every device this VM needs:
//! - the kernel booted directly (`-kernel`, `-initrd`, `-append`), straight into Android's
//!   init, as the Windows host does with `LinuxKernelDirect`;
//! - the image's disks on virtio-blk, at fixed PCI addresses. Android's first stage init makes
//!   `/dev/block/by-name` links only for disks below the devices `androidboot.boot_devices`
//!   names, so the command line names those addresses in place of the image's `vmbus`. The
//!   partitions themselves are found by the disk's GPT GUID, whatever the bus;
//! - vsock through the kernel's vhost-vsock, so the guest reaches the host at CID 2 on the same
//!   ports as on Hyper-V (5000 control, 5001 log, 5590 and 5591 file sharing, 5555 adb), and
//!   the host's `AF_VSOCK` sockets serve it (see `vsock`);
//! - memory that returns to the host: a virtio balloon with free page reporting, so the pages
//!   the guest frees are handed back to the host, and, when the VM may grow, a virtio-mem
//!   device the host plugs memory into while the guest is short of it and takes back after.
//!   Nothing pins guest memory: no preallocation, no huge pages, no locked memory;
//! - a virtio network adapter on a user-mode NAT (passt, or QEMU's own slirp), or on a
//!   macvtap on the host's LAN for advanced networking (see `network`);
//! - the guest's console on the first serial port (COM1, or the PL011 UART of the `virt`
//!   machine), which the host keeps in the console log;
//! - QMP on a Unix socket, through which the host resizes memory, freezes and stops the VM.
//!
//! QEMU is GPL-2.0. DSA runs it as a separate program and links none of it, so DSA's own code
//! stays Apache-2.0 and QEMU comes from the distribution's package.
//!
//! This module only builds the command line, so it is tested on every platform.

use std::fmt;
use std::path::{Path, PathBuf};

/// The architecture of the guest, which is the host's: KVM runs guests of its own
/// architecture only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestArch {
    /// The `q35` machine, a `bzImage`, the console on COM1.
    X86_64,
    /// The `virt` machine, an `Image`, the console on its PL011 UART.
    Arm64,
}

/// The PCI slots of the VM's devices on the root bus (`pcie.0`), fixed so the guest finds its
/// disks at the same place at every boot.
pub mod slot {
    pub const NETWORK: u8 = 0x03;
    pub const BALLOON: u8 = 0x04;
    pub const VSOCK: u8 = 0x05;
    pub const MEMORY: u8 = 0x06;
    pub const SYSTEM_DISK: u8 = 0x10;
    pub const DATA_DISK: u8 = 0x11;
    pub const PSTORE_DISK: u8 = 0x12;
}

/// The platform device of the `virt` machine's PCIe host bridge, as Linux names it from the
/// device tree: its ECAM above 4 GiB (`highmem-ecam`, which the command line turns on), and
/// below for a host whose address space is too small for it.
const ARM64_PCIE_DEVICES: &str = "4010000000.pcie,3f000000.pcie";

impl GuestArch {
    /// The architecture this host was built for.
    pub const HOST: GuestArch = if cfg!(target_arch = "aarch64") {
        GuestArch::Arm64
    } else {
        GuestArch::X86_64
    };

    /// The QEMU program for this architecture.
    pub fn qemu_program(self) -> &'static str {
        match self {
            GuestArch::X86_64 => "qemu-system-x86_64",
            GuestArch::Arm64 => "qemu-system-aarch64",
        }
    }

    /// The kernel's console from the first instruction: COM1 (I/O port 0x3f8) on `q35`, the
    /// PL011 UART at 0x09000000 on `virt`.
    pub fn console_cmdline(self) -> &'static str {
        match self {
            GuestArch::X86_64 => "earlycon=uart8250,io,0x3f8,115200 console=ttyS0,115200",
            GuestArch::Arm64 => "earlycon=pl011,0x09000000 console=ttyAMA0",
        }
    }

    /// The machine and its options.
    fn machine(self) -> &'static str {
        match self {
            // No legacy devices the guest has no use for.
            GuestArch::X86_64 => "q35,accel=kvm,usb=off,vmport=off,smm=off,dump-guest-core=off",
            // The host's interrupt controller, and the ECAM where `boot_devices` expects it.
            GuestArch::Arm64 => {
                "virt,accel=kvm,gic-version=host,highmem-ecam=on,dump-guest-core=off"
            }
        }
    }

    /// What `androidboot.boot_devices` names for disks in `slots`.
    pub fn boot_devices(self, slots: &[u8]) -> String {
        match self {
            GuestArch::X86_64 => slots
                .iter()
                .map(|slot| format!("pci0000:00/0000:00:{slot:02x}.0"))
                .collect::<Vec<_>>()
                .join(","),
            // Below the `virt` machine's host bridge, the first platform device up the path.
            GuestArch::Arm64 => ARM64_PCIE_DEVICES.to_string(),
        }
    }
}

/// The format of a disk file, from its extension: what the image ships (`.vhdx`), what the
/// Linux package may convert it to (`.img`, raw), and each user's disk (`.qcow2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskFormat {
    Raw,
    Qcow2,
    Vhdx,
}

impl DiskFormat {
    pub fn of(path: &Path) -> DiskFormat {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("qcow2") => DiskFormat::Qcow2,
            Some("vhdx") => DiskFormat::Vhdx,
            _ => DiskFormat::Raw,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DiskFormat::Raw => "raw",
            DiskFormat::Qcow2 => "qcow2",
            DiskFormat::Vhdx => "vhdx",
        }
    }
}

/// The VM's network adapter, and what it is connected to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkBackend {
    /// No adapter.
    None,
    /// A passt process serving user-mode NAT on this Unix socket.
    Passt { socket: PathBuf },
    /// QEMU's own user-mode NAT (slirp).
    User,
    /// A tap device (a macvtap on the host's LAN) open as this file descriptor in QEMU.
    Tap { fd: i32 },
}

/// Everything needed to describe the Android VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmConfig {
    /// Shown as the process's name and in diagnostics.
    pub name: String,
    pub arch: GuestArch,
    /// The guest kernel, with binder built in.
    pub kernel: PathBuf,
    /// The boot's initrd, with its bootconfig.
    pub initrd: PathBuf,
    /// The image's kernel command line, after the settings this module adds.
    pub kernel_cmdline: String,
    /// The memory the VM starts with, in MiB.
    pub memory_mb: u32,
    /// How far the VM may grow while it runs, in MiB; `None` keeps it at `memory_mb`.
    pub memory_max_mb: Option<u32>,
    pub processors: u32,
    /// The Android system disk, attached read only. Images with one disk have none.
    pub system_image: Option<PathBuf>,
    /// The user's disk, attached writable with discard, so freed blocks leave the file.
    pub data_disk: PathBuf,
    /// The small raw disk the guest kernel keeps its last log on (pstore).
    pub pstore_disk: Option<PathBuf>,
    /// The guest's vsock address. Unique on the machine while the VM runs.
    pub guest_cid: u32,
    /// Free page reporting granularity, as a page order (9 is 2 MiB).
    pub page_reporting_order: u8,
    /// Where QEMU serves QMP.
    pub qmp_socket: PathBuf,
    pub network: NetworkBackend,
    /// The adapter's MAC address.
    pub mac_address: String,
}

/// Why a configuration cannot describe a VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// The pstore/blk settings for the kernel command line: the partition by its UUID, the same
/// disk layout as on every host (see `pstore`).
pub const PSTORE_CMDLINE: &str = "pstore_blk.blkdev=PARTUUID=5f8b3c2e-9a41-4d6e-b07c-1e2d3f4a5b6c \
     pstore_blk.best_effort=y pstore_blk.kmsg_size=64 pstore_blk.console_size=256 \
     pstore_blk.pmsg_size=256";

/// What the host writes to the console's input to stop the guest in order: each SysRq comes
/// after a serial BREAK (the multiplexer's Ctrl-A b): `s` syncs every file system, `u` remounts
/// them read only, `o` powers the VM off. It is what the Hyper-V host's shutdown request does
/// in the guest kernel (a sync, then power off), with the file systems left clean as well.
pub const SHUTDOWN_SEQUENCE: [&[u8]; 3] = [b"\x01bs", b"\x01bu", b"\x01bo"];

/// The virtio-mem device's ID, and its path for `qom-set`.
pub const MEMORY_DEVICE: &str = "dsa-mem";
pub const MEMORY_DEVICE_PATH: &str = "/machine/peripheral/dsa-mem";
/// virtio-mem's block size: memory is plugged and unplugged 2 MiB at a time.
pub const MEMORY_BLOCK_MB: u32 = 2;

/// Writes `value` as one QEMU option value: a comma is doubled.
pub fn escape(value: &str) -> String {
    value.replace(',', ",,")
}

fn path_value(path: &Path) -> String {
    escape(&path.to_string_lossy())
}

/// Replaces `androidboot.boot_devices=...` in `cmdline` with `devices`, or adds it.
pub fn with_boot_devices(cmdline: &str, devices: &str) -> String {
    let mut parts: Vec<String> = cmdline
        .split_whitespace()
        .filter(|part| !part.starts_with("androidboot.boot_devices="))
        .map(str::to_string)
        .collect();
    parts.push(format!("androidboot.boot_devices={devices}"));
    parts.join(" ")
}

impl VmConfig {
    /// The slots of the disks the VM has, in order.
    pub fn disk_slots(&self) -> Vec<u8> {
        let mut slots = Vec::new();
        if self.system_image.is_some() {
            slots.push(slot::SYSTEM_DISK);
        }
        slots.push(slot::DATA_DISK);
        if self.pstore_disk.is_some() {
            slots.push(slot::PSTORE_DISK);
        }
        slots
    }

    /// The kernel command line the VM boots with: the console, the memory settings, pstore,
    /// then the image's own with the disks' devices in place of Hyper-V's.
    pub fn full_kernel_cmdline(&self) -> String {
        // SysRq on the serial console is how the host asks the guest to sync its disks and power
        // off when it stops the VM (see `SHUTDOWN_SEQUENCE`).
        let mut cmdline = format!(
            "{} panic=-1 sysrq_always_enabled=1 page_reporting.page_reporting_order={}",
            self.arch.console_cmdline(),
            self.page_reporting_order
        );
        if self.memory_max_mb.is_some() {
            // Memory virtio-mem plugs in is onlined at once, movable so it can be unplugged.
            cmdline.push_str(" memhp_default_state=online_movable");
        }
        if self.pstore_disk.is_some() {
            cmdline.push(' ');
            cmdline.push_str(PSTORE_CMDLINE);
        }
        let image = with_boot_devices(
            self.kernel_cmdline.trim(),
            &self.arch.boot_devices(&self.disk_slots()),
        );
        cmdline.push(' ');
        cmdline.push_str(&image);
        cmdline
    }

    fn check(&self) -> Result<(), ConfigError> {
        for (what, path) in [
            ("kernel", Some(&self.kernel)),
            ("initrd", Some(&self.initrd)),
            ("system image", self.system_image.as_ref()),
            ("data disk", Some(&self.data_disk)),
            ("pstore disk", self.pstore_disk.as_ref()),
            ("QMP socket", Some(&self.qmp_socket)),
        ] {
            if let Some(path) = path {
                if !path.is_absolute() {
                    return Err(ConfigError(format!(
                        "the {what} path {} is not absolute",
                        path.display()
                    )));
                }
            }
        }
        if self.processors == 0 {
            return Err(ConfigError("the VM needs at least one processor".into()));
        }
        if self.memory_mb < 256 {
            return Err(ConfigError(format!(
                "{} MiB is too little memory for Android",
                self.memory_mb
            )));
        }
        if self.guest_cid <= 2 || self.guest_cid == u32::MAX {
            return Err(ConfigError(format!(
                "{} is not a guest's vsock address",
                self.guest_cid
            )));
        }
        Ok(())
    }

    /// The memory QEMU is started with, and the part of it virtio-mem can plug, in MiB.
    fn memory_layout(&self) -> (u32, Option<u32>) {
        let base = self.memory_mb - self.memory_mb % MEMORY_BLOCK_MB;
        let growth = self
            .memory_max_mb
            .map(|max| max - max % MEMORY_BLOCK_MB)
            .filter(|max| *max > base)
            .map(|max| max - base);
        (base, growth)
    }

    /// QEMU's arguments, after the program name.
    pub fn qemu_args(&self) -> Result<Vec<String>, ConfigError> {
        self.check()?;
        let mut args: Vec<String> = Vec::new();
        let mut push = |option: &str, value: String| {
            args.push(option.to_string());
            args.push(value);
        };
        push(
            "-name",
            format!("guest={},debug-threads=on", escape(&self.name)),
        );
        push("-machine", self.arch.machine().to_string());
        push("-cpu", "host".into());
        push("-smp", self.processors.to_string());
        let (base, growth) = self.memory_layout();
        match growth {
            Some(growth) => {
                push("-m", format!("size={base}M,maxmem={}M", base + growth));
                push(
                    "-object",
                    format!("memory-backend-ram,id={MEMORY_DEVICE}-backend,size={growth}M"),
                );
                push(
                    "-device",
                    format!(
                        "virtio-mem-pci,id={MEMORY_DEVICE},memdev={MEMORY_DEVICE}-backend,\
                         block-size={MEMORY_BLOCK_MB}M,requested-size=0,bus=pcie.0,addr={:#04x}",
                        slot::MEMORY
                    ),
                );
            }
            None => push("-m", format!("{base}M")),
        }
        push("-kernel", path_value(&self.kernel));
        push("-initrd", path_value(&self.initrd));
        push("-append", self.full_kernel_cmdline());
        // The console goes to QEMU's standard output, which the host reads. Its input is
        // multiplexed, so the host can send the serial BREAK that a stop's SysRq follows.
        push("-chardev", "stdio,id=console,signal=off,mux=on".into());
        push("-serial", "chardev:console".into());
        push(
            "-qmp",
            format!("unix:{},server=on,wait=off", path_value(&self.qmp_socket)),
        );
        push(
            "-device",
            format!(
                "vhost-vsock-pci,guest-cid={},bus=pcie.0,addr={:#04x}",
                self.guest_cid,
                slot::VSOCK
            ),
        );
        push(
            "-device",
            format!(
                "virtio-balloon-pci,id=balloon,free-page-reporting=on,bus=pcie.0,addr={:#04x}",
                slot::BALLOON
            ),
        );
        if let Some(system) = &self.system_image {
            push(
                "-drive",
                format!(
                    "file={},format={},if=none,id=system,readonly=on,cache=writeback,aio=threads",
                    path_value(system),
                    DiskFormat::of(system).name()
                ),
            );
            push(
                "-device",
                format!(
                    "virtio-blk-pci,drive=system,bus=pcie.0,addr={:#04x}",
                    slot::SYSTEM_DISK
                ),
            );
        }
        push(
            "-drive",
            format!(
                "file={},format={},if=none,id=data,cache=writeback,aio=threads,discard=unmap,\
                 detect-zeroes=unmap",
                path_value(&self.data_disk),
                DiskFormat::of(&self.data_disk).name()
            ),
        );
        push(
            "-device",
            format!(
                "virtio-blk-pci,drive=data,bus=pcie.0,addr={:#04x}",
                slot::DATA_DISK
            ),
        );
        if let Some(pstore) = &self.pstore_disk {
            push(
                "-drive",
                format!(
                    "file={},format=raw,if=none,id=pstore,cache=writethrough",
                    path_value(pstore)
                ),
            );
            push(
                "-device",
                format!(
                    "virtio-blk-pci,drive=pstore,bus=pcie.0,addr={:#04x}",
                    slot::PSTORE_DISK
                ),
            );
        }
        let netdev = match &self.network {
            NetworkBackend::None => None,
            NetworkBackend::Passt { socket } => Some(format!(
                "stream,id=net,server=off,addr.type=unix,addr.path={}",
                path_value(socket)
            )),
            NetworkBackend::User => Some("user,id=net,ipv6=on".to_string()),
            NetworkBackend::Tap { fd } => Some(format!("tap,id=net,fd={fd}")),
        };
        if let Some(netdev) = netdev {
            push("-netdev", netdev);
            push(
                "-device",
                format!(
                    "virtio-net-pci,netdev=net,mac={},bus=pcie.0,addr={:#04x}",
                    self.mac_address,
                    slot::NETWORK
                ),
            );
        }
        push("-rtc", "base=utc,clock=host".into());
        push("-msg", "timestamp=on".into());
        // No display: each Android window is a window of its own on the host.
        push("-display", "none".into());
        args.extend(
            [
                "-nodefaults",
                "-no-user-config",
                // A guest that reboots (a panic, with panic=-1) ends the VM, as on Hyper-V; the
                // next start is a new VM.
                "-no-reboot",
            ]
            .map(str::to_string),
        );
        Ok(args)
    }
}

/// A locally administered MAC address for the VM of user `uid`, stable across boots so the
/// guest keeps its DHCP lease and Android sees the same network.
pub fn mac_address(uid: u32) -> String {
    let b = uid.to_be_bytes();
    format!("52:54:{:02x}:{:02x}:{:02x}:{:02x}", b[0], b[1], b[2], b[3])
}

/// The vsock address of the VM of user `uid`. Each user has their own, so the host tells
/// their guests apart by the address a connection comes from; it stays clear of the low
/// addresses other VM managers hand out from 3 upward.
pub fn guest_cid(uid: u32) -> u32 {
    const BASE: u32 = 0x0044_5341; // "DSA"
    let cid = BASE.wrapping_add(uid);
    if cid <= 2 || cid == u32::MAX {
        BASE
    } else {
        cid
    }
}

/// The `qom-set` value that asks virtio-mem for `mb` MiB above the VM's starting size, in
/// bytes, rounded down to its block size.
pub fn plugged_bytes(mb: u64) -> u64 {
    let mb = mb - mb % u64::from(MEMORY_BLOCK_MB);
    mb * 1024 * 1024
}

// Linux paths: "/a" is not absolute on Windows, where the workspace is also tested.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn config() -> VmConfig {
        VmConfig {
            name: "dsa-1000".into(),
            arch: GuestArch::X86_64,
            kernel: "/var/lib/dsa/images/1/bzImage".into(),
            initrd: "/home/u/.local/share/dsa/tmp/boot.initrd".into(),
            kernel_cmdline:
                "console=ttyS0 androidboot.boot_devices=vmbus androidboot.hardware=cutf_cvm".into(),
            memory_mb: 3072,
            memory_max_mb: None,
            processors: 4,
            system_image: Some("/var/lib/dsa/images/1/system.vhdx".into()),
            data_disk: "/home/u/.local/share/dsa/data.qcow2".into(),
            pstore_disk: Some("/home/u/.local/share/dsa/pstore.img".into()),
            guest_cid: guest_cid(1000),
            page_reporting_order: 9,
            qmp_socket: "/home/u/.local/share/dsa/tmp/qmp.sock".into(),
            network: NetworkBackend::Passt {
                socket: "/home/u/.local/share/dsa/tmp/passt.sock".into(),
            },
            mac_address: mac_address(1000),
        }
    }

    fn value_of<'a>(args: &'a [String], option: &str) -> Vec<&'a str> {
        args.windows(2)
            .filter(|pair| pair[0] == option)
            .map(|pair| pair[1].as_str())
            .collect()
    }

    #[test]
    fn fr2_the_vm_boots_the_kernel_directly_under_kvm() {
        let args = config().qemu_args().unwrap();
        assert_eq!(
            value_of(&args, "-kernel"),
            ["/var/lib/dsa/images/1/bzImage"]
        );
        assert_eq!(
            value_of(&args, "-initrd"),
            ["/home/u/.local/share/dsa/tmp/boot.initrd"]
        );
        assert!(value_of(&args, "-machine")[0].contains("accel=kvm"));
        assert_eq!(value_of(&args, "-cpu"), ["host"]);
        assert!(args.contains(&"-no-reboot".to_string()));
    }

    #[test]
    fn fr2_the_disks_are_found_at_the_pci_addresses_the_command_line_names() {
        let config = config();
        let cmdline = config.full_kernel_cmdline();
        assert!(!cmdline.contains("boot_devices=vmbus"));
        assert!(cmdline.contains(
            "androidboot.boot_devices=pci0000:00/0000:00:10.0,pci0000:00/0000:00:11.0,\
             pci0000:00/0000:00:12.0"
        ));
        let args = config.qemu_args().unwrap();
        let devices = value_of(&args, "-device");
        assert!(devices.contains(&"virtio-blk-pci,drive=system,bus=pcie.0,addr=0x10"));
        assert!(devices.contains(&"virtio-blk-pci,drive=data,bus=pcie.0,addr=0x11"));
        assert!(devices.contains(&"virtio-blk-pci,drive=pstore,bus=pcie.0,addr=0x12"));
        let drives = value_of(&args, "-drive");
        assert!(drives[0].contains("format=vhdx") && drives[0].contains("readonly=on"));
        assert!(drives[1].contains("format=qcow2") && drives[1].contains("discard=unmap"));
    }

    #[test]
    fn fr2_an_arm64_guest_boots_on_the_virt_machine_with_its_uart_console() {
        let mut config = config();
        config.arch = GuestArch::Arm64;
        let cmdline = config.full_kernel_cmdline();
        assert!(cmdline.starts_with("earlycon=pl011,0x09000000 console=ttyAMA0"));
        assert!(cmdline.contains("androidboot.boot_devices=4010000000.pcie"));
        let args = config.qemu_args().unwrap();
        assert!(value_of(&args, "-machine")[0].starts_with("virt,"));
    }

    #[test]
    fn pr1_the_guest_has_a_vhost_vsock_device_at_the_users_address() {
        let args = config().qemu_args().unwrap();
        let cid = guest_cid(1000);
        assert!(value_of(&args, "-device")
            .contains(&format!("vhost-vsock-pci,guest-cid={cid},bus=pcie.0,addr=0x05").as_str()));
        assert_ne!(guest_cid(1000), guest_cid(1001));
        assert!(guest_cid(u32::MAX) > 2);
    }

    #[test]
    fn nfr2_free_guest_pages_are_reported_back_to_the_host() {
        let config = config();
        let args = config.qemu_args().unwrap();
        assert!(value_of(&args, "-device")
            .iter()
            .any(|d| d.starts_with("virtio-balloon-pci") && d.contains("free-page-reporting=on")));
        assert!(config
            .full_kernel_cmdline()
            .contains("page_reporting.page_reporting_order=9"));
        // Nothing that pins or preallocates the guest's memory.
        let joined = args.join(" ");
        for pinned in [
            "mem-prealloc",
            "prealloc=on",
            "mlock",
            "hugepages",
            "share=on",
        ] {
            assert!(!joined.contains(pinned), "{pinned} pins guest memory");
        }
    }

    #[test]
    fn nfr2_a_vm_that_may_grow_starts_small_and_gets_memory_plugged_in() {
        let mut config = config();
        config.memory_max_mb = Some(16385);
        let args = config.qemu_args().unwrap();
        assert_eq!(value_of(&args, "-m"), ["size=3072M,maxmem=16384M"]);
        assert!(value_of(&args, "-object")
            .contains(&"memory-backend-ram,id=dsa-mem-backend,size=13312M"));
        assert!(value_of(&args, "-device")
            .iter()
            .any(|d| d.starts_with("virtio-mem-pci,id=dsa-mem") && d.contains("requested-size=0")));
        assert!(config
            .full_kernel_cmdline()
            .contains("memhp_default_state=online_movable"));
        assert_eq!(plugged_bytes(1025), 1024 * 1024 * 1024);
    }

    #[test]
    fn fr28_the_vm_has_a_network_adapter_on_the_nat_it_was_given() {
        let args = config().qemu_args().unwrap();
        assert_eq!(
            value_of(&args, "-netdev"),
            ["stream,id=net,server=off,addr.type=unix,addr.path=/home/u/.local/share/dsa/tmp/passt.sock"]
        );
        let mut slirp = config();
        slirp.network = NetworkBackend::User;
        assert_eq!(
            value_of(&slirp.qemu_args().unwrap(), "-netdev"),
            ["user,id=net,ipv6=on"]
        );
        let mut tap = config();
        tap.network = NetworkBackend::Tap { fd: 7 };
        assert_eq!(
            value_of(&tap.qemu_args().unwrap(), "-netdev"),
            ["tap,id=net,fd=7"]
        );
        let mut none = config();
        none.network = NetworkBackend::None;
        assert!(value_of(&none.qemu_args().unwrap(), "-netdev").is_empty());
        assert_eq!(mac_address(1000), "52:54:00:00:03:e8");
    }

    #[test]
    fn fr2_a_stop_can_ask_the_guest_to_sync_and_power_off() {
        let config = config();
        assert!(config
            .full_kernel_cmdline()
            .contains("sysrq_always_enabled=1"));
        let args = config.qemu_args().unwrap();
        assert!(value_of(&args, "-chardev")[0].contains("mux=on"));
        assert_eq!(SHUTDOWN_SEQUENCE[2], b"\x01bo");
    }

    #[test]
    fn fr2_a_path_with_a_comma_stays_one_option_value() {
        let mut config = config();
        config.data_disk = "/home/a,b/data.qcow2".into();
        let args = config.qemu_args().unwrap();
        assert!(value_of(&args, "-drive")[1].starts_with("file=/home/a,,b/data.qcow2,"));
    }

    #[test]
    fn fr2_a_relative_path_is_refused() {
        let mut config = config();
        config.kernel = "bzImage".into();
        assert!(config.qemu_args().is_err());
    }

    #[test]
    fn fr19_disk_formats_come_from_the_extension() {
        assert_eq!(
            DiskFormat::of(Path::new("/a/system.vhdx")),
            DiskFormat::Vhdx
        );
        assert_eq!(
            DiskFormat::of(Path::new("/a/data.QCOW2")),
            DiskFormat::Qcow2
        );
        assert_eq!(DiskFormat::of(Path::new("/a/system.img")), DiskFormat::Raw);
    }
}
