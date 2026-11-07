//! What the pages show, worked out from what the service reports. Nothing here touches GTK,
//! so it is tested without a display.

use std::collections::VecDeque;

use crate::api::{Failure, Problem};
use crate::i18n::{tr, trf};
use crate::model::{
    AndroidVersion, App, AppWindow, ArmTranslation, Gapps, GappsState, GuestMemory, HostRole,
    Memory, Network, NetworkMode, State, Status, ALL_FOLDERS,
};

/// A problem in the user's language, with what the client said after it.
pub fn problem_text(failure: &Failure) -> String {
    let words = match failure.problem {
        Problem::CliMissing => tr("The dsa command could not be started. Reinstall DSA."),
        Problem::ServiceUnreachable => tr("The DSA service is not running."),
        Problem::Refused => tr("The DSA service could not do this."),
        Problem::Unreadable => {
            tr("The answer of the dsa command could not be read. Reinstall DSA.")
        }
        Problem::NotAuthorized => {
            tr("The setting was not changed, because an administrator's permission was not given.")
        }
    };
    if failure.detail.is_empty() || failure.problem == Problem::NotAuthorized {
        words
    } else {
        format!("{words}\n{}", failure.detail)
    }
}

pub fn state_text(state: State) -> String {
    tr(match state {
        State::Unknown => "Unknown",
        State::Stopped => "Stopped",
        State::Starting => "Starting",
        State::Active => "Running",
        State::Dozing => "Dozing (idle)",
        State::Frozen => "Frozen",
        State::Stopping => "Stopping",
        State::Stalled => "Not ready",
    })
}

/// Which of the General page's buttons can be used in a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    pub can_start: bool,
    pub can_stop: bool,
    pub can_freeze: bool,
}

pub fn controls(status: Option<&Status>) -> Controls {
    let state = status.map(|s| s.state).unwrap_or(State::Unknown);
    Controls {
        can_start: matches!(state, State::Stopped | State::Stalled),
        can_stop: matches!(
            state,
            State::Starting | State::Active | State::Dozing | State::Frozen | State::Stalled
        ),
        can_freeze: matches!(state, State::Active | State::Dozing),
    }
}

/// A size in KiB as people read it: `512 KB`, `340 MB`, `2.4 GB`.
pub fn size_kb(kb: u64) -> String {
    const MB: u64 = 1024;
    const GB: u64 = 1024 * 1024;
    if kb >= GB {
        format!("{:.1} GB", kb as f64 / GB as f64)
    } else if kb >= MB {
        format!("{} MB", (kb + MB / 2) / MB)
    } else {
        format!("{kb} KB")
    }
}

/// A size in bytes, as [`size_kb`] shows sizes.
pub fn size_bytes(bytes: u64) -> String {
    size_kb(bytes.div_ceil(1024))
}

/// One line of the memory page: what it is, a detail, and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRow {
    pub label: String,
    pub detail: String,
    pub size: String,
}

fn row(label: String, detail: String, kb: u64) -> MemoryRow {
    MemoryRow {
        label,
        detail,
        size: size_kb(kb),
    }
}

pub fn host_rows(memory: &Memory) -> Vec<MemoryRow> {
    memory
        .host
        .iter()
        .map(|p| {
            let label = tr(match p.role {
                HostRole::Vm => "Virtual machine (crosvm)",
                HostRole::WindowManager => "Window manager",
                HostRole::AppWindow => "App window",
            });
            let detail = trf(
                "Process {0}, {1} private",
                &[&p.pid.to_string(), &size_kb(p.private_kb)],
            );
            row(label, detail, p.working_set_kb)
        })
        .collect()
}

