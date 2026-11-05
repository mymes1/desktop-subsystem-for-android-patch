//! The Android VM as the session's guest launcher: QEMU under KVM, run as the user it is for.
//!
//! Each boot's secret reaches the guest as a bootconfig appended to a copy of the image's
//! initrd, readable only by the user, as on every host. The copy exists only while the VM
//! starts: QEMU has read it by the time it answers on its control socket, and it is deleted
//! then. The user's device identity, when they changed it, travels in the same copy.
//!
//! Stopping asks the guest to sync its disks and power off (SysRq on the serial console, see
//! `qemu::SHUTDOWN_SEQUENCE`), ends QEMU through QMP if it has not exited within the stop
//! timeout, and kills it if that does not work either. Every guest process has ended once
//! `stop` returns `Ok`, because the VM itself is gone.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dsa_host_core::bootconfig;
use dsa_host_core::device_identity::{append_overlay, ramdisk_overlay};
use dsa_host_core::host_api::NetworkStatus;
use dsa_host_core::session::{GuestLauncher, GuestStall};
use dsa_protocol::auth::{bootconfig_entry, BootSecret};

use crate::console::{boot_header, capture, Tail};
use crate::image::ImageFiles;
use crate::network::{place_tap, VmNetwork};
use crate::qemu::{plugged_bytes, GuestArch, NetworkBackend, VmConfig, SHUTDOWN_SEQUENCE};
use crate::qmp::Qmp;
use crate::signs::{explain_unfinished_boot, explain_unreachable_agent, BootSigns};
use crate::users::{Account, BeforeExec};

/// How long QEMU may take to open its control socket.
const QMP_WAIT: Duration = Duration::from_secs(30);
/// How long QEMU may take to exit after it is told to quit or is killed.
const QUIT_GRACE: Duration = Duration::from_secs(10);
/// The pause between the steps of the guest's shutdown, for each to finish.
const SYSRQ_STEP: Duration = Duration::from_millis(1500);

/// What one boot starts: the image's files and version, and its memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootPlan {
    pub files: ImageFiles,
    pub image_version: Option<String>,
    /// The memory the VM starts with, in MiB.
    pub memory_mb: u32,
    /// How far it may grow while it runs, in MiB.
    pub memory_max_mb: Option<u32>,
}

/// Supplies the plan of each boot.
pub type PlanSource = Box<dyn FnMut() -> Result<BootPlan, String> + Send>;
/// Supplies more bootconfig lines (`key = "value"`) for each boot, beside the secret.
pub type BootconfigSource = Box<dyn FnMut() -> String + Send>;
/// Supplies the properties each boot hands Android in its initrd, if any.
pub type BootProperties = Box<dyn FnMut(&ImageFiles) -> Option<String> + Send>;
/// Says what the VM cannot run without and is missing, and tries to provide it.
pub type Prerequisite = Box<dyn FnMut() -> Option<String> + Send>;
/// Whether the next boot's network is on the LAN (advanced networking) rather than NAT.
pub type AdvancedNetworking = Box<dyn FnMut() -> bool + Send>;
/// The network of a user's running VM, as the local API reports it.
pub type SharedNetworkStatus = Arc<Mutex<Option<NetworkStatus>>>;

/// The QEMU process of each running VM, by the VM's vsock address, for the memory report.
fn running_vms() -> &'static Mutex<HashMap<u32, u32>> {
    static VMS: OnceLock<Mutex<HashMap<u32, u32>>> = OnceLock::new();
    VMS.get_or_init(Default::default)
}

/// The process ID of the QEMU that runs the VM at `cid`.
pub fn vm_process(cid: u32) -> Option<u32> {
    running_vms()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&cid)
        .copied()
}

/// A file that is removed when this is dropped.
struct TransientFile(PathBuf);

impl Drop for TransientFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct Networking {
    advanced: AdvancedNetworking,
    network: VmNetwork,
    status: SharedNetworkStatus,
}

