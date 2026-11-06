//! The launcher against a real QEMU under KVM: the command line it builds is one QEMU takes,
//! and the VM can be resized, frozen, thawed and stopped through QMP.
//!
//! It needs QEMU, `/dev/kvm`, `/dev/vhost-vsock` and a kernel to boot, so it runs only when
//! `DSA_QEMU_TEST_KERNEL` names that kernel; CI installs QEMU and sets it. The kernel need not
//! reach Android: the VM is paused as soon as QEMU answers.
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::time::Duration;

use dsa_host_core::session::GuestLauncher;
use dsa_host_linux::image::ImageFiles;
use dsa_host_linux::launcher::{BootPlan, LauncherSetup, LinuxVmLauncher};
use dsa_host_linux::qemu::{guest_cid, GuestArch};
use dsa_host_linux::users::Account;
use dsa_protocol::auth::BootSecret;

#[test]
fn fr2_qemu_starts_resizes_freezes_and_stops_the_vm() {
    let Some(kernel) = std::env::var_os("DSA_QEMU_TEST_KERNEL").map(PathBuf::from) else {
        eprintln!("skipped: DSA_QEMU_TEST_KERNEL names no kernel to boot");
        return;
    };
    let dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("qemu-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // QEMU's QMP socket is a Unix socket, whose path must fit in 108 bytes. A deep checkout
    // (a CI runner's work directory) pushes `target/` past that, so the run directory moves
    // to the temporary directory when the checkout's own would not fit.
    let mut run_dir = dir.join("run");
    if run_dir.join("qmp.sock").as_os_str().len() >= 108 {
        run_dir = std::env::temp_dir().join(format!("dsa-qemu-{}", std::process::id()));
    }
    let _ = std::fs::remove_dir_all(&run_dir);
    let initrd = dir.join("initrd.img");
    std::fs::write(&initrd, b"070701").unwrap();
    let account = Account::current().unwrap();
    let qemu = dsa_host_linux::network::find_program(GuestArch::HOST.qemu_program())
        .expect("QEMU is installed");
    let disk = dir.join("data.qcow2");
    dsa_host_linux::image::make_disk(&account, &disk, None, 1).unwrap();
    let files = ImageFiles {
        kernel,
        initrd: Some(initrd),
        kernel_cmdline: "androidboot.boot_devices=vmbus".into(),
        system_image: None,
        disk_template: None,
    };
    let mut launcher = LinuxVmLauncher::new(
        LauncherSetup {
            account: account.clone(),
            name: "dsa-test".into(),
            // Clear of the addresses the service gives users' VMs.
            guest_cid: guest_cid(account.uid).wrapping_add(0x1000_0000),
            qemu,
            processors: 1,
            page_reporting_order: 9,
            data_disk: disk,
            pstore_disk: Some(dir.join("pstore.img")),
            run_dir: run_dir.clone(),
            console_log: dir.join("console.log"),
            grow_memory: true,
        },
        Box::new(move || {
            Ok(BootPlan {
                files: files.clone(),
                image_version: Some("test".into()),
                memory_mb: 512,
                memory_max_mb: Some(1024),
            })
        }),
    );
    assert_eq!(launcher.prerequisite(), None);
    let secret = BootSecret::generate().unwrap();
    launcher
        .start(&secret)
        .unwrap_or_else(|why| panic!("{why}\n{:?}", launcher.diagnostics()));
    launcher.freeze().unwrap();
    assert!(launcher.vm_id().is_some());
    assert_eq!(launcher.memory_range_mb(), Some((512, 1024)));
    launcher.set_memory_mb(768).unwrap();
    launcher.set_memory_mb(512).unwrap();
    launcher.thaw().unwrap();
    launcher.stop(Duration::from_secs(5)).unwrap();
    assert!(launcher.vm_id().is_none());
    // The boot's initrd, which carried the secret, is gone.
    let left: Vec<_> = std::fs::read_dir(&run_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .filter(|name| name.to_string_lossy().ends_with(".initrd"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    let console = std::fs::read_to_string(dir.join("console.log")).unwrap();
    assert!(console.contains("==== DSA boot at"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&run_dir);
}