/// Android's breakdown: the apps and the system, the caches, the kernel's reclaimable caches,
/// shared memory, compressed memory and what is free.
pub fn guest_rows(guest: &GuestMemory) -> Vec<MemoryRow> {
    vec![
        row(
            tr("Apps and system"),
            tr("The apps and the system server, counted by their share (PSS)."),
            guest.total_pss_kb,
        ),
        row(
            tr("Cache"),
            tr("File contents kept in memory. Android drops them when it needs the memory."),
            guest.cached_kb + guest.buffers_kb,
        ),
        row(
            tr("Kernel caches"),
            tr("Kernel caches the kernel can free on demand."),
            guest.slab_reclaimable_kb,
        ),
        row(tr("Shared memory"), String::new(), guest.shmem_kb),
        row(
            tr("Compressed memory"),
            trf(
                "Holds {0} of pages, compressed",
                &[&size_kb(guest.zram_original_kb)],
            ),
            guest.zram_used_kb,
        ),
        row(tr("Free"), String::new(), guest.free_kb),
    ]
}

pub fn kernel_rows(guest: &GuestMemory) -> Vec<MemoryRow> {
    let Some(k) = &guest.kernel else {
        return Vec::new();
    };
    let mut rows = vec![
        row(
            tr("Kernel objects"),
            tr("Memory the kernel keeps for its own data and cannot reclaim"),
            k.slab_unreclaimable_kb,
        ),
        row(tr("Thread stacks"), String::new(), k.kernel_stack_kb),
        row(tr("Page tables"), String::new(), k.page_tables_kb),
        row(
            tr("Kernel mappings (vmalloc)"),
            String::new(),
            k.vmalloc_used_kb,
        ),
        row(tr("Per-CPU data"), String::new(), k.percpu_kb),
        row(
            tr("Drivers"),
            tr("What no counter names, such as GPU and DMA buffers"),
            k.unaccounted_kb,
        ),
    ];
    if let Some(reserved) = k.reserved_at_boot_kb {
        rows.push(row(
            tr("Reserved at boot"),
            trf(
                "Kernel code, page structures and buffers, of {0} given; contiguous area {1}",
                &[&size_kb(k.mem_online_kb), &size_kb(k.cma_total_kb)],
            ),
            reserved,
        ));
    }
    rows
}

pub fn process_rows(guest: &GuestMemory) -> Vec<MemoryRow> {
    guest
        .top_processes
        .iter()
        .map(|p| {
            row(
                p.name.clone(),
                trf("Process {0}", &[&p.pid.to_string()]),
                p.pss_kb,
            )
        })
        .collect()
}

/// What the VM holds on the host now, or `None` without a VM.
pub fn vm_size_kb(memory: &Memory) -> Option<u64> {
    let vm: Vec<_> = memory
        .host
        .iter()
        .filter(|p| p.role == HostRole::Vm)
        .collect();
    (!vm.is_empty()).then(|| vm.iter().map(|p| p.working_set_kb).sum())
}

/// How full the guest's memory is, from 0 to 1.
pub fn guest_used_fraction(guest: &GuestMemory) -> f64 {
    if guest.total_kb == 0 {
        return 0.0;
    }
    (guest.total_kb - guest.available_kb.min(guest.total_kb)) as f64 / guest.total_kb as f64
}

/// The smallest memory limit the service takes, in MiB.
pub const SMALLEST_MEMORY_LIMIT_MB: u32 = 2048;
const OFFERED_LIMITS_MB: [u32; 7] = [2048, 3072, 4096, 6144, 8192, 12288, 16384];

/// The memory limits to choose from, automatic (0) first, with the current one among them
/// even when it was set another way.
pub fn memory_limit_choices(current_mb: u32) -> Vec<(String, u32)> {
    let mut limits: Vec<u32> = OFFERED_LIMITS_MB.to_vec();
    if current_mb != 0 && !limits.contains(&current_mb) {
        limits.push(current_mb);
        limits.sort_unstable();
    }
    let mut choices = vec![(tr("Automatic"), 0)];
    choices.extend(
        limits
            .into_iter()
            .map(|mb| (size_kb(u64::from(mb) * 1024), mb)),
    );
    choices
}