struct RunningVm {
    child: Child,
    console_input: Option<ChildStdin>,
    qmp: Option<Qmp>,
    readers: Vec<JoinHandle<()>>,
    qmp_socket: PathBuf,
    /// The memory it started with and may grow to, in MiB, when it may grow.
    memory: Option<(u64, u64)>,
}

impl RunningVm {
    fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.exited() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Asks the guest to sync, remount read only and power off.
    fn ask_guest_to_power_off(&mut self) {
        let Some(mut input) = self.console_input.take() else {
            return;
        };
        for step in SHUTDOWN_SEQUENCE {
            if input.write_all(step).and_then(|_| input.flush()).is_err() {
                break;
            }
            if self.wait_exit(SYSRQ_STEP) {
                break;
            }
        }
        self.console_input = Some(input);
    }
}

/// Starts and stops one user's Android VM.
pub struct LinuxVmLauncher {
    account: Account,
    name: String,
    guest_cid: u32,
    qemu: PathBuf,
    arch: GuestArch,
    processors: u32,
    page_reporting_order: u8,
    data_disk: PathBuf,
    pstore_disk: Option<PathBuf>,
    /// The user's folder for this VM's sockets and each boot's initrd.
    run_dir: PathBuf,
    console_log: PathBuf,
    plan: PlanSource,
    bootconfig: Option<BootconfigSource>,
    boot_properties: Option<BootProperties>,
    prerequisite: Option<Prerequisite>,
    networking: Option<Networking>,
    /// Whether the VM may grow; turned off for good when growing did not work.
    grow: bool,
    vm: Option<RunningVm>,
    tail: Tail,
    signs: Arc<Mutex<BootSigns>>,
    /// What the last finished boot's console says, for a start that failed.
    report: Option<String>,
}

/// What a launcher is made from: who it is for and where their files are.
pub struct LauncherSetup {
    pub account: Account,
    pub name: String,
    pub guest_cid: u32,
    pub qemu: PathBuf,
    pub processors: u32,
    pub page_reporting_order: u8,
    pub data_disk: PathBuf,
    pub pstore_disk: Option<PathBuf>,
    pub run_dir: PathBuf,
    pub console_log: PathBuf,
    pub grow_memory: bool,
}

impl LinuxVmLauncher {
    pub fn new(setup: LauncherSetup, plan: PlanSource) -> LinuxVmLauncher {
        LinuxVmLauncher {
            account: setup.account,
            name: setup.name,
            guest_cid: setup.guest_cid,
            qemu: setup.qemu,
            arch: GuestArch::HOST,
            processors: setup.processors,
            page_reporting_order: setup.page_reporting_order,
            data_disk: setup.data_disk,
            pstore_disk: setup.pstore_disk,
            run_dir: setup.run_dir,
            console_log: setup.console_log,
            plan,
            bootconfig: None,
            boot_properties: None,
            prerequisite: None,
            networking: None,
            grow: setup.grow_memory,
            vm: None,
            tail: Tail::default(),
            signs: Arc::default(),
            report: None,
        }
    }

    /// Gives each boot a network adapter: on the LAN while `advanced` says so, else NAT. The
    /// network's state goes to `status` while the VM runs.
    pub fn with_networking(
        mut self,
        advanced: AdvancedNetworking,
        status: SharedNetworkStatus,
    ) -> LinuxVmLauncher {
        self.networking = Some(Networking {
            advanced,
            network: VmNetwork::default(),
            status,
        });
        self
    }

    pub fn with_bootconfig(mut self, source: BootconfigSource) -> LinuxVmLauncher {
        self.bootconfig = Some(source);
        self
    }

    pub fn with_boot_properties(mut self, properties: BootProperties) -> LinuxVmLauncher {
        self.boot_properties = Some(properties);
        self
    }

    pub fn with_prerequisite(mut self, prerequisite: Prerequisite) -> LinuxVmLauncher {
        self.prerequisite = Some(prerequisite);
        self
    }