/// The render GPU choices, as `settings set render-gpu` takes them, and their labels.
pub fn render_gpu_choices() -> Vec<(&'static str, String)> {
    vec![
        (
            "display",
            tr("The GPU of the primary monitor (recommended)"),
        ),
        ("integrated", tr("Integrated GPU")),
        ("discrete", tr("Discrete GPU (for example, for games)")),
    ]
}

/// Where the render GPU setting is among the choices; another name is shown as itself after
/// them.
pub fn render_gpu_index(setting: &str) -> usize {
    let setting = setting.to_ascii_lowercase();
    render_gpu_choices()
        .iter()
        .position(|(value, _)| *value == setting || setting.is_empty() && *value == "display")
        .unwrap_or(render_gpu_choices().len())
}

/// One choice on the Android version page: always the newest, or one major version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidChoice {
    pub channel: String,
    pub major: i64,
    pub label: String,
}

/// How moving to a choice treats the user's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidMove {
    Same,
    /// A newer major: apps and their data move along.
    Up,
    /// An older major: every user's Android data is erased.
    Down,
}

pub fn android_choices(info: &AndroidVersion) -> Vec<AndroidChoice> {
    let newest = info.supported.iter().copied().max().unwrap_or(info.major);
    let mut choices = vec![AndroidChoice {
        channel: "latest".into(),
        major: newest,
        label: trf(
            "Always the newest (now Android {0})",
            &[&newest.to_string()],
        ),
    }];
    let mut majors = info.supported.clone();
    majors.sort_unstable_by(|a, b| b.cmp(a));
    choices.extend(majors.into_iter().map(|major| AndroidChoice {
        channel: major.to_string(),
        major,
        label: trf("Android {0}", &[&major.to_string()]),
    }));
    choices
}

pub fn android_choice_index(info: &AndroidVersion) -> usize {
    android_choices(info)
        .iter()
        .position(|c| c.channel == info.channel)
        .unwrap_or(0)
}

pub fn android_move(info: &AndroidVersion, choice: &AndroidChoice) -> AndroidMove {
    if choice.major == info.major {
        AndroidMove::Same
    } else if choice.major > info.major {
        AndroidMove::Up
    } else {
        AndroidMove::Down
    }
}

/// The apps a search matches, by name: the query is matched against the label and the
/// package, ignoring case.
pub fn filter_apps<'a>(apps: &'a [App], query: &str) -> Vec<&'a App> {
    let query = query.trim().to_lowercase();
    let mut shown: Vec<&App> = apps
        .iter()
        .filter(|a| {
            query.is_empty()
                || a.label.to_lowercase().contains(&query)
                || a.package.to_lowercase().contains(&query)
        })
        .collect();
    shown.sort_by_key(|a| a.label.to_lowercase());
    shown
}

pub fn apps_count(total: usize, shown: usize, searching: bool) -> String {
    if searching {
        trf("{0} of {1} apps", &[&shown.to_string(), &total.to_string()])
    } else {
        trf("{0} apps", &[&total.to_string()])
    }
}

/// What the row of an app says under its name.
pub fn app_detail(app: &App) -> String {
    let mut parts = Vec::new();
    if !app.version_name.is_empty() {
        parts.push(trf("Version {0}", &[&app.version_name]));
    }
    if app.total_bytes > 0 {
        parts.push(size_bytes(app.total_bytes));
    }
    if app.system_app {
        parts.push(tr("Part of Android"));
    }
    parts.join(" · ")
}

/// The sizes an app's windows can open at: Android's choice first, then phones and tablets.
pub fn start_size_choices() -> Vec<(String, Option<(u32, u32)>)> {
    let size =
        |key: &str, w: u32, h: u32| (trf(key, &[&w.to_string(), &h.to_string()]), Some((w, h)));
    vec![
        (tr("Opens at the size Android chooses"), None),
        size("Phone, portrait ({0} × {1})", 412, 915),
        size("Phone, landscape ({0} × {1})", 915, 412),
        size("Tablet, portrait ({0} × {1})", 800, 1280),
        size("Tablet, landscape ({0} × {1})", 1280, 800),
    ]
}

/// Where an app's start size is among [`start_size_choices`]; `None` for a size set another
/// way (with `dsa app-window`).
pub fn start_size_index(window: Option<&AppWindow>) -> Option<usize> {
    let size = window.and_then(|w| w.start_size);
    start_size_choices().iter().position(|(_, s)| *s == size)
}

/// Why Android refused to uninstall an app, in words.
pub fn uninstall_reason(status: &str) -> String {
    tr(match status {
        "not-installed" => "it is no longer installed",
        "system-app" => "it is part of Android",
        "blocked" => {
            "Android does not allow it, because the app is a device administrator or a policy keeps it"
        }
        _ => "Android could not uninstall it",
    })
}

/// Why Android refused to install an APK, in words.
pub fn install_reason(status: &str) -> String {
    tr(match status {
        "incompatible" => "the app does not run on this Android: it needs another processor type, a newer Android version or a feature this device does not have.",
        "conflicting-signature" => "the app is installed already, signed with another key. Uninstall it in Android first.",
        "invalid-apk" => "the file is not a signed APK that Android can read.",
        "insufficient-storage" => "Android has no room for the app.",
        "downgrade" => "a newer version of the app is installed. Uninstall it in Android first.",
        "blocked" => "a policy of this Android does not allow the app.",
        _ => "Android did not install the app.",
    })
}

/// What the ARM translation section says.
pub fn arm_status(arm: &ArmTranslation) -> String {
    match &arm.package {
        None => tr("Not supported: no ARM translation package is installed."),
        Some((name, version, _)) if !arm.enabled => trf(
            "Not supported: {0} {1} is installed but turned off.",
            &[name, version],
        ),
        Some((name, version, license)) => {
            let license = if license.is_empty() {
                tr("licence not given")
            } else {
                license.clone()
            };
            trf("Supported by {0} {1} ({2}).", &[name, version, &license])
        }
    }
}

pub fn arm_abis(arm: &ArmTranslation) -> String {
    match &arm.abilist {
        Some(abis) => trf("Android installs apps for: {0}", &[abis]),
        None => tr("Android is not running, so what it supports now is not known."),
    }
}

/// What the confirmation says before Google's ARM translator is downloaded: where it comes
/// from, and the terms the user accepts.
pub fn arm_google_notice() -> String {
    trf(
        "DSA downloads the Android emulator image for this Android version from Google's SDK repository ({0}) and takes out only its ARM translator. The translator is Google's software, offered under the Android SDK License ({1}); by downloading it you accept those terms. DSA does not ship or pass it on. The image is about 2 GB and is deleted afterwards. The translator works from the next start of Android.",
        &[
            crate::model::GOOGLE_ARM_SOURCE,
            crate::model::GOOGLE_ARM_TERMS_URL,
        ],
    )
}

/// What the ARM apps section says while Google's translator is fetched.
pub fn arm_google_busy_text() -> String {
    tr("Downloading about 2 GB from Google and taking out the translator. It takes minutes.")
}

/// What the Google Play item says of the package and whether Android uses it.
pub fn gapps_status(gapps: &Gapps) -> String {
    let package = match &gapps.package {
        Some(p) => trf(
            "LiteGapps {0} {1} for Android {2} ({3})",
            &[&p.variant, &p.version, &p.android_version, &p.arch],
        ),
        None => String::new(),
    };
    match gapps.state() {
        GappsState::RestartPending if gapps.package.is_some() => trf(
            "Installed: {0}. Restart Android to start using it.",
            &[&package],
        ),
        GappsState::RestartPending => {
            tr("Removed. The Google apps stay in Android until it restarts.")
        }
        GappsState::Installed if gapps.package.is_some() => trf("Installed: {0}.", &[&package]),
        _ => tr("Not installed."),
    }
}