    /// Reads the signs of a late boot from `signs` as well, which whoever reads Android's log
    /// while it boots fills in. Each boot starts it afresh.
    pub fn with_signs(mut self, signs: Arc<Mutex<BootSigns>>) -> LinuxVmLauncher {
        self.signs = signs;
        self
    }

    /// The initrd of one boot: the image's, the boot's properties, then the bootconfig with the
    /// secret.
    fn boot_initrd(
        &mut self,
        files: &ImageFiles,
        secret: &BootSecret,
    ) -> Result<TransientFile, String> {
        let source = files.initrd.as_ref().ok_or(
            "the Android image has no initrd, which is what carries the boot's secret to the guest",
        )?;
        let mut image = std::fs::read(source)
            .map_err(|e| format!("could not read the initrd {}: {e}", source.display()))?;
        if let Some(properties) = self.boot_properties.as_mut().and_then(|p| p(files)) {
            image = append_overlay(&image, &ramdisk_overlay(&properties));
        }
        let more = self.bootconfig.as_mut().map(|b| b()).unwrap_or_default();
        let text = format!("{}{more}", bootconfig_entry(secret));
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let path = self.run_dir.join(format!("boot-{unique}.initrd"));
        self.account
            .write_private(&path, &bootconfig::append(&image, &text))?;
        Ok(TransientFile(path))
    }

    fn finish(&mut self, mut vm: RunningVm) {
        let _ = vm.child.wait();
        drop(vm.console_input.take());
        drop(vm.qmp.take());
        for reader in vm.readers.drain(..) {
            let _ = reader.join();
        }
        let _ = std::fs::remove_file(&vm.qmp_socket);
        running_vms()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.guest_cid);
        if let Some(networking) = &mut self.networking {
            networking.network.detach();
            *networking.status.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        let tail = self.tail.text();
        self.report = (!tail.trim().is_empty()).then(|| {
            format!(
                "The Android VM's console ended with:\n{tail}\nThe whole boot is in {}.",
                self.console_log.display()
            )
        });
    }