/// Where the installed package came from; empty without one.
pub fn gapps_source(gapps: &Gapps) -> String {
    match &gapps.package {
        Some(p) => {
            let file = if p.file.is_empty() { &p.source } else { &p.file };
            trf("Downloaded from the LiteGapps project: {0}", &[file])
        }
        None => String::new(),
    }
}

/// What the Google Play item says while the package is being installed.
pub fn gapps_busy_text() -> String {
    tr("Downloading and installing Google Play. This can take several minutes.")
}

/// What the Google Play item says of the Android ID Google's registration page asks for.
pub fn gapps_device_id(gapps: &Gapps) -> String {
    match &gapps.device_id {
        Some(_) => tr("Enter this number on Google's registration page."),
        None => tr("It shows here once Google Play is installed and Android has started."),
    }
}

/// A shared folder's name in the user's language.
pub fn folder_label(folder: &str) -> String {
    tr(folder)
}

/// The shared folders after turning `folder` on or off, in the order the service keeps them.
pub fn toggle_folder(current: &[String], folder: &str, on: bool) -> Vec<String> {
    ALL_FOLDERS
        .iter()
        .filter(|f| {
            if **f == folder {
                on
            } else {
                current.iter().any(|c| c == *f)
            }
        })
        .map(|f| f.to_string())
        .collect()
}

pub fn network_mode_text(network: Option<&Network>) -> String {
    tr(match network.map(|n| n.mode) {
        Some(NetworkMode::Nat) => {
            "Connected through this computer (NAT). Android apps see it as Wi-Fi, with no data charges."
        }
        Some(NetworkMode::Advanced) => {
            "Connected to this computer's network as a device of its own (advanced networking)."
        }
        Some(NetworkMode::Off) => "Android is not running, so it has no network yet.",
        _ => "The DSA service does not report the network. Update DSA to see it.",
    })
}

/// A list of addresses, or "None".
pub fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        tr("None")
    } else {
        items.join(", ")
    }
}

/// Whether the advanced networking setting differs from the network Android has now, so it
/// applies only at the next start.
pub fn network_restart_needed(advanced_setting: bool, network: Option<&Network>) -> bool {
    match network.map(|n| n.mode) {
        Some(NetworkMode::Nat) => advanced_setting,
        Some(NetworkMode::Advanced) => !advanced_setting,
        _ => false,
    }
}

/// The fingerprint Android reports from its next start, from the values on the page: the
/// whole fingerprint when one is written, or the one its parts make.
pub fn fingerprint_preview(value: impl Fn(&str) -> String) -> String {
    let whole = value("fingerprint");
    if !whole.is_empty() {
        return whole;
    }
    format!(
        "{}/{}/{}:{}/{}/{}:{}/{}",
        value("brand"),
        value("name"),
        value("device"),
        value("fingerprint.release"),
        value("fingerprint.id"),
        value("fingerprint.incremental"),
        value("fingerprint.type"),
        value("fingerprint.tags")
    )
}

/// The label of a device identity value.
pub fn identity_label(key: &str) -> String {
    tr(match key {
        "manufacturer" => "Manufacturer",
        "brand" => "Brand",
        "model" => "Model",
        "device" => "Device",
        "name" => "Product name",
        "characteristics" => "Form factor",
        "fingerprint.release" => "Release",
        "fingerprint.id" => "Build ID",
        "fingerprint.incremental" => "Incremental version",
        "fingerprint.type" => "Build type",
        "fingerprint.tags" => "Tags",
        _ => "Whole fingerprint",
    })
}

/// Checks a value typed for the device identity as the service will; `None` when it is
/// fine. An empty value is fine: it keeps the value shown.
pub fn identity_problem(key: &str, value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let field = dsa_host_core::device_identity::Field::parse(key).ok()?;
    field.check(value).err()
}

/// The priorities of Android's log, lowest first, as `dsa logcat` takes them.
pub const LOG_PRIORITIES: [&str; 6] = ["V", "D", "I", "W", "E", "F"];