    fn start_vm(&mut self, secret: &BootSecret) -> Result<(), String> {
        let plan = (self.plan)()?;
        self.account.make_dir(&self.run_dir)?;
        if let Some(logs) = self.console_log.parent() {
            self.account.make_dir(logs)?;
        }
        if let Some(pstore) = &self.pstore_disk {
            crate::pstore::ensure(pstore)?;
            self.account.own(pstore)?;
        }
        let initrd = self.boot_initrd(&plan.files, secret)?;
        let (backend, mac, tap) = match &mut self.networking {
            Some(networking) => {
                let advanced = (networking.advanced)();
                let choice = networking
                    .network
                    .attach(advanced, &self.account, &self.run_dir)?;
                *networking.status.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(choice.status.clone());
                (choice.backend, choice.mac_address, choice.tap)
            }
            None => (NetworkBackend::None, None, None),
        };
        let qmp_socket = self.run_dir.join("qmp.sock");
        let _ = std::fs::remove_file(&qmp_socket);
        let memory_max_mb = if self.grow { plan.memory_max_mb } else { None };
        let config = VmConfig {
            name: self.name.clone(),
            arch: self.arch,
            kernel: plan.files.kernel.clone(),
            initrd: initrd.0.clone(),
            kernel_cmdline: plan.files.kernel_cmdline.clone(),
            memory_mb: plan.memory_mb,
            memory_max_mb,
            processors: self.processors,
            system_image: plan.files.system_image.clone(),
            data_disk: self.data_disk.clone(),
            pstore_disk: self.pstore_disk.clone(),
            guest_cid: self.guest_cid,
            page_reporting_order: self.page_reporting_order,
            qmp_socket: qmp_socket.clone(),
            network: backend,
            mac_address: mac.unwrap_or_else(|| crate::qemu::mac_address(self.account.uid)),
        };
        let args = config.qemu_args().map_err(|e| e.to_string())?;
        let mut command = Command::new(&self.qemu);
        command
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let before: Option<BeforeExec> = tap
            .as_ref()
            .map(|tap| Box::new(place_tap(tap)) as BeforeExec);
        self.account.run_as(&mut command, before);
        self.tail = Tail::default();
        *self.signs.lock().unwrap_or_else(|e| e.into_inner()) = BootSigns::default();
        self.report = None;
        let mut child = command.spawn().map_err(|e| {
            format!(
                "could not start {} for the Android VM: {e}",
                self.qemu.display()
            )
        })?;
        // The child has its own copy of the tap.
        drop(tap);
        running_vms()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.guest_cid, child.id());
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let header = boot_header(
            started,
            plan.image_version.as_deref(),
            &config.full_kernel_cmdline(),
        );
        let mut readers = Vec::new();
        if let Some(out) = child.stdout.take() {
            readers.extend(
                capture(
                    out,
                    Some(self.console_log.clone()),
                    Some(header),
                    self.tail.clone(),
                    Arc::clone(&self.signs),
                )
                .ok(),
            );
        }
        if let Some(err) = child.stderr.take() {
            readers.extend(
                capture(
                    err,
                    Some(self.console_log.clone()),
                    None,
                    self.tail.clone(),
                    Arc::clone(&self.signs),
                )
                .ok(),
            );
        }
        let console_input = child.stdin.take();
        let mut vm = RunningVm {
            child,
            console_input,
            qmp: None,
            readers,
            qmp_socket: qmp_socket.clone(),
            memory: memory_max_mb
                .map(|max| (u64::from(plan.memory_mb), u64::from(max)))
                .filter(|(base, max)| max > base),
        };
        // QEMU answers on its control socket once the machine is made, the initrd read.
        let deadline = Instant::now() + QMP_WAIT;
        let qmp = loop {
            match Qmp::connect(&qmp_socket, Duration::ZERO) {
                Ok(qmp) => break Ok(qmp),
                Err(error) => {
                    if vm.exited() {
                        break Err("QEMU exited while it started the Android VM".to_string());
                    }
                    if Instant::now() >= deadline {
                        break Err(format!(
                            "QEMU did not answer on its control socket within {} s: {error}",
                            QMP_WAIT.as_secs()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        };
        drop(initrd);
        match qmp {
            Ok(qmp) => {
                vm.qmp = Some(qmp);
                self.vm = Some(vm);
                Ok(())
            }
            Err(why) => {
                let _ = vm.child.kill();
                self.finish(vm);
                Err(why)
            }
        }
    }
}

impl GuestLauncher for LinuxVmLauncher {
    fn start(&mut self, secret: &BootSecret) -> Result<(), String> {
        if let Some(mut vm) = self.vm.take() {
            // A VM left from an earlier start goes first; it has no secret this session knows.
            let _ = vm.child.kill();
            self.finish(vm);
        }
        let mut started = self.start_vm(secret);
        if started.is_err() && self.grow && self.tail.contains("virtio-mem") {
            // A QEMU or a host without virtio-mem: the VM keeps the size it starts with.
            self.memory_growth_failed("QEMU could not make the virtio-mem device");
            started = self.start_vm(secret);
        }
        if started.is_err() {
            if let Some(networking) = &mut self.networking {
                networking.network.detach();
                *networking.status.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        }
        started
    }

    fn stop(&mut self, timeout: Duration) -> Result<(), String> {
        let Some(mut vm) = self.vm.take() else {
            if let Some(networking) = &mut self.networking {
                networking.network.detach();
            }
            return Ok(());
        };
        if !vm.exited() {
            // A frozen guest cannot take the request.
            if let Some(qmp) = vm.qmp.as_mut() {
                let _ = qmp.cont();
            }
            vm.ask_guest_to_power_off();
            if !vm.wait_exit(timeout) {
                if let Some(qmp) = vm.qmp.as_mut() {
                    let _ = qmp.quit();
                }
                if !vm.wait_exit(QUIT_GRACE) {
                    let _ = vm.child.kill();
                    if !vm.wait_exit(QUIT_GRACE) {
                        let pid = vm.child.id();
                        self.vm = Some(vm);
                        return Err(format!(
                            "the Android VM (QEMU, process {pid}) did not exit within {} s of \
                             being killed",
                            QUIT_GRACE.as_secs()
                        ));
                    }
                }
            }
        }
        self.finish(vm);
        Ok(())
    }

    fn exited(&mut self) -> Option<String> {
        let vm = self.vm.as_mut()?;
        let status = vm.child.try_wait().ok().flatten()?;
        Some(format!("the Android VM exited during boot (QEMU {status})"))
    }

    fn diagnostics(&mut self) -> Option<String> {
        self.report.clone().or_else(|| {
            let tail = self.tail.text();
            (!tail.trim().is_empty()).then(|| {
                format!(
                    "The Android VM's console last showed:\n{tail}\nThe whole boot is in {}.",
                    self.console_log.display()
                )
            })
        })
    }

    fn vm_id(&self) -> Option<String> {
        self.vm.as_ref().map(|_| self.guest_cid.to_string())
    }

    fn prerequisite(&mut self) -> Option<String> {
        for (device, what) in [
            ("/dev/kvm", "KVM, which runs the Android VM: turn virtualization on in the computer's firmware settings and load the kvm module"),
            ("/dev/vhost-vsock", "vhost-vsock, which connects the Android VM to DSA: load the vhost_vsock module"),
        ] {
            if !Path::new(device).exists() {
                return Some(format!("{device} is missing. DSA needs {what}."));
            }
        }
        if !self.qemu.is_absolute() || !self.qemu.exists() {
            return Some(format!(
                "{} is not installed. DSA runs Android with QEMU: install it (the qemu-system \
                 package for this computer's architecture).",
                self.qemu.display()
            ));
        }
        self.prerequisite.as_mut().and_then(|check| check())
    }

    fn explain_stall(&mut self, stall: GuestStall) -> Option<String> {
        let signs = self.signs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Some(match stall {
            GuestStall::BootUnfinished => explain_unfinished_boot(&signs, &self.console_log),
            GuestStall::AgentUnreachable => {
                explain_unreachable_agent(&signs, &self.console_log, &self.data_disk)
            }
        })
    }

    fn agent_refused_for_keys(&mut self) -> bool {
        self.signs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .agent_refused_for_keys()
    }

    /// Pauses the whole VM. Its memory stays with QEMU, where Linux can reclaim and swap it
    /// like any process's, and nothing in the guest runs until the thaw.
    fn freeze(&mut self) -> Result<(), String> {
        let qmp = self
            .vm
            .as_mut()
            .and_then(|vm| vm.qmp.as_mut())
            .ok_or("the Android VM is not running")?;
        qmp.stop()
            .map_err(|e| format!("the Android VM could not be paused: {e}"))
    }

    fn thaw(&mut self) -> Result<(), String> {
        let Some(qmp) = self.vm.as_mut().and_then(|vm| vm.qmp.as_mut()) else {
            return Ok(());
        };
        qmp.cont()
            .map_err(|e| format!("the Android VM could not be resumed: {e}"))
    }

    fn memory_range_mb(&self) -> Option<(u64, u64)> {
        self.vm.as_ref()?.memory
    }

    fn set_memory_mb(&mut self, mb: u64) -> Result<(), String> {
        let vm = self.vm.as_mut().ok_or("the Android VM is not running")?;
        let (base, max) = vm
            .memory
            .ok_or("the Android VM keeps the size it started with")?;
        let mb = mb.clamp(base, max);
        let qmp = vm.qmp.as_mut().ok_or("the Android VM is not running")?;
        qmp.set_plugged(plugged_bytes(mb - base))
            .map_err(|e| format!("the Android VM's memory could not be set to {mb} MiB: {e}"))
    }

    fn memory_growth_failed(&mut self, why: &str) {
        // The guest keeps the memory it started with, and later VMs start without virtio-mem.
        self.grow = false;
        eprintln!(
            "uid:{}: memory: the Android VM cannot grow on this computer ({why}); it keeps the \
             memory it started with",
            self.account.uid
        );
    }
}