pub fn priority_label(priority: &str) -> String {
    tr(match priority {
        "V" => "Verbose",
        "D" => "Debug",
        "I" => "Info",
        "W" => "Warning",
        "E" => "Error",
        _ => "Fatal",
    })
}

/// The arguments that read Android's log from `priority` up.
pub fn logcat_args(priority: &str) -> Vec<String> {
    vec!["logcat".into(), "--json".into(), format!("*:{priority}")]
}

/// The log page's lines: the newest records, filtered by text, which can be paused.
#[derive(Debug, Default)]
pub struct LogView {
    lines: VecDeque<crate::model::LogLine>,
    waiting: Vec<crate::model::LogLine>,
    pub filter: String,
    paused: bool,
}

impl LogView {
    /// The most lines the page keeps.
    pub const LIMIT: usize = 5000;

    /// Adds a line `dsa logcat --json` printed. Returns whether the page should show it now.
    pub fn add(&mut self, json_line: &str) -> bool {
        let Some(line) = crate::model::LogLine::parse(json_line) else {
            return false;
        };
        if self.paused {
            self.waiting.push(line);
            return false;
        }
        self.push(line);
        true
    }

    fn push(&mut self, line: crate::model::LogLine) {
        self.lines.push_back(line);
        while self.lines.len() > Self::LIMIT {
            self.lines.pop_front();
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// How many lines wait while paused, in words; empty when not paused.
    pub fn paused_text(&self) -> String {
        if self.paused {
            trf(
                "Paused. {0} new lines are waiting.",
                &[&self.waiting.len().to_string()],
            )
        } else {
            String::new()
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if !paused {
            for line in std::mem::take(&mut self.waiting) {
                self.push(line);
            }
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.waiting.clear();
    }

    pub fn matches(&self, line: &crate::model::LogLine) -> bool {
        let filter = self.filter.trim().to_lowercase();
        filter.is_empty() || line.text().to_lowercase().contains(&filter)
    }

    /// The lines the filter shows, oldest first.
    pub fn shown(&self) -> Vec<&crate::model::LogLine> {
        self.lines.iter().filter(|l| self.matches(l)).collect()
    }

    /// The newest line, for appending it to what is shown.
    pub fn last(&self) -> Option<&crate::model::LogLine> {
        self.lines.back()
    }
}

/// What copying log lines puts on the clipboard: each whole line as the log prints it, one
/// per line. `shown` is what the page shows and `selected` the positions in it the user
/// picked, in any order; the lines keep the page's order.
pub fn log_copy_text(shown: &[crate::model::LogLine], selected: &[usize]) -> String {
    let mut picked: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|&i| i < shown.len())
        .collect();
    picked.sort_unstable();
    picked.dedup();
    picked
        .iter()
        .map(|&i| shown[i].text())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The user's DSA logs: the window manager's and the app windows'.
pub fn user_log_folder() -> std::path::PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })
        .unwrap_or_else(|| "/tmp".into());
    data.join("dsa").join("logs")
}

/// The service's and Android's logs: the folder the service reports, or the machine's.
pub fn service_log_folder(reported: Option<&str>) -> std::path::PathBuf {
    reported
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/var/lib/dsa/logs".into())
}

/// Where the packages put the licences of everything DSA ships.
pub const LICENSE_FOLDER: &str = "/usr/share/doc/desktop-subsystem-for-android";

/// Where the source code is.
pub const SOURCE_URL: &str = "https://github.com/thisisthepy/desktop-subsystem-for-android";

/// The program that starts the user's window manager again: the compositor's user unit.
pub fn restart_window_manager_command() -> Vec<&'static str> {
    vec!["systemctl", "--user", "restart", "dsa-compositor.service"]
}

/// The program that shows the system's software updates, when the desktop has one.
pub fn software_updates_commands() -> Vec<Vec<&'static str>> {
    vec![
        vec!["gnome-software", "--mode=updates"],
        vec!["plasma-discover", "--mode", "update"],
    ]
}
