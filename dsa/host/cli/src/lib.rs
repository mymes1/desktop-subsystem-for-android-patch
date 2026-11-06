//! `dsa`, the command line client of the local API.
//!
//! ```text
//! dsa status
//! dsa start [--wait]
//! dsa stop [--wait]
//! dsa freeze
//! dsa update [--wait]
//! dsa launch <package> [--wait-first-frame]
//! dsa tap <package> <x> <y>
//! dsa stop-app <package>
//! dsa install <file.apk> [--progress]
//! dsa apk-info <file.apk>
//! dsa open <file>
//! dsa share <file>
//! dsa tasks
//! dsa settings [set <name> <value>]
//! dsa settings device | set device.<field> <value> | reset device[.<field>]
//! dsa memory
//! dsa gpus
//! dsa widgets [list | add <provider> | remove <id> | watch]
//! dsa apps [list | info <package> | uninstall <package> | settings [<package>]]
//! dsa background [list | open <package> | stop <package>]
//! dsa app-window [<package> [--fixed-size on|off] [--start-size <width>x<height>|auto] | <package> reset]
//! dsa logcat [-d] [-b <buffers>] [<tag>[:<priority>] ...]
//! dsa logs collect -o <file.zip>
//! dsa arm-translation [status | install <folder> | remove]
//! dsa reset-data --yes
//! ```
//!
//! `dsa app-window` lists the user's window settings for each app that has any; with a package
//! it shows that app's, and with `--fixed-size` or `--start-size` changes them: a fixed-size
//! app's windows cannot be resized or maximized, and an app with a start size opens its windows
//! at that size, in density-independent pixels. `reset` puts the app back to the defaults.
//! With `--json` it prints `apps`, each with `package`, `fixed_size` and `start_size` (null
//! for none, else `[width, height]`).
//!
//! It acts on the Android of the user who runs it: the service learns that user from the
//! operating system, so there is no option to name another one.
//!
//! `dsa launch` starts an app in a window of its own, named by its package (such as
//! `com.android.settings`). `dsa tasks` lists the app tasks Android shows as windows.
//!
//! `dsa launch --wait-first-frame` (Windows) waits until the app's new window has presented
//! its first frame and that frame has reached the screen, and prints how long each took from
//! the launch request, in milliseconds on the host clock. With `--json` it prints
//! `package`, `task_id`, `request_ms` (the launch request's answer), `first_frame_ms`
//! and `first_shown_ms` (null when the window's swapchain does not say when its frames are
//! shown). It counts only a window opened after the command started, so close the app's
//! window first.
//!
//! `dsa tap <package> <x> <y>` presses and releases the primary button at a point of the
//! app's task (its focused task, else its newest), in Android display pixels from the task's
//! top left corner. The press goes to Android through the local API, so it never moves the
//! mouse or reaches another window. While the input trace is on, the app's window is told of
//! the press first (Windows), so the trace has it and the window watches for its first
//! visible change. With `--json` it prints `task_id`, `x`, `y` and `traced`.
//!
//! `dsa install` installs an APK from this computer in Android, starting Android first when it
//! is not running, and says how it went: the app's package once it is installed, or the reason
//! Android gave for refusing it, with exit code 1. The installed app then appears in the Start
//! menu like any other. APKs up to 2 GiB can be installed, one at a time; a second install waits
//! for the first. With `--json` it prints `installed`, `status` (`installed`, or the reason:
//! `incompatible`, `conflicting-signature`, `invalid-apk`, `insufficient-storage`, `downgrade`,
//! `blocked` or `other`), `package` and `message`. A refusal's line on standard error starts
//! with `not installed (<reason>):`. With `--progress` it first prints a line for each step,
//! for programs such as the install window: `stage starting` while Android starts,
//! `stage sending` and then `progress <percent>` as the APK goes to Android, and
//! `stage installing` once Android has all of it.
//!
//! `dsa apk-info` reads an APK on this computer and says what it is: its package, its name and
//! icon, its version, the Android versions it is made for, the permissions it asks for and the
//! certificate it is signed with. When Android is running it also says what Android has of the
//! package, and whether installing the APK would be a new install, an update, the same version
//! again, a downgrade or an APK signed with another key; Android refuses the last two. It never
//! starts Android. With `--json` it prints `package`, `label`, `version_code`, `version_name`,
//! `min_sdk`, `min_android`, `target_sdk`, `target_android`, `size`, `permissions` (each with
//! `name`, `description` and `sensitive`), `icon` (`kind` `image` with `png` in base64, or
//! `adaptive` with a `background` and a `foreground` layer, each an `image`, a `color` as
//! `#AARRGGBB` or `unknown`), `signer` (`scheme` and `sha256`), `abis`, `android` (`running`,
//! `stopped` or `unreachable`), `installed` (`version_code`, `version_name` and `signers`, or
//! null) and `install` (`new`, `update`, `reinstall`, `downgrade`, `signature-mismatch`, or
//! null while Android is not running).
//!
//! `dsa open` sends a file from this computer to Android, which asks which app opens it; that
//! app shows in a window of its own. `dsa share` does the same for sharing the file with an
//! app. Android is started first when it is not running. Files up to 64 MiB can be sent.
//!
//! `dsa widgets list` (or `dsa widgets`) lists the widgets the apps in Android offer, each by
//! its provider (`package/class`), and the widgets placed on the desktop, each by its ID.
//! `dsa widgets add <provider>` places one; when its app asks the user to set it up first, its
//! setup shows in a window and the command waits for the user. `dsa widgets remove <id>`
//! removes one, with its window. Android is started first when it is not running. With
//! `--json`, `list` prints `providers` (each with `provider`, `package`, `label`,
//! `preview_png` in base64, its sizes in dp, `resize_mode`, `needs_configuration` and `app`,
//! the name of the app that offers it) and `widgets` (each with `id`, `provider`, `label`,
//! `app` and its `placement`), `add` prints the placed widget, and `remove` prints `removed`
//! with the widget's ID.
//!
//! `dsa widgets watch` prints the placed widgets, then a line each time one is placed or
//! removed, until it is stopped or Android stops. With `--json` each line is one JSON
//! object: `{"event":"widgets","widgets":[...]}` first, then
//! `{"event":"placed","widget":{...}}` or `{"event":"removed","id":N}`.
//!
//! `dsa apps list` (or `dsa apps`) lists the apps installed in Android that a launcher lists,
//! each with its package, name, version and size; `(system)` marks the apps that are part of
//! Android. `dsa apps info <package>` describes one: its version, its size (app, data and
//! cache), when it was installed and updated, and whether it can be uninstalled. Both have
//! Android read the sizes again first. `dsa apps uninstall <package>` uninstalls an app you
//! installed; an app that is part of Android stays, and the line on standard error starts with
//! `not uninstalled (<reason>):` (`not-installed`, `system-app`, `blocked` or `other`), with
//! exit code 1. Windows' list of installed apps runs it to uninstall an app from there.
//! `dsa apps settings <package>` opens Android's own settings of the app (its permissions,
//! storage and notifications) in a window, and `dsa apps settings` Android's list of every app.
//! Android is started first when it is not running. With `--json`, `list` prints `apps`, each
//! with `package`, `label`, `version_name`, `version_code`, `first_install_time_ms`,
//! `last_update_time_ms`, `system_app`, `app_bytes`, `data_bytes`, `cache_bytes`,
//! `total_bytes` and `icon_png` in base64; `info` prints one such object; `uninstall` prints
//! `package`, `uninstalled`, `status` and `message`; `settings` prints `opened` with the package,
//! or null for the list of apps.
//!
//! `dsa background list` (or `dsa background`) lists the apps that keep running in Android
//! with no window because they run a foreground service (music playback, a download, a VPN),
//! each with the notification the service keeps shown, and the active media sessions. It does
//! not start Android; one that is not running has none. `dsa background open <package>`
//! opens the app's window, and `dsa background stop <package>` stops the app (Android
//! force-stops it, which ends its services and frees its memory). With `--json`, `list`
//! prints `apps` (each with `package`, `label`, `title`, `text`, `actions` and `icon_png` in
//! base64) and `media` (each with `package`, `label`, `title`, `artist`, `album` and
//! `playback`: `playing`, `paused` or `stopped`), and `open` and `stop` print `opened` or
//! `stopped` with the package.
//!
//! `dsa freeze` stops every process of the Android until one of its windows is shown
//! again. Its notifications, alarms and widgets stop with it.
//!
//! `dsa logcat` prints Android's log as logcat's `threadtime` format does, with the year and in
//! UTC: the recent records the service holds, then each new one, until it is stopped (Ctrl+C).
//! Android sends its log only while something reads it, and nothing writes it to a file.
//! `-b` names the buffers, comma separated (`main`, `radio`, `events`, `system`, `crash`,
//! `stats`, `security`, `kernel`, or `all`; `main,system,crash` when not given). Each
//! `TAG:P` keeps the records of that tag at priority `P` and above (`V`, `D`, `I`, `W`, `E`,
//! `F`, or `S` for none), and `*:P` sets it for every other tag, as with logcat. `-d` prints
//! what Android's log holds now and ends. With `--json` each record is one JSON object on a
//! line (`time`, `time_ns`, `buffer`, `priority`, `pid`, `tid`, `uid`, `tag`, `message`, or
//! `dropped` for records that were lost because they came faster than they were read).
//!
//! `dsa logs collect -o <file.zip>` saves what a problem report needs as one zip archive:
//! Android's whole log, its tombstones, ANR traces and dropbox, pstore, a summary of the
//! system services' state, and the DSA logs of this computer that concern the user who runs
//! it. It takes a minute or more; `summary.txt` in the archive says what could not be gathered.
//!
//! `dsa settings` shows the settings the service keeps: the GPU the window manager renders on
//! (`render-gpu`: `display`, `integrated`, `discrete` or part of a GPU's name), whether
//! Android may be frozen when idle (`freeze`), whether text copied on the host can be pasted in
//! Android and the reverse (`clipboard`, on by default), whether web links apps in Android open
//! go to apps in Android instead of the default browser (`links-in-android`, off by default),
//! whether DSA (`program-updates`) and the Android image (`image-updates`) are updated, and
//! whether every DSA process writes the input trace (`input-trace`) and each stage of each
//! resize (`resize-trace`) to its log, off by default. Those last four are the computer's, so
//! only an administrator changes them. `developer-mode` (off by default) lets adb reach the
//! user's Android: while it is on, `adb connect 127.0.0.1:58526` connects, with no prompt to
//! accept in Android. `adb-port` picks another loopback port than 58526. `stop-without-asking` (off by default)
//! is whether stopping an app that runs in the background skips the question that confirms it.
//! `advanced-networking` (off by default) puts the user's Android on this computer's network as
//! a device of its own, so other devices there reach it, instead of behind a NAT network of its
//! own; it applies the next time Android starts.
//! `run-mode` is `as-needed` (the default) or `continuous`: as needed, Android stops once the
//! last app window has closed and no app runs in the background and no widget is placed, after
//! `stop-after` (seconds, or a time such as `5m` or `1h`; five minutes by default), and starts
//! again with the next app; continuous, it stays running in Doze. `keep-for-notifications`
//! (off by default) is whether an app with a notification shown keeps it running as needed.
//! `dsa settings set <name> <value>` changes one;
//! the switches take `on` or `off`. `dsa memory` shows how much memory the user's Android
//! takes, on the host and inside Android. `dsa network` shows how the user's Android reaches
//! the network: NAT, advanced or none while it is not running, its addresses (a program on this
//! computer reaches a server in Android at any of them), its DNS servers and its gateway. With
//! `--json` it prints `mode`, `addresses`, `dns_servers`, `gateway` and `problem`. `dsa gpus` lists the GPUs of this machine, for
//! choosing the render GPU.
//!
//! `dsa arm-translation status` (or `dsa arm-translation`) says whether apps built only for
//! 64-bit ARM (`arm64-v8a`) can be installed and run: which translation package is installed,
//! whether the `arm-translation` setting is on, and which ABIs the running Android installs apps
//! for. `dsa arm-translation install <folder>` installs a translation package: a folder holding
//! `arm-translation.json` and `system.img`, as `dsa/guest/arm-translation/make-package.py`
//! makes one. Android is started first when it is not running. `dsa arm-translation remove`
//! removes it. Both apply from the next start of Android, and say so. With `--json` each prints
//! `package` (`name`, `version`, `library`, `abis`, `license`, or null), `enabled`,
//! `supported` (a package is installed and on), `android` (`installed`, `active` and `abilist`,
//! or null while Android is not running) and `restart_needed`. `dsa settings set
//! arm-translation off` keeps the package but boots Android without it.
//!
//! `dsa settings device` shows the device identity Android reports to its apps: what the user
//! chose, and each value as Android reports it from its next start.
//! `dsa settings set device.<field> <value>` changes one value: `manufacturer`, `brand`,
//! `model`, `device`, `name` (the product name), `characteristics` (the form factor, such as
//! `tablet`), `fingerprint` (the whole string), or one part of the fingerprint
//! (`fingerprint.release`, `fingerprint.id`, `fingerprint.incremental`, `fingerprint.type`,
//! `fingerprint.tags`). `dsa settings reset device` puts every value back to the image's own,
//! and `dsa settings reset device.<field>` one. Android reads them when it starts, so a change
//! applies from its next start. With `--json` it prints `chosen` and `effective`, each with the
//! fields above (`fingerprint_release` and so on for the parts).
//!
//! `dsa reset-data --yes` erases the user's Android data: the apps they installed, their
//! accounts and every app's data. It stops Android, puts a fresh data disk in place of theirs,
//! and the next start boots Android as it was first installed. The Android image and DSA's
//! settings stay. Without `--yes` it only says what would be lost. With `--json` it prints
//! `reset`.
//!
//! `dsa status` says `Active` only once Android has finished booting and its agent is connected.
//! An Android that runs but has taken too long is `Stalled`, and a line after the state says why
//! and what to do; so does `problem` in the JSON. `dsa start --wait` fails on it.
//!
//! `--json` prints what `status`, `start`, `stop`, `settings`, `memory` and `gpus` report as
//! one JSON object, for programs such as the settings app. Errors still go to standard error,
//! with exit code 1.
//!
//! `dsa update` installs the newest version of DSA now instead of when no Android is
//! running. It stops every user's Android, since the service restarts. With `--wait` it
//! waits for the service to come back and shows what it says about the update.
//!
//! The service is found at the platform's usual place: the named pipe `\\.\pipe\dsa-host`
//! on Windows, `/run/dsa/host.sock` on Linux, and
//! `~/Library/Application Support/DSA/host.sock` on macOS. `--pipe <name>` (Windows) and
//! `--socket <path>` (Linux, macOS) point elsewhere, as do the `DSA_PIPE` and `DSA_SOCKET`
//! environment variables.

use std::fmt::Write as _;
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::apk::{ApkFile, InstallKind, InstallOutcome, InstalledPackage};
use dsa_host_core::apk_info::{signing, ApkInfo, Icon, Layer};
use dsa_host_core::app_windows::AppWindowSettings;
use dsa_host_core::arm_translation;
use dsa_host_core::background::{BackgroundApp, BackgroundCommand, MediaSession, Playback};
use dsa_host_core::files::{FileAction, FileTransfer};
use dsa_host_core::host_api::{
    ArmTranslationStatus, ClientError, HostApiClient, NetworkStatus, SettingsReport, Status,
    TaskList,
};
use dsa_host_core::memory::MemorySnapshot;
use dsa_host_core::session::SessionState;
use dsa_host_core::update::versions::{Channel, VersionReport};
use dsa_host_core::tasks::{AppInfo, Task};
use dsa_host_core::widgets::{Widget, WidgetEvent, WidgetProvider};
use dsa_transport::Connector;
use serde_json::{json, Value};

pub const USAGE: &str = "usage: dsa <status | start | stop | freeze | update | \
     launch <package> [--wait-first-frame] | tap <package> <x> <y> | stop-app <package> | \
     install <file.apk> [--progress] | apk-info <file.apk> | open <file> | share <file> | \
     tasks | settings [set <name> <value> | device | reset device[.<field>]] | memory | \
     network | gpus | \
     widgets [list | add <provider> | remove <id> | watch] | \
     apps [list | info <package> | uninstall <package> | settings [<package>]] | \
     background [list | open <package> | stop <package>] | \
     app-window [<package> [--fixed-size on|off] [--start-size <w>x<h>|auto] | <package> reset] | \
     logcat [-d] [-b <buffers>] [<tag>[:<priority>] ...] | logs collect -o <file.zip> | \
     arm-translation [status | install <folder> | remove] | files [open]> \
     arm-translation [status | install <folder> | remove] | reset-data --yes | \
     android [version | use <major>|latest [--erase-data]]> \
     [--wait] [--timeout <seconds>] [--json] [--pipe <name> | --socket <path>]";

pub mod device;
pub mod logs;

/// What to ask the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Status,
    Start,
    Stop,
    /// Freeze the Android until one of its windows is shown again.
    Freeze,
    Update,
    /// Start an app in a window of its own; the package is in [`Invocation::package`].
    Launch,
    /// Press and release at [`Invocation::point`] in a task of [`Invocation::package`].
    Tap,
    StopApp,
    /// Install an APK; the file is in [`Invocation::file`].
    Install,
    /// Say what an APK is, and what installing it would do; the file is in
    /// [`Invocation::file`].
    ApkInfo,
    /// Open a file in an app in Android; the file is in [`Invocation::file`].
    Open,
    /// Share a file with an app in Android; the file is in [`Invocation::file`].
    Share,
    /// List the app tasks Android shows as windows.
    Tasks,
    /// Show the settings, or change one ([`Invocation::change`]).
    Settings,
    /// Show how much memory the user's Android takes.
    Memory,
    /// Show how the user's Android reaches the network: the mode, its addresses and DNS.
    Network,
    /// List this machine's GPUs.
    Gpus,
    /// List, place or remove widgets ([`Invocation::widgets`]).
    Widgets,
    /// List, open or stop the apps that run in the background ([`Invocation::background`]).
    Background,
    /// Print Android's log ([`Invocation::log_filter`], [`Invocation::dump`]).
    Logcat,
    /// Save the diagnostics archive to [`Invocation::output`].
    LogsCollect,
    /// Say whether ARM apps can run, and with which translation package.
    ArmTranslationStatus,
    /// Install the ARM translation package in the folder [`Invocation::file`].
    ArmTranslationInstall,
    /// Remove the ARM translation package.
    ArmTranslationRemove,
    /// List, describe or uninstall the installed apps, or open their settings in Android
    /// ([`Invocation::apps`]).
    Apps,
    /// Show or change the window settings of apps ([`Invocation::app_window`]).
    AppWindow,
    /// Say where the host's file manager shows Android's shared storage.
    Files,
    /// Open Android's shared storage in the host's file manager.
    FilesOpen,
    /// Erase the user's Android data, once [`Invocation::yes`] confirms it.
    ResetData,
    /// Show the Android version that runs and the ones that can be chosen.
    AndroidVersion,
    /// Choose the Android version [`Invocation::android`] names.
    AndroidUse,
}

/// What `dsa app-window` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppWindowAction {
    /// List every app's settings.
    List,
    /// Show one app's.
    Show(String),
    /// Change one app's: whether its windows keep their size, and the size they open at
    /// (`Some(None)` for the size Android gives the task).
    Set {
        package: String,
        fixed_size: Option<bool>,
        start_size: Option<Option<(u32, u32)>>,
    },
    /// Put one app's back to the defaults.
    Reset(String),
}

/// A start size as `dsa app-window --start-size` takes it: `auto`, or `<width>x<height>` in
/// density-independent pixels.
pub fn parse_start_size(value: &str) -> Result<Option<(u32, u32)>, String> {
    let lower = value.trim().to_ascii_lowercase();
    if lower == "auto" {
        return Ok(None);
    }
    let size = lower.split_once('x').and_then(|(width, height)| {
        Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
    });
    match size {
        Some(size) => Ok(Some(size)),
        None => Err(format!(
            "--start-size is auto or a size such as 400x700, not {value:?}"
        )),
    }
}

/// What `dsa apps` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppsAction {
    /// List every app.
    List,
    /// Describe one app.
    Info(String),
    /// Uninstall one app.
    Uninstall(String),
    /// Open Android's settings of one app, or Android's list of every app.
    Settings(Option<String>),
}

/// What `dsa widgets` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetAction {
    /// List the widgets on offer and the ones placed.
    List,
    /// Place a widget of this provider.
    Add(String),
    /// Remove the widget with this ID.
    Remove(i32),
    /// Print the placed widgets, then each change, until stopped.
    Watch,
}

/// What `dsa background` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundAction {
    /// List the apps that run in the background and the media sessions.
    List,
    /// Open this package's window.
    Open(String),
    /// Stop this package.
    Stop(String),
}

/// A setting `dsa settings set` changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    RenderGpu(String),
    Freeze(bool),
    LinksInAndroid(bool),
    Clipboard(bool),
    ProgramUpdates(bool),
    ImageUpdates(bool),
    InputTrace(bool),
    ResizeTrace(bool),
    /// The most memory Android may have, in MiB; 0 for automatic.
    MemoryLimit(u32),
    DeveloperMode(bool),
    AdbPort(u16),
    ArmTranslation(bool),
    /// Show the device identity.
    ShowDevice,
    /// Change one value of the device identity.
    Device(dsa_host_core::device_identity::Field, String),
    /// Put one value of the device identity, or all of them, back to the image's own.
    ResetDevice(Option<dsa_host_core::device_identity::Field>),
    StopWithoutAsking(bool),
    FileSharing(bool),
    /// The folders to share, in the order the service keeps them.
    SharedFolders(Vec<String>),
    AdvancedNetworking(bool),
    RunMode(dsa_host_core::run_mode::RunMode),
    /// The wait before Android stops in "As needed", in seconds.
    StopAfter(u32),
    KeepForNotifications(bool),
}

/// A wait in seconds, or with a unit: `90`, `90s`, `5m`, `1h`.
fn parse_stop_after(value: &str) -> Result<u32, String> {
    let text = value.trim().to_ascii_lowercase();
    let (number, scale) = if let Some(n) = text.strip_suffix('h') {
        (n, 3600)
    } else if let Some(n) = text.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = text.strip_suffix('s') {
        (n, 1)
    } else {
        (text.as_str(), 1)
    };
    let secs = number
        .trim()
        .parse::<u32>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .ok_or_else(|| format!("stop-after is a time such as 300, 90s, 5m or 1h, not {value:?}"))?;
    let (min, max) = (
        dsa_host_core::run_mode::MIN_STOP_AFTER.as_secs() as u32,
        dsa_host_core::run_mode::MAX_STOP_AFTER.as_secs() as u32,
    );
    if !(min..=max).contains(&secs) {
        return Err(format!(
            "stop-after is from {min} seconds to {} hours, not {value:?}",
            max / 3600
        ));
    }
    Ok(secs)
}

impl Change {
    fn parse(name: &str, value: &str) -> Result<Change, String> {
        let switch = || match value.to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => Ok(true),
            "off" | "false" | "no" | "0" => Ok(false),
            _ => Err(format!("{name} is on or off, not {value:?}")),
        };
        if let Some(field) = name.strip_prefix("device.") {
            return device::parse_set(field, value);
        }
        match name {
            "render-gpu" => Ok(Change::RenderGpu(value.to_string())),
            "freeze" => switch().map(Change::Freeze),
            "clipboard" => switch().map(Change::Clipboard),
            "links-in-android" => switch().map(Change::LinksInAndroid),
            "program-updates" => switch().map(Change::ProgramUpdates),
            "image-updates" => switch().map(Change::ImageUpdates),
            "input-trace" => switch().map(Change::InputTrace),
            "resize-trace" => switch().map(Change::ResizeTrace),
            "memory-limit" => parse_memory_limit(value).map(Change::MemoryLimit),
            "developer-mode" => switch().map(Change::DeveloperMode),
            "arm-translation" => switch().map(Change::ArmTranslation),
            "stop-without-asking" => switch().map(Change::StopWithoutAsking),
            "file-sharing" => switch().map(Change::FileSharing),
            "shared-folders" => parse_shared_folders(value).map(Change::SharedFolders),
            "advanced-networking" => switch().map(Change::AdvancedNetworking),
            "run-mode" => dsa_host_core::run_mode::RunMode::parse(value).map(Change::RunMode),
            "stop-after" => parse_stop_after(value).map(Change::StopAfter),
            "keep-for-notifications" => switch().map(Change::KeepForNotifications),
            "adb-port" => match value.parse::<u16>() {
                Ok(port) if port > 0 => Ok(Change::AdbPort(port)),
                _ => Err(format!(
                    "adb-port is a TCP port from 1 to 65535, not {value:?}"
                )),
            },
            _ => Err(format!(
                "there is no setting {name:?}; the settings are render-gpu, freeze, \
                 clipboard, links-in-android, program-updates, image-updates, input-trace, \
                 resize-trace, memory-limit, developer-mode, adb-port, arm-translation, \
                 stop-without-asking, file-sharing, shared-folders, run-mode, stop-after, \
                 keep-for-notifications, advanced-networking and device.<field>"
            )),
        }
    }
}

/// The folders `dsa settings set shared-folders` takes: `all`, `none`, or names separated by
/// commas, such as `Documents,Pictures`.
fn parse_shared_folders(value: &str) -> Result<Vec<String>, String> {
    let names: Vec<String> = match value.trim().to_ascii_lowercase().as_str() {
        "all" => return Ok(dsa_host_core::file_share::all_folders()),
        "none" | "" => return Ok(Vec::new()),
        _ => value
            .split(',')
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect(),
    };
    dsa_host_core::file_share::normalise_folders(&names).map_err(|why| {
        format!("shared-folders is all, none or a list such as Documents,Pictures: {why}")
    })
}

/// A memory limit as `dsa settings set memory-limit` takes it: `auto` (the service's
/// default size), or a size in MiB, with `G` or `GB` for GiB.
fn parse_memory_limit(value: &str) -> Result<u32, String> {
    let lower = value.trim().to_ascii_lowercase();
    if lower == "auto" || lower == "0" {
        return Ok(0);
    }
    let (number, scale) = match lower.strip_suffix("gb").or_else(|| lower.strip_suffix('g')) {
        Some(gib) => (gib, 1024),
        None => (lower.strip_suffix("mb").unwrap_or(&lower), 1),
    };
    number
        .trim()
        .parse::<u32>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .filter(|mb| *mb >= 2048)
        .ok_or_else(|| {
            format!(
                "memory-limit is auto or a size of at least 2048 MiB (such as 8192 or 8G), \
                 not {value:?}"
            )
        })
}

/// What can start a host service or ensure it is running.
pub trait ServiceStarter: Send + Sync {
    /// Starts the service if it is not already running, waiting until it enters the
    /// running state or its local transport is available.
    fn start_service(&self) -> Result<(), String>;
}

#[cfg(windows)]
impl ServiceStarter for dsa_host_windows::scm::WindowsServiceStarter {
    fn start_service(&self) -> Result<(), String> {
        self.start()
    }
}

/// The default service starter for this platform, when using the standard endpoint.
pub fn default_starter(endpoint: Option<&str>) -> Option<Box<dyn ServiceStarter>> {
    if endpoint.is_some() {
        return None;
    }
    #[cfg(windows)]
    {
        Some(Box::new(dsa_host_windows::scm::WindowsServiceStarter::new(
            "DsaHost",
            Duration::from_secs(30),
        )))
    }
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(LaunchAgentStarter {
            wait: Duration::from_secs(30),
        }))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// Starts the user's launchd agent on macOS when its socket is not there, and waits for the
/// socket.
#[cfg(target_os = "macos")]
pub struct LaunchAgentStarter {
    pub wait: Duration,
}

#[cfg(target_os = "macos")]
impl ServiceStarter for LaunchAgentStarter {
    fn start_service(&self) -> Result<(), String> {
        let socket = default_socket()?;
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            return Ok(());
        }
        let uid = dsa_host_unix::socket::current_uid();
        dsa_host_macos::launchd::launchctl(&dsa_host_macos::launchd::start_command(uid)).map_err(
            |why| {
                format!(
                    "{why}; install the agent with `dsa-hostd install-agent` if it is not \
                     installed"
                )
            },
        )?;
        let deadline = Instant::now() + self.wait;
        while Instant::now() < deadline {
            if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }
        Err(format!(
            "the DSA agent did not open {} within {} seconds",
            socket.display(),
            self.wait.as_secs()
        ))
    }
}

/// How long the service may take to answer `dsa update`: it may download the setup file
/// first.
const UPDATE_ANSWER_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// How long `dsa start --wait` waits unless told otherwise: longer than Android may take to
/// finish booting before it is reported as stalled, so that report is what ends the wait.
const START_TIMEOUT: Duration = Duration::from_secs(360);

/// How long the other waits last unless told otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    /// Wait until the Android has started or stopped, instead of returning at once.
    pub wait: bool,
    pub timeout: Duration,
    /// Where the service is, when not at the usual place.
    pub endpoint: Option<String>,
    /// The app to launch, for `dsa launch`.
    pub package: Option<String>,
    /// The file to install, open or share, for `dsa install`, `dsa open` and `dsa share`.
    pub file: Option<std::path::PathBuf>,
    /// The setting to change, for `dsa settings set`.
    pub change: Option<Change>,
    /// What to do with widgets, for `dsa widgets`.
    pub widgets: Option<WidgetAction>,
    /// What to do with the installed apps, for `dsa apps`.
    pub apps: Option<AppsAction>,
    /// What to do with the apps that run in the background, for `dsa background`.
    pub background: Option<BackgroundAction>,
    /// What to do with the window settings of apps, for `dsa app-window`.
    pub app_window: Option<AppWindowAction>,
    /// Print the answer as JSON.
    pub json: bool,
    /// Print each step of `dsa install` as it happens.
    pub progress: bool,
    /// Which records `dsa logcat` prints.
    pub log_filter: Option<dsa_host_core::guest_log::LogFilter>,
    /// `dsa logcat -d`: print what Android's log holds now and end.
    pub dump: bool,
    /// Where `dsa logs collect` saves the archive.
    pub output: Option<std::path::PathBuf>,
    /// `dsa launch --wait-first-frame`: wait for the new window's first frame and say when it
    /// came.
    pub wait_first_frame: bool,
    /// Where `dsa tap` presses, in Android display pixels from the task's top left corner.
    pub point: Option<(u32, u32)>,
    /// `dsa reset-data --yes`: the user confirmed that their Android data goes.
    pub yes: bool,
    /// `dsa android use <major>`: the version to follow.
    pub android: Option<Channel>,
    /// `dsa android use <major> --erase-data`: the user agreed that moving to an older Android
    /// erases its data.
    pub erase_data: bool,
}

/// Whether `name` can be an Android package name: dot-separated parts of letters, digits
/// and underscores.
fn is_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Parses the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut command = None;
    let mut wait = false;
    let mut timeout = None;
    let mut endpoint = None;
    let mut package = None;
    let mut file = None;
    let mut change = None;
    let mut widgets = None;
    let mut apps = None;
    let mut background = None;
    let mut app_window: Option<AppWindowAction> = None;
    let mut json = false;
    let mut yes = false;
    let mut android = None;
    let mut erase_data = false;
    let mut progress = false;
    let mut buffers: u32 = 0;
    let mut tags = Vec::new();
    let mut dump = false;
    let mut output = None;
    let mut wait_first_frame = false;
    let mut coordinates: Vec<u32> = Vec::new();
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "logcat" if command.is_none() => command = Some(Command::Logcat),
            "app-window" if command.is_none() => command = Some(Command::AppWindow),
            name if command == Some(Command::AppWindow)
                && app_window.is_none()
                && !name.starts_with('-') =>
            {
                if !is_package_name(name) {
                    return Err(format!(
                        "{name:?} is not a package name; dsa apps list shows them, such as \
                         com.example.notes"
                    ));
                }
                app_window = Some(AppWindowAction::Show(name.to_string()));
            }
            "reset" if command == Some(Command::AppWindow) => match app_window.take() {
                Some(AppWindowAction::Show(package)) => {
                    app_window = Some(AppWindowAction::Reset(package));
                }
                _ => return Err("dsa app-window reset follows the app's package".into()),
            },
            flag @ ("--fixed-size" | "--start-size") if command == Some(Command::AppWindow) => {
                let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
                let (package, mut fixed_size, mut start_size) = match app_window.take() {
                    Some(AppWindowAction::Show(package)) => (package, None, None),
                    Some(AppWindowAction::Set {
                        package,
                        fixed_size,
                        start_size,
                    }) => (package, fixed_size, start_size),
                    _ => return Err(format!("dsa app-window {flag} follows the app's package")),
                };
                if flag == "--fixed-size" {
                    fixed_size = Some(match value.to_ascii_lowercase().as_str() {
                        "on" | "true" | "yes" | "1" => true,
                        "off" | "false" | "no" | "0" => false,
                        _ => return Err(format!("--fixed-size is on or off, not {value:?}")),
                    });
                } else {
                    start_size = Some(parse_start_size(value)?);
                }
                app_window = Some(AppWindowAction::Set {
                    package,
                    fixed_size,
                    start_size,
                });
            }
            "apps" if command.is_none() => command = Some(Command::Apps),
            "files" if command.is_none() => command = Some(Command::Files),
            "open" if command == Some(Command::Files) => command = Some(Command::FilesOpen),
            "list" if command == Some(Command::Apps) && apps.is_none() => {
                apps = Some(AppsAction::List);
            }
            action @ ("info" | "uninstall") if command == Some(Command::Apps) && apps.is_none() => {
                let package = args.next().ok_or_else(|| {
                    format!(
                        "dsa apps {action} needs the app's package, such as com.example.notes; \
                         dsa apps list shows them"
                    )
                })?;
                if !is_package_name(package) {
                    return Err(format!(
                        "{package:?} is not a package name; dsa apps list shows them, such as \
                         com.example.notes"
                    ));
                }
                apps = Some(if action == "info" {
                    AppsAction::Info(package.clone())
                } else {
                    AppsAction::Uninstall(package.clone())
                });
            }
            "settings" if command == Some(Command::Apps) && apps.is_none() => {
                apps = Some(AppsAction::Settings(None));
            }
            name if command == Some(Command::Apps)
                && apps == Some(AppsAction::Settings(None))
                && !name.starts_with('-') =>
            {
                if !is_package_name(name) {
                    return Err(format!(
                        "{name:?} is not a package name; dsa apps list shows them, such as \
                         com.example.notes"
                    ));
                }
                apps = Some(AppsAction::Settings(Some(name.to_string())));
            }
            "logs" if command.is_none() => match args.next().map(String::as_str) {
                Some("collect") => command = Some(Command::LogsCollect),
                _ => return Err(format!("dsa logs takes collect -o <file.zip>\n{USAGE}")),
            },
            "arm-translation" if command.is_none() => {
                // A bare option such as --json belongs to the default action, status.
                let action = match args.peek() {
                    Some(next) if next.starts_with("--") => None,
                    _ => args.next().map(String::as_str),
                };
                command = Some(match action {
                    None | Some("status") => Command::ArmTranslationStatus,
                    Some("install") => {
                        let folder = args.next().ok_or(
                            "dsa arm-translation install needs the folder of the translation \
                             package",
                        )?;
                        file = Some(std::path::PathBuf::from(folder));
                        Command::ArmTranslationInstall
                    }
                    Some("remove") => Command::ArmTranslationRemove,
                    Some(other) => {
                        return Err(format!(
                            "dsa arm-translation takes status, install <folder> or remove, not \
                             {other:?}\n{USAGE}"
                        ))
                    }
                });
            }
            "-d" if command == Some(Command::Logcat) => dump = true,
            "-b" if command == Some(Command::Logcat) => {
                let names = args
                    .next()
                    .ok_or("-b needs the log buffers, such as main,system,crash")?;
                buffers |= dsa_host_core::guest_log::LogFilter::parse_buffers(names)?;
            }
            "-o" | "--output" if command == Some(Command::LogsCollect) => {
                let path = args
                    .next()
                    .ok_or("dsa logs collect -o needs the file to save the archive to")?;
                output = Some(std::path::PathBuf::from(path));
            }
            spec if command == Some(Command::Logcat) && !spec.starts_with('-') => {
                tags.push(dsa_host_core::guest_log::LogFilter::parse_spec(spec)?);
            }
            "status" | "start" | "stop" | "freeze" | "update" | "launch" | "tap" | "stop-app"
            | "install" | "apk-info" | "open" | "share" | "tasks" | "settings" | "memory"
            | "gpus" | "widgets" | "background" | "network"
                if command.is_none() =>
            {
                command = Some(match arg.as_str() {
                    "status" => Command::Status,
                    "start" => Command::Start,
                    "freeze" => Command::Freeze,
                    "update" => Command::Update,
                    "launch" => Command::Launch,
                    "tap" => Command::Tap,
                    "stop-app" => Command::StopApp,
                    "install" => Command::Install,
                    "apk-info" => Command::ApkInfo,
                    "open" => Command::Open,
                    "share" => Command::Share,
                    "tasks" => Command::Tasks,
                    "settings" => Command::Settings,
                    "memory" => Command::Memory,
                    "network" => Command::Network,
                    "gpus" => Command::Gpus,
                    "widgets" => Command::Widgets,
                    "background" => Command::Background,
                    _ => Command::Stop,
                });
            }
            "set" if command == Some(Command::Settings) && change.is_none() => {
                let name = args
                    .next()
                    .ok_or("dsa settings set needs a setting and a value")?;
                let value = args
                    .next()
                    .ok_or_else(|| format!("dsa settings set {name} needs a value"))?;
                change = Some(Change::parse(name, value)?);
            }
            "reset" if command == Some(Command::Settings) && change.is_none() => {
                let name = args
                    .next()
                    .ok_or("dsa settings reset needs device or device.<field>")?;
                change = Some(device::parse_reset(name)?);
            }
            "device" if command == Some(Command::Settings) && change.is_none() => {
                change = Some(Change::ShowDevice);
            }
            "list" if command == Some(Command::Widgets) && widgets.is_none() => {
                widgets = Some(WidgetAction::List);
            }
            "watch" if command == Some(Command::Widgets) && widgets.is_none() => {
                widgets = Some(WidgetAction::Watch);
            }
            "add" if command == Some(Command::Widgets) && widgets.is_none() => {
                let provider = args.next().ok_or(
                    "dsa widgets add needs the widget's provider; dsa widgets list shows them",
                )?;
                if !dsa_host_core::widgets::valid_provider_name(provider) {
                    return Err(format!(
                        "{provider:?} is not a widget's provider; dsa widgets list shows them, \
                         such as com.example.clock/.ClockWidget"
                    ));
                }
                widgets = Some(WidgetAction::Add(provider.clone()));
            }
            "remove" if command == Some(Command::Widgets) && widgets.is_none() => {
                let id = args.next().ok_or(
                    "dsa widgets remove needs the widget's ID; dsa widgets list shows them",
                )?;
                let id: i32 = id
                    .parse()
                    .ok()
                    .filter(|id: &i32| *id > 0)
                    .ok_or_else(|| format!("a widget's ID is a number above 0, not {id:?}"))?;
                widgets = Some(WidgetAction::Remove(id));
            }
            "list" if command == Some(Command::Background) && background.is_none() => {
                background = Some(BackgroundAction::List);
            }
            verb @ ("open" | "stop")
                if command == Some(Command::Background) && background.is_none() =>
            {
                let name = args.next().ok_or_else(|| {
                    format!(
                        "dsa background {verb} needs the app's package; dsa background lists \
                         them"
                    )
                })?;
                if !dsa_host_core::background::valid_package(name) || !is_package_name(name) {
                    return Err(format!(
                        "{name:?} is not a package name; name the app by its package, such as \
                         com.example.music"
                    ));
                }
                background = Some(if verb == "open" {
                    BackgroundAction::Open(name.clone())
                } else {
                    BackgroundAction::Stop(name.clone())
                });
            }
            "--json" => json = true,
            "--yes" if command == Some(Command::ResetData) => yes = true,
            "android" if command.is_none() => {
                let action = match args.peek() {
                    Some(next) if next.starts_with("--") => None,
                    _ => args.next().map(String::as_str),
                };
                command = Some(match action {
                    None | Some("version") => Command::AndroidVersion,
                    Some("use") => {
                        let chosen = args.next().ok_or(
                            "dsa android use needs the Android major version, such as 17, or \
                             latest; dsa android version lists the supported ones",
                        )?;
                        android = Some(Channel::parse(chosen).ok_or_else(|| {
                            format!(
                                "{chosen:?} is not an Android major version; give a number such \
                                 as 17, or latest"
                            )
                        })?);
                        Command::AndroidUse
                    }
                    Some(other) => {
                        return Err(format!(
                            "dsa android takes version or use <major>, not {other:?}\n{USAGE}"
                        ))
                    }
                });
            }
            "--erase-data" if command == Some(Command::AndroidUse) => erase_data = true,
            "reset-data" if command.is_none() => command = Some(Command::ResetData),
            "--progress" if command == Some(Command::Install) => progress = true,
            "--wait-first-frame" if command == Some(Command::Launch) => wait_first_frame = true,
            number
                if command == Some(Command::Tap)
                    && package.is_some()
                    && coordinates.len() < 2
                    && !number.starts_with('-') =>
            {
                let value: u32 = number
                    .parse()
                    .map_err(|_| format!("a point is two whole numbers, not {number:?}"))?;
                coordinates.push(value);
            }
            name if matches!(
                command,
                Some(Command::Launch | Command::Tap | Command::StopApp)
            ) && package.is_none()
                && !name.starts_with('-') =>
            {
                if !is_package_name(name) {
                    return Err(format!(
                        "{name:?} is not a package name; name the app by its package, such as \
                         com.android.settings"
                    ));
                }
                package = Some(name.to_string());
            }
            name if matches!(command, Some(Command::Install | Command::ApkInfo))
                && file.is_none()
                && !name.starts_with("--") =>
            {
                file = Some(std::path::PathBuf::from(name));
            }
            name if matches!(command, Some(Command::Open | Command::Share))
                && file.is_none()
                && !name.starts_with('-') =>
            {
                file = Some(std::path::PathBuf::from(name));
            }
            "--wait" => wait = true,
            "--timeout" => {
                let value = args.next().ok_or("--timeout needs a number of seconds")?;
                let seconds: u64 = value
                    .parse()
                    .map_err(|_| format!("--timeout needs a number of seconds, not {value:?}"))?;
                timeout = Some(Duration::from_secs(seconds));
            }
            "--pipe" | "--socket" => {
                let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
                endpoint = Some(value.clone());
            }
            other => return Err(format!("unexpected {other:?}\n{USAGE}")),
        }
    }
    let command = command.ok_or(USAGE)?;
    let widgets = (command == Command::Widgets).then(|| widgets.unwrap_or(WidgetAction::List));
    let apps = (command == Command::Apps).then(|| apps.unwrap_or(AppsAction::List));
    let app_window =
        (command == Command::AppWindow).then(|| app_window.unwrap_or(AppWindowAction::List));
    let background =
        (command == Command::Background).then(|| background.unwrap_or(BackgroundAction::List));
    if command == Command::Install && file.is_none() {
        return Err(format!(
            "dsa install needs the APK file to install\n{USAGE}"
        ));
    }
    if command == Command::ApkInfo && file.is_none() {
        return Err(format!("dsa apk-info needs the APK file to read\n{USAGE}"));
    }
    if matches!(command, Command::Open | Command::Share) && file.is_none() {
        return Err(format!(
            "dsa {} needs the file to send to Android\n{USAGE}",
            if command == Command::Open {
                "open"
            } else {
                "share"
            }
        ));
    }
    if command == Command::LogsCollect && output.is_none() {
        return Err(format!(
            "dsa logs collect needs the file to save the archive to, such as -o diag.zip\n{USAGE}"
        ));
    }
    let filter = dsa_host_core::guest_log::LogFilter {
        buffers: if buffers == 0 {
            dsa_host_core::guest_log::DEFAULT_BUFFERS
        } else {
            buffers
        },
        tags,
    };
    let log_filter = (command == Command::Logcat).then_some(filter);
    if command == Command::StopApp && package.is_none() {
        return Err(format!(
            "dsa stop-app needs the package of the app to stop, such as com.example.music\n\
             {USAGE}"
        ));
    }
    if command == Command::Launch && package.is_none() {
        return Err(format!(
            "dsa launch needs the package of the app to start, such as com.android.settings\n\
             {USAGE}"
        ));
    }
    if command == Command::Tap && (package.is_none() || coordinates.len() != 2) {
        return Err(format!(
            "dsa tap needs the app's package and the point to press, such as \
             dsa tap com.android.settings 200 300\n{USAGE}"
        ));
    }
    let point = match coordinates[..] {
        [x, y] if command == Command::Tap => Some((x, y)),
        _ => None,
    };
    Ok(Invocation {
        command,
        wait,
        timeout: timeout.unwrap_or(if matches!(command, Command::Start | Command::Install) {
            START_TIMEOUT
        } else {
            DEFAULT_TIMEOUT
        }),
        endpoint,
        package,
        file,
        change,
        widgets,
        apps,
        background,
        app_window,
        json,
        progress,
        log_filter,
        dump,
        output,
        wait_first_frame,
        point,
        yes,
        android,
        erase_data,
    })
}

/// Encodes `data` in standard base64, for the previews `dsa widgets list --json` prints.
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A widget on offer as one JSON object.
pub fn provider_json(provider: &WidgetProvider) -> Value {
    json!({
        "provider": provider.provider,
        "package": provider.package_name,
        "label": provider.label,
        "preview_png": base64(&provider.preview_png),
        "min_width_dp": provider.min_width_dp,
        "min_height_dp": provider.min_height_dp,
        "default_width_dp": provider.default_width_dp,
        "default_height_dp": provider.default_height_dp,
        "max_width_dp": provider.max_width_dp,
        "max_height_dp": provider.max_height_dp,
        "resize_mode": provider.resize_mode,
        "needs_configuration": provider.needs_configuration,
        "app": provider.app_label,
    })
}

/// A placed widget as one JSON object.
pub fn widget_json(widget: &Widget) -> Value {
    json!({
        "id": widget.widget_id,
        "provider": widget.provider.provider,
        "package": widget.provider.package_name,
        "label": widget.provider.label,
        "app": widget.provider.app_label,
        "placement": {
            "x": widget.placement.x,
            "y": widget.placement.y,
            "width_dp": widget.placement.width_dp,
            "height_dp": widget.placement.height_dp,
        },
    })
}

/// What `dsa widgets list` prints: the widgets on offer, then the ones placed.
pub fn describe_widgets(providers: &[WidgetProvider], placed: &[Widget]) -> String {
    let mut out = String::new();
    if providers.is_empty() {
        out.push_str("No app in Android offers a widget.\n");
    } else {
        out.push_str("Widgets on offer (dsa widgets add <provider>):\n");
        for provider in providers {
            let _ = writeln!(
                out,
                "  {}  {} ({}x{} dp){}",
                provider.provider,
                provider.label,
                provider.default_width_dp,
                provider.default_height_dp,
                if provider.needs_configuration {
                    ", set up when placed"
                } else {
                    ""
                }
            );
        }
    }
    if placed.is_empty() {
        out.push_str("No widget is placed.");
    } else {
        out.push_str("Placed widgets (dsa widgets remove <id>):");
        for widget in placed {
            let _ = write!(
                out,
                "\n  {}  {} ({}) at {},{} {}x{} dp",
                widget.widget_id,
                widget.provider.label,
                widget.provider.provider,
                widget.placement.x,
                widget.placement.y,
                widget.placement.width_dp,
                widget.placement.height_dp
            );
        }
    }
    out
}

/// `dsa widgets`: makes sure Android runs (and, on Windows, the window manager that shows the
/// widgets), then lists, places or removes widgets.
fn widgets(
    invocation: &Invocation,
    mut client: HostApiClient,
    connector: &mut dyn Connector,
) -> Result<String, String> {
    let action = invocation.widgets.clone().unwrap_or(WidgetAction::List);
    #[cfg(windows)]
    if invocation.endpoint.is_none()
        && matches!(action, WidgetAction::Add(_) | WidgetAction::Remove(_))
    {
        ensure_window_manager()?;
    }
    client
        .ensure_running(OPEN_START_TIMEOUT, Duration::from_millis(250), |_| {})
        .map_err(|e| format!("Android did not start: {e}"))?;
    drop(client);
    let mut client = connect(connector, None, false, Duration::from_secs(10))?;
    match action {
        WidgetAction::List => {
            let providers = client.widget_providers().map_err(|e| e.to_string())?;
            let (placed, _) = client.widgets().map_err(|e| e.to_string())?;
            Ok(if invocation.json {
                json!({
                    "providers": providers.iter().map(provider_json).collect::<Vec<_>>(),
                    "widgets": placed.iter().map(widget_json).collect::<Vec<_>>(),
                })
                .to_string()
            } else {
                describe_widgets(&providers, &placed)
            })
        }
        WidgetAction::Add(provider) => {
            let widget = client
                .add_widget(
                    &provider,
                    dsa_host_core::host_api::WIDGET_ADD_WAIT + Duration::from_secs(10),
                )
                .map_err(|e| format!("the widget was not placed: {e}"))?;
            Ok(if invocation.json {
                widget_json(&widget).to_string()
            } else {
                format!(
                    "Placed {} (widget {})",
                    if widget.provider.label.is_empty() {
                        &widget.provider.provider
                    } else {
                        &widget.provider.label
                    },
                    widget.widget_id
                )
            })
        }
        WidgetAction::Remove(id) => {
            client.remove_widget(id).map_err(|e| e.to_string())?;
            Ok(if invocation.json {
                json!({ "removed": id }).to_string()
            } else {
                format!("Removed widget {id}")
            })
        }
        WidgetAction::Watch => {
            let mut stream = client.watch_widgets().map_err(|e| e.to_string())?;
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let first = widgets_now_line(stream.current(), invocation.json);
            if !print_line(&mut out, &first) {
                return Ok(String::new());
            }
            loop {
                match stream.next(Duration::from_secs(60)) {
                    Ok(Some(event)) => {
                        if let Some(line) = widget_event_line(&event, invocation.json) {
                            if !print_line(&mut out, &line) {
                                return Ok(String::new());
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => return Err(format!("the watch on the widgets ended: {e}")),
                }
            }
        }
    }
}

/// Writes one line and flushes it, so a program reading the output sees it at once. False
/// once the output is closed.
fn print_line(out: &mut dyn std::io::Write, line: &str) -> bool {
    writeln!(out, "{line}").is_ok() && out.flush().is_ok()
}

/// The first line `dsa widgets watch` prints: the widgets placed now.
pub fn widgets_now_line(placed: &[Widget], json: bool) -> String {
    if json {
        json!({
            "event": "widgets",
            "widgets": placed.iter().map(widget_json).collect::<Vec<_>>(),
        })
        .to_string()
    } else if placed.is_empty() {
        "No widget is placed.".to_string()
    } else {
        let ids: Vec<String> = placed.iter().map(|w| w.widget_id.to_string()).collect();
        format!("Placed widgets: {}", ids.join(", "))
    }
}

/// The line `dsa widgets watch` prints for a change, or None for one it does not report
/// (a widget's new picture).
pub fn widget_event_line(event: &WidgetEvent, json: bool) -> Option<String> {
    match event {
        WidgetEvent::Placed(widget) => Some(if json {
            json!({ "event": "placed", "widget": widget_json(widget) }).to_string()
        } else {
            format!(
                "Placed widget {} ({})",
                widget.widget_id,
                if widget.provider.label.is_empty() {
                    &widget.provider.provider
                } else {
                    &widget.provider.label
                }
            )
        }),
        WidgetEvent::Removed { widget_id } => Some(if json {
            json!({ "event": "removed", "id": widget_id }).to_string()
        } else {
            format!("Removed widget {widget_id}")
        }),
        WidgetEvent::Frame(_) | WidgetEvent::Listed(_) => None,
    }
}

/// An installed app as one JSON object. With `icon`, the icon is in it as base64 PNG.
pub fn app_json(app: &AppInfo, icon: bool) -> Value {
    let mut value = json!({
        "package": app.package_name,
        "label": app.label,
        "version_name": app.version_name,
        "version_code": app.version_code,
        "first_install_time_ms": app.first_install_time_ms,
        "last_update_time_ms": app.last_update_time_ms,
        "system_app": app.system_app,
        "app_bytes": app.app_bytes,
        "data_bytes": app.data_bytes,
        "cache_bytes": app.cache_bytes,
        "total_bytes": app.total_bytes(),
    });
    if icon {
        value["icon_png"] = Value::String(base64(&app.icon_png));
    }
    value
}

/// A size in bytes as people read it: `3.2 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// What `dsa apps list` prints: one app a line.
pub fn describe_apps(apps: &[AppInfo]) -> String {
    if apps.is_empty() {
        return "No app is installed in Android.".into();
    }
    let mut out = String::from("Installed apps (dsa apps info <package>):");
    for app in apps {
        let _ = write!(
            out,
            "\n  {}  {}{}  {}{}",
            app.package_name,
            app.label,
            if app.version_name.is_empty() {
                String::new()
            } else {
                format!(" {}", app.version_name)
            },
            human_size(app.total_bytes()),
            if app.system_app { "  (system)" } else { "" }
        );
    }
    out
}

/// What `dsa apps info` prints.
pub fn describe_app(app: &AppInfo) -> String {
    let date = |ms: u64| {
        if ms == 0 {
            "unknown".to_string()
        } else {
            dsa_host_core::guest_log::utc_time(ms.saturating_mul(1_000_000))
                .chars()
                .take(10)
                .collect()
        }
    };
    [
        format!("{} ({})", app.label, app.package_name),
        format!(
            "Version: {} ({})",
            if app.version_name.is_empty() {
                "unknown"
            } else {
                &app.version_name
            },
            app.version_code
        ),
        format!(
            "Size: {} (app {}, data {}, cache {})",
            human_size(app.total_bytes()),
            human_size(app.app_bytes),
            human_size(app.data_bytes),
            human_size(app.cache_bytes)
        ),
        format!("Installed: {}", date(app.first_install_time_ms)),
        format!("Updated: {}", date(app.last_update_time_ms)),
        if app.system_app {
            "Part of Android: it cannot be uninstalled".to_string()
        } else {
            "Installed by you: dsa apps uninstall removes it".to_string()
        },
    ]
    .join("\n")
}

/// `dsa apps`: makes sure Android runs, then lists or describes its apps with their sizes read
/// again, uninstalls one, or opens Android's settings of one (and, on Windows, the window
/// manager that shows them).
fn apps(
    invocation: &Invocation,
    mut client: HostApiClient,
    connector: &mut dyn Connector,
) -> Result<String, String> {
    let action = invocation.apps.clone().unwrap_or(AppsAction::List);
    #[cfg(windows)]
    if invocation.endpoint.is_none() && matches!(action, AppsAction::Settings(_)) {
        ensure_window_manager()?;
    }
    client
        .ensure_running(OPEN_START_TIMEOUT, Duration::from_millis(250), |_| {})
        .map_err(|e| format!("Android did not start: {e}"))?;
    drop(client);
    let mut client = connect(connector, None, false, Duration::from_secs(10))?;
    match action {
        AppsAction::List => {
            let apps = client
                .refreshed_apps()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            Ok(if invocation.json {
                json!({ "apps": apps.iter().map(|app| app_json(app, true)).collect::<Vec<_>>() })
                    .to_string()
            } else {
                describe_apps(&apps)
            })
        }
        AppsAction::Info(package) => {
            let apps = client
                .refreshed_apps()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            let app = apps
                .iter()
                .find(|app| app.package_name == package)
                .ok_or_else(|| {
                    format!("no app of {package} is installed in Android; dsa apps list shows them")
                })?;
            Ok(if invocation.json {
                app_json(app, true).to_string()
            } else {
                describe_app(app)
            })
        }
        AppsAction::Uninstall(package) => {
            let outcome = client.uninstall_app(&package).map_err(|e| e.to_string())?;
            if !outcome.uninstalled() {
                return Err(format!(
                    "not uninstalled ({}): {}",
                    outcome.status.code(),
                    outcome
                ));
            }
            Ok(if invocation.json {
                json!({
                    "package": outcome.package_name,
                    "uninstalled": true,
                    "status": outcome.status.code(),
                    "message": outcome.message,
                })
                .to_string()
            } else {
                format!("Uninstalled {package}")
            })
        }
        AppsAction::Settings(package) => {
            client
                .open_app_settings(package.as_deref())
                .map_err(|e| e.to_string())?;
            Ok(if invocation.json {
                json!({ "opened": package }).to_string()
            } else {
                match package {
                    Some(package) => format!("Opening the settings of {package}"),
                    None => "Opening the list of apps".into(),
                }
            })
        }
    }
}

/// `dsa background`: lists the apps that run in the background, or opens or stops one. It
/// never starts Android: an app runs in the background only in an Android that runs.
/// What `dsa app-window` prints of one app's window settings.
fn app_window_json(app: &AppWindowSettings) -> Value {
    json!({
        "package": app.package_name,
        "fixed_size": app.fixed_size,
        "start_size": app.start_size_dp.map(|(width, height)| json!([width, height])),
    })
}

fn describe_app_window(app: &AppWindowSettings) -> String {
    let size = match app.start_size_dp {
        Some((width, height)) => format!("opens at {width} x {height} dp"),
        None => "opens at the size Android gives it".to_string(),
    };
    let resizable = if app.fixed_size {
        "fixed size"
    } else {
        "resizable"
    };
    format!("{}: {resizable}, {size}", app.package_name)
}

fn app_window(invocation: &Invocation, mut client: HostApiClient) -> Result<String, String> {
    let action = invocation
        .app_window
        .clone()
        .unwrap_or(AppWindowAction::List);
    let apps = client.app_window_settings().map_err(|e| e.to_string())?;
    let defaults = |package: &str| AppWindowSettings {
        package_name: package.to_string(),
        ..AppWindowSettings::default()
    };
    let shown: Vec<AppWindowSettings> = match action {
        AppWindowAction::List => apps,
        AppWindowAction::Show(package) => vec![apps
            .into_iter()
            .find(|app| app.package_name == package)
            .unwrap_or_else(|| defaults(&package))],
        AppWindowAction::Reset(package) => {
            client
                .set_app_window_settings(&defaults(&package))
                .map_err(|e| e.to_string())?;
            vec![defaults(&package)]
        }
        AppWindowAction::Set {
            package,
            fixed_size,
            start_size,
        } => {
            let current = apps
                .into_iter()
                .find(|app| app.package_name == package)
                .unwrap_or_else(|| defaults(&package));
            let wanted = AppWindowSettings {
                fixed_size: fixed_size.unwrap_or(current.fixed_size),
                start_size_dp: start_size.unwrap_or(current.start_size_dp),
                ..current
            };
            let now = client
                .set_app_window_settings(&wanted)
                .map_err(|e| e.to_string())?;
            vec![now
                .into_iter()
                .find(|app| app.package_name == package)
                .unwrap_or_else(|| defaults(&package))]
        }
    };
    Ok(if invocation.json {
        json!({ "apps": shown.iter().map(app_window_json).collect::<Vec<_>>() }).to_string()
    } else if shown.is_empty() {
        "No app has window settings of its own".to_string()
    } else {
        shown
            .iter()
            .map(describe_app_window)
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn background(invocation: &Invocation, mut client: HostApiClient) -> Result<String, String> {
    match invocation
        .background
        .clone()
        .unwrap_or(BackgroundAction::List)
    {
        BackgroundAction::List => {
            let (apps, media) = client.background_apps().map_err(|e| e.to_string())?;
            Ok(if invocation.json {
                json!({
                    "apps": apps.iter().map(background_app_json).collect::<Vec<_>>(),
                    "media": media.iter().map(media_json).collect::<Vec<_>>(),
                })
                .to_string()
            } else {
                describe_background(&apps, &media)
            })
        }
        BackgroundAction::Open(package) => {
            client
                .act_on_background(&package, BackgroundCommand::Open)
                .map_err(|e| e.to_string())?;
            Ok(if invocation.json {
                json!({ "opened": package }).to_string()
            } else {
                format!("Opening {package}")
            })
        }
        BackgroundAction::Stop(package) => {
            client
                .act_on_background(&package, BackgroundCommand::Stop)
                .map_err(|e| e.to_string())?;
            Ok(if invocation.json {
                json!({ "stopped": package }).to_string()
            } else {
                format!("Stopped {package}")
            })
        }
    }
}

/// An app that runs in the background as one JSON object.
pub fn background_app_json(app: &BackgroundApp) -> Value {
    json!({
        "package": app.package_name,
        "label": app.app_label,
        "title": app.title,
        "text": app.text,
        "actions": app.actions,
        "icon_png": base64(&app.icon_png),
    })
}

/// A media session as one JSON object.
pub fn media_json(session: &MediaSession) -> Value {
    json!({
        "package": session.package_name,
        "label": session.app_label,
        "title": session.title,
        "artist": session.artist,
        "album": session.album,
        "playback": match session.playback {
            Playback::Playing => "playing",
            Playback::Paused => "paused",
            Playback::Stopped => "stopped",
        },
    })
}

/// The apps that run in the background, one per line, and the media sessions.
pub fn describe_background(apps: &[BackgroundApp], media: &[MediaSession]) -> String {
    if apps.is_empty() && media.is_empty() {
        return "No app runs in the background.".into();
    }
    let mut lines = Vec::new();
    for app in apps {
        let name = if app.app_label.is_empty() {
            &app.package_name
        } else {
            &app.app_label
        };
        let mut line = format!("{name} ({})", app.package_name);
        if !app.title.is_empty() {
            let _ = write!(line, ": {}", app.title);
        }
        if !app.text.is_empty() {
            let _ = write!(line, " - {}", app.text);
        }
        lines.push(line);
    }
    for session in media {
        let state = match session.playback {
            Playback::Playing => "playing",
            Playback::Paused => "paused",
            Playback::Stopped => "stopped",
        };
        lines.push(format!(
            "Media of {}: {} {} ({state})",
            session.package_name, session.title, session.artist
        ));
    }
    lines.join("\n")
}

/// `status` as one JSON object.
pub fn status_json(status: &Status) -> Value {
    json!({
        "state": status.state.to_string(),
        "last_reason": status.last_reason,
        "update_notice": status.update_notice,
        "problem": status.problem,
    })
}

/// The settings as one JSON object.
pub fn settings_json(report: &SettingsReport) -> Value {
    json!({
        "render_gpu": report.settings.render_gpu,
        "freeze": report.settings.freeze,
        "links_in_android": report.settings.links_in_android,
        "clipboard": report.settings.clipboard,
        "program_updates": report.settings.program_updates,
        "image_updates": report.settings.image_updates,
        "input_trace": report.settings.input_trace,
        "resize_trace": report.settings.resize_trace,
        "memory_limit_mb": report.settings.memory_limit_mb,
        "developer_mode": report.settings.developer_mode,
        "adb_port": report.settings.adb_port,
        "adb_address": adb_address(&report.settings),
        "arm_translation": report.settings.arm_translation,
        "stop_without_asking": report.settings.stop_without_asking,
        "file_sharing": report.settings.file_sharing,
        "shared_folders": report.settings.shared_folders,
        "advanced_networking": report.settings.advanced_networking,
        "network": network_json(report.network.as_ref()),
        "run_mode": report.settings.run_mode.as_str(),
        "stop_after_secs": report.settings.stop_after_secs,
        "keep_for_notifications": report.settings.keep_for_notifications,
        "program_updates_available": report.facts.program_updates_available,
        "image_updates_available": report.facts.image_updates_available,
        "version": report.facts.version,
        "log_dir": report.facts.log_dir,
        "android_version": report.facts.android_version,
    })
}

/// The network as one JSON object (`mode`, `addresses`, `dns_servers`, `gateway`, `problem`),
/// or null from a service that does not report it.
pub fn network_json(network: Option<&NetworkStatus>) -> Value {
    match network {
        Some(network) => json!({
            "mode": network.mode,
            "addresses": network.addresses,
            "dns_servers": network.dns_servers,
            "gateway": network.gateway,
            "problem": network.problem,
        }),
        None => Value::Null,
    }
}

/// The network, one fact per line, with the advanced networking setting.
pub fn describe_network(report: &SettingsReport) -> String {
    let setting = format!(
        "Advanced networking: {}",
        on_off(report.settings.advanced_networking)
    );
    let Some(network) = &report.network else {
        return format!("{setting}\nThis service does not report the network; update it.");
    };
    let mode = match network.mode.as_str() {
        NetworkStatus::NAT => "NAT (behind this computer)",
        NetworkStatus::ADVANCED => "advanced (on this computer's network)",
        NetworkStatus::OFF => "none (Android is not running)",
        other => other,
    };
    let list = |items: &[String]| {
        if items.is_empty() {
            "none".to_string()
        } else {
            items.join(", ")
        }
    };
    let mut lines = vec![setting, format!("Network: {mode}")];
    if network.mode != NetworkStatus::OFF {
        lines.push(format!("Addresses: {}", list(&network.addresses)));
        lines.push(format!("DNS servers: {}", list(&network.dns_servers)));
        if !network.gateway.is_empty() {
            lines.push(format!("Gateway: {}", network.gateway));
        }
    }
    if let Some(problem) = &network.problem {
        lines.push(format!("Problem: {problem}"));
    }
    if report.settings.advanced_networking != (network.mode == NetworkStatus::ADVANCED)
        && network.mode != NetworkStatus::OFF
        && network.problem.is_none()
    {
        lines.push("The change applies the next time Android starts.".into());
    }
    lines.join("\n")
}

/// The address `adb connect` takes in developer mode.
pub fn adb_address(settings: &dsa_host_core::settings::Settings) -> String {
    format!("127.0.0.1:{}", settings.adb_port)
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

/// A wait in words: `5 minutes`, `90 seconds`, `1 hour`.
fn describe_wait(secs: u32) -> String {
    let plural = |n: u32, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    if secs % 3600 == 0 {
        plural(secs / 3600, "hour")
    } else if secs % 60 == 0 {
        plural(secs / 60, "minute")
    } else {
        plural(secs, "second")
    }
}

/// What a data reset erases, as the command line and the settings app say it.
pub const RESET_DATA_WARNING: &str = "Resetting erases every app you installed in Android, \
     the accounts you signed in to and the data of every app. Android stops, and its next \
     start is as it was first installed. The Android image and DSA's settings stay. This \
     cannot be undone.";

/// `dsa reset-data`.
fn reset_data(invocation: &Invocation, client: &mut HostApiClient) -> Result<String, String> {
    if !invocation.yes {
        return Err(format!(
            "{RESET_DATA_WARNING}\nRun dsa reset-data --yes to reset."
        ));
    }
    client.reset_data().map_err(|e| e.to_string())?;
    Ok(if invocation.json {
        json!({ "reset": true }).to_string()
    } else {
        "Android's data was reset; the next start boots Android as it was first installed"
            .to_string()
    })
}

/// What moving to an older Android major erases, as the command line and the settings app
/// say it.
pub const OLDER_ANDROID_WARNING: &str = "Android cannot read data a newer version wrote, so \
     moving to an older Android version erases the Android data of every user of this \
     computer: every app they installed, the accounts they signed in to and the data of every \
     app. Each user's Android starts as it was first installed. DSA's settings stay. This \
     cannot be undone.";

/// The version report as JSON, for `dsa android version --json` and the settings app.
pub fn android_versions_json(report: &VersionReport) -> Value {
    json!({
        "version": report.version,
        "android_major": report.android_major,
        "build": report.build,
        "build_id": report.build_id,
        "security_patch": report.security_patch,
        "channel": match report.channel {
            Channel::Latest => "latest".to_string(),
            Channel::Major(major) => major.to_string(),
        },
        "supported_majors": report.supported_majors,
        "target_major": report.target_major,
        "on_trial": report.on_trial,
        "notice": report.notice,
    })
}

/// The version report in words.
pub fn android_versions_text(report: &VersionReport) -> String {
    let mut lines = vec![format!(
        "Android {} ({}), build {}, security patch {}",
        report.android_major,
        report.version,
        report.build_id.as_deref().unwrap_or("unknown"),
        report.security_patch.as_deref().unwrap_or("unknown"),
    )];
    lines.push(match report.channel {
        Channel::Latest => format!(
            "Follows: the newest supported version (Android {})",
            report.target_major
        ),
        Channel::Major(major) => format!("Follows: Android {major}"),
    });
    let supported: Vec<String> = report
        .supported_majors
        .iter()
        .map(|m| m.to_string())
        .collect();
    lines.push(format!("Supported: Android {}", supported.join(", ")));
    if report.target_major != report.android_major {
        lines.push(format!(
            "Moving to Android {}: it is used once its image is downloaded and Android starts \
             again",
            report.target_major
        ));
    } else if report.on_trial {
        lines.push("The image is new and is kept once Android has started from it".into());
    }
    if let Some(notice) = &report.notice {
        lines.push(format!("Update: {notice}"));
    }
    lines.join("\n")
}

/// `dsa android version` and `dsa android use`.
fn android_version(invocation: &Invocation, client: &mut HostApiClient) -> Result<String, String> {
    let report = match (invocation.command, invocation.android) {
        (Command::AndroidUse, Some(channel)) => {
            match client.use_android_version(channel, invocation.erase_data) {
                Err(why) if why.data_would_be_lost() => {
                    return Err(format!(
                        "{OLDER_ANDROID_WARNING}\nRun dsa android use {} --erase-data to move.",
                        channel.to_wire()
                    ))
                }
                other => other.map_err(|e| e.to_string())?,
            }
        }
        _ => client.android_versions().map_err(|e| e.to_string())?,
    };
    Ok(if invocation.json {
        android_versions_json(&report).to_string()
    } else {
        android_versions_text(&report)
    })
}

/// Where DSA's own updates come from on Linux, where it has no updater of its own.
const LINUX_PROGRAM_UPDATES: &str =
    "Program updates: from the system's package manager (software center, apt, dnf or zypper)";

/// The settings, one per line.
pub fn describe_settings(report: &SettingsReport) -> String {
    let settings = &report.settings;
    let facts = &report.facts;
    let available = |value: bool, available: bool| {
        if available {
            on_off(value).to_string()
        } else {
            format!("{} (this installation does not update it)", on_off(value))
        }
    };
    let mut lines = vec![
        format!("Render GPU: {}", settings.render_gpu),
        format!("Freeze when idle: {}", on_off(settings.freeze)),
        format!(
            "Web links open in: {}",
            if settings.links_in_android {
                "Android apps"
            } else {
                "the default browser"
            }
        ),
        format!("Clipboard sharing: {}", on_off(settings.clipboard)),
        if cfg!(target_os = "linux") {
            // The .deb and the .rpm add DSA's package repository, so DSA comes with the
            // system's other updates and the program has no updater of its own there.
            LINUX_PROGRAM_UPDATES.to_string()
        } else {
            format!(
                "Program updates: {}",
                available(settings.program_updates, facts.program_updates_available)
            )
        },
        format!(
            "Image updates: {}",
            available(settings.image_updates, facts.image_updates_available)
        ),
        format!("Input trace: {}", on_off(settings.input_trace)),
        format!("Resize trace: {}", on_off(settings.resize_trace)),
        format!(
            "Memory limit: {}",
            match settings.memory_limit_mb {
                0 => "automatic".to_string(),
                mb => format!("{mb} MiB"),
            }
        ),
        if settings.developer_mode {
            format!("Developer mode: on (adb connect {})", adb_address(settings))
        } else {
            format!("Developer mode: off (adb port {})", settings.adb_port)
        },
        format!(
            "ARM translation: {} (dsa arm-translation status says whether a package is installed)",
            on_off(settings.arm_translation)
        ),
        format!(
            "Stop background apps without asking: {}",
            on_off(settings.stop_without_asking)
        ),
        format!(
            "File sharing: {}{}",
            on_off(settings.file_sharing),
            if settings.shared_folders.is_empty() {
                " (no folder chosen)".to_string()
            } else {
                format!(" ({})", settings.shared_folders.join(", "))
            }
        ),
        format!(
            "Advanced networking: {} (dsa network shows the addresses)",
            on_off(settings.advanced_networking)
        ),
        match settings.run_mode {
            dsa_host_core::run_mode::RunMode::AsNeeded => format!(
                "Run mode: as needed (stops {} after the last app closes)",
                describe_wait(settings.stop_after_secs)
            ),
            dsa_host_core::run_mode::RunMode::Continuous => {
                "Run mode: continuous (stays running in Doze)".to_string()
            }
        },
        format!(
            "Apps with notifications keep Android running: {}",
            on_off(settings.keep_for_notifications)
        ),
        format!("Service version: {}", facts.version),
    ];
    if let Some(version) = &facts.android_version {
        lines.push(format!("Android image: {version}"));
    }
    if let Some(dir) = &facts.log_dir {
        lines.push(format!("Service log folder: {dir}"));
    }
    lines.join("\n")
}

/// The memory report as one JSON object. Sizes in KiB; `guest` is null until Android has
/// reported, and `age_seconds` says how old its report is.
pub fn memory_json(memory: &MemorySnapshot) -> Value {
    let guest = memory.guest.as_ref().map(|report| {
        let guest = &report.memory;
        json!({
            "mem_total_kb": guest.mem_total_kb,
            "mem_free_kb": guest.mem_free_kb,
            "mem_available_kb": guest.mem_available_kb,
            "buffers_kb": guest.buffers_kb,
            "cached_kb": guest.cached_kb,
            "anon_pages_kb": guest.anon_pages_kb,
            "shmem_kb": guest.shmem_kb,
            "slab_reclaimable_kb": guest.slab_reclaimable_kb,
            "swap_total_kb": guest.swap_total_kb,
            "swap_free_kb": guest.swap_free_kb,
            "zram_original_kb": guest.zram_original_kb,
            "zram_used_kb": guest.zram_used_kb,
            "total_pss_kb": guest.total_pss_kb,
            "kernel": {
                "slab_unreclaimable_kb": guest.kernel.slab_unreclaimable_kb,
                "kernel_stack_kb": guest.kernel.kernel_stack_kb,
                "page_tables_kb": guest.kernel.page_tables_kb,
                "vmalloc_used_kb": guest.kernel.vmalloc_used_kb,
                "percpu_kb": guest.kernel.percpu_kb,
                "cma_total_kb": guest.kernel.cma_total_kb,
                "hugetlb_kb": guest.kernel.hugetlb_kb,
                "mem_online_kb": guest.kernel.mem_online_kb,
                "unaccounted_kb": guest.unaccounted_kb(),
                "reserved_at_boot_kb": guest.reserved_at_boot_kb(),
            },
            "age_seconds": report.age.as_secs(),
            "top_processes": guest.top_processes.iter().map(|p| json!({
                "pid": p.pid,
                "name": p.name,
                "pss_kb": p.pss_kb,
                "swap_pss_kb": p.swap_pss_kb,
            })).collect::<Vec<_>>(),
        })
    });
    json!({
        "guest": guest,
        "host": memory.host.iter().map(|p| json!({
            "role": p.role.name(),
            "pid": p.pid,
            "working_set_kb": p.working_set_kb,
            "private_kb": p.private_kb,
        })).collect::<Vec<_>>(),
        "host_total_kb": memory.host_total_kb(),
        "vm_beyond_guest_kb": memory.vm_beyond_guest_kb(),
    })
}

/// This machine's GPUs, and which one drives the primary monitor.
pub fn gpus_json() -> Value {
    #[cfg(windows)]
    {
        use dsa_host_windows::compositor::display_gpu::{adapter_names, display_sources};
        let adapters = adapter_names();
        let primary = display_sources().ok().and_then(|sources| {
            sources
                .into_iter()
                .find(|source| source.x == 0 && source.y == 0)
                .map(|source| source.adapter)
        });
        json!({
            "gpus": adapters.iter().map(|adapter| json!({
                "name": adapter.name,
                "luid": adapter.luid.to_string(),
                "primary_display": Some(adapter.luid) == primary,
            })).collect::<Vec<_>>(),
        })
    }
    #[cfg(not(windows))]
    {
        json!({ "gpus": [] })
    }
}

fn describe_gpus(gpus: &Value) -> String {
    let list = gpus["gpus"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return "no GPU found".into();
    }
    list.iter()
        .map(|gpu| {
            let mut line = gpu["name"].as_str().unwrap_or_default().to_string();
            if gpu["primary_display"].as_bool() == Some(true) {
                line.push_str(" (drives the primary monitor)");
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where the host's file manager shows the WebDAV `url` of Android's shared storage: on
/// Windows, the UNC path of the WebDAV redirector (`\\127.0.0.1@<port>\<token>`); on Linux,
/// the `dav:` location the desktop's file managers (GVfs, KIO) open.
pub fn files_path(url: &str) -> String {
    #[cfg(windows)]
    {
        dsa_host_windows::android_files::unc_path(url).unwrap_or_else(|| url.to_string())
    }
    #[cfg(target_os = "linux")]
    {
        dav_location(url)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        url.to_string()
    }
}

/// Mounts the WebDAV `url` of Android's shared storage in Finder, as Finder's "Connect to
/// Server" does.
#[cfg(target_os = "macos")]
fn open_in_finder(url: &str) -> Result<(), String> {
    let script = format!("mount volume \"{}\"", url.replace(['\\', '"'], ""));
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("running osascript: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Finder could not open {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The `dav:` location of the WebDAV `url`: `http://...` becomes `dav://...`.
pub fn dav_location(url: &str) -> String {
    match url.strip_prefix("http://") {
        Some(rest) => format!("dav://{rest}"),
        None => url.to_string(),
    }
}

/// `dsa files`, and `dsa files open`.
fn android_files(invocation: &Invocation, client: &mut HostApiClient) -> Result<String, String> {
    let url = client.android_files().map_err(|e| e.to_string())?;
    let path = files_path(&url);
    if invocation.command == Command::FilesOpen {
        // Explorer given a path that does not answer opens the user's Documents instead.
        #[cfg(windows)]
        dsa_host_windows::android_files::reachable(&path, std::time::Duration::from_secs(90))?;
        #[cfg(windows)]
        dsa_host_windows::android_files::open_in_explorer(&path)?;
        #[cfg(target_os = "macos")]
        open_in_finder(&url)?;
        // The desktop's own file manager, through its URL opener.
        #[cfg(target_os = "linux")]
        std::process::Command::new("xdg-open")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("open {path} in a file manager; xdg-open could not: {e}"))?;
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        return Err(format!(
            "open {url} in a file manager that speaks WebDAV; this host opens none itself"
        ));
    }
    Ok(if invocation.json {
        json!({ "url": url, "path": path }).to_string()
    } else if invocation.command == Command::FilesOpen {
        format!("Opened Android's files ({path})")
    } else {
        format!("Android's files: {path}")
    })
}

/// `dsa settings`, and `dsa settings set`.
fn settings(invocation: &Invocation, client: &mut HostApiClient) -> Result<String, String> {
    if let Some(change @ (Change::ShowDevice | Change::Device(..) | Change::ResetDevice(_))) =
        &invocation.change
    {
        return device::run(change, invocation.json, client);
    }
    let mut report = client.settings().map_err(|e| e.to_string())?;
    if let Some(change) = &invocation.change {
        let mut wanted = report.settings.clone();
        match change {
            Change::RenderGpu(value) => wanted.render_gpu = value.clone(),
            Change::Freeze(on) => wanted.freeze = *on,
            Change::LinksInAndroid(on) => wanted.links_in_android = *on,
            Change::Clipboard(on) => wanted.clipboard = *on,
            Change::ProgramUpdates(on) => wanted.program_updates = *on,
            Change::ImageUpdates(on) => wanted.image_updates = *on,
            Change::InputTrace(on) => wanted.input_trace = *on,
            Change::ResizeTrace(on) => wanted.resize_trace = *on,
            Change::MemoryLimit(mb) => wanted.memory_limit_mb = *mb,
            Change::DeveloperMode(on) => wanted.developer_mode = *on,
            Change::AdbPort(port) => wanted.adb_port = *port,
            Change::ArmTranslation(on) => wanted.arm_translation = *on,
            Change::ShowDevice | Change::Device(..) | Change::ResetDevice(_) => {
                unreachable!("answered above")
            }
            Change::StopWithoutAsking(on) => wanted.stop_without_asking = *on,
            Change::FileSharing(on) => wanted.file_sharing = *on,
            Change::SharedFolders(folders) => wanted.shared_folders = folders.clone(),
            Change::AdvancedNetworking(on) => wanted.advanced_networking = *on,
            Change::RunMode(mode) => wanted.run_mode = *mode,
            Change::StopAfter(secs) => wanted.stop_after_secs = *secs,
            Change::KeepForNotifications(on) => wanted.keep_for_notifications = *on,
        }
        report = client.set_settings(&wanted).map_err(|e| e.to_string())?;
    }
    Ok(if invocation.json {
        settings_json(&report).to_string()
    } else {
        describe_settings(&report)
    })
}

/// One line for each task of `list`, or a line saying there are none.
pub fn describe_tasks(list: &TaskList) -> String {
    if list.tasks.is_empty() {
        return "Android has no app tasks".into();
    }
    let lines: Vec<String> = list
        .tasks
        .iter()
        .map(|task| {
            let mut line = format!(
                "Task {}: {} ({}), {}x{}",
                task.task_id,
                if task.label.is_empty() {
                    &task.package_name
                } else {
                    &task.label
                },
                task.package_name,
                task.bounds.width(),
                task.bounds.height()
            );
            if let Some((_, display_id)) = list.frames.iter().find(|(id, _)| *id == task.task_id) {
                let _ = write!(line, ", display {display_id}");
            }
            if task.focused {
                line.push_str(", focused");
            }
            line
        })
        .collect();
    lines.join("\n")
}

/// A line describing `status`, and one more for each piece of news about an update.
///
/// A reason's first line goes in the status line. Any further lines (the end of the guest's
/// console, after a failed start) follow it as they are, so they can be copied whole.
pub fn describe(status: &Status) -> String {
    let mut line = format!("Android: {}", status.state);
    if let Some(reason) = &status.last_reason {
        let mut lines = reason.lines();
        let _ = write!(
            line,
            " (last stopped: {})",
            lines.next().unwrap_or_default()
        );
        for detail in lines {
            let _ = write!(line, "\n{detail}");
        }
    }
    for problem in status.problem.iter().flat_map(|problem| problem.lines()) {
        let _ = write!(line, "\nProblem: {problem}");
    }
    for notice in status
        .update_notice
        .iter()
        .flat_map(|notice| notice.lines())
    {
        let _ = write!(line, "\nUpdate: {notice}");
    }
    line
}

fn build() -> String {
    // The installer build sets DSA_VERSION to the release's version.
    let version = option_env!("DSA_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
    format!("dsa {version}")
}

fn connect(
    connector: &mut dyn Connector,
    starter: Option<&dyn ServiceStarter>,
    can_start: bool,
    timeout: Duration,
) -> Result<HostApiClient, String> {
    match HostApiClient::connect(connector, &build(), timeout) {
        Ok(client) => Ok(client),
        Err(ClientError::Connect(why)) if can_start && starter.is_some() => {
            let starter = starter.unwrap();
            match starter.start_service() {
                Ok(()) => {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while Instant::now() < deadline {
                        match HostApiClient::connect(connector, &build(), Duration::from_secs(1)) {
                            Ok(client) => return Ok(client),
                            Err(ClientError::Connect(_)) => {
                                thread::sleep(Duration::from_millis(100));
                            }
                            Err(other) => return Err(other.to_string()),
                        }
                    }
                    Err(format!(
                        "cannot reach the DSA service ({why}): the service started, but its transport did not become ready"
                    ))
                }
                Err(start_err) => Err(format!(
                    "cannot reach the DSA service ({why}): starting the service failed: {start_err}"
                )),
            }
        }
        Err(ClientError::Connect(error)) => Err(format!(
            "cannot reach the DSA service ({error}); check that it is installed and running"
        )),
        Err(other) => Err(other.to_string()),
    }
}

/// `dsa update`: asks the service to install the newest version now and, with `--wait`,
/// waits for the service to go away and come back.
fn update(
    invocation: &Invocation,
    mut client: HostApiClient,
    connector: &mut dyn Connector,
) -> Result<String, String> {
    let update = client
        .update_program(UPDATE_ANSWER_TIMEOUT)
        .map_err(|e| e.to_string())?;
    drop(client);
    if !invocation.wait || update.installing.is_none() {
        return Ok(update.detail);
    }
    let deadline = Instant::now() + invocation.timeout;
    let mut went_away = false;
    let mut last = None;
    loop {
        match connect(connector, None, false, Duration::from_secs(2))
            .and_then(|mut client| client.status().map_err(|e| e.to_string()))
        {
            Ok(status) if went_away => {
                return Ok(format!("{}\n{}", update.detail, describe(&status)))
            }
            Ok(status) => last = Some(status),
            Err(_) => went_away = true,
        }
        if Instant::now() >= deadline {
            // A setup that failed before it stopped the service leaves it running, and the
            // service says so in its status.
            let now = last
                .as_ref()
                .map(|status| format!("\n{}", describe(status)))
                .unwrap_or_default();
            return Err(format!(
                "{}\nthe service has not restarted after {} seconds{now}",
                update.detail,
                invocation.timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// How an install ended, as one JSON object.
pub fn install_json(outcome: &InstallOutcome) -> Value {
    json!({
        "installed": outcome.installed(),
        "status": outcome.status.code(),
        "package": outcome.package_name,
        "message": outcome.message,
    })
}

/// The line `dsa install` prints when Android refused the APK. It starts with the reason's
/// stable code, which the settings app reads to say the reason in the user's language.
pub fn describe_refusal(name: &str, outcome: &InstallOutcome) -> String {
    format!(
        "not installed ({}): {name}: {outcome}",
        outcome.status.code()
    )
}

/// `dsa install`: opens the APK, makes sure Android runs (and, on Windows, the window manager
/// that shows the app once it is opened), then streams the APK to the service.
fn install(
    invocation: &Invocation,
    mut client: HostApiClient,
    connector: &mut dyn Connector,
) -> Result<String, String> {
    let path = invocation
        .file
        .as_deref()
        .unwrap_or(std::path::Path::new(""));
    // A file that cannot be installed is refused before Android is started for it.
    let mut apk = ApkFile::open(path)?;
    let step = |line: &str| {
        if invocation.progress {
            use std::io::Write as _;
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    };
    #[cfg(windows)]
    if invocation.endpoint.is_none() {
        ensure_window_manager()?;
    }
    if !client.status().is_ok_and(|status| {
        matches!(
            status.state,
            SessionState::Active | SessionState::Dozing | SessionState::Frozen
        )
    }) {
        step("stage starting");
    }
    client
        .ensure_running(invocation.timeout, Duration::from_millis(250), |_| {})
        .map_err(|e| format!("Android did not start: {e}"))?;
    drop(client);
    let name = apk.name.clone();
    let size = apk.size;
    step("stage sending");
    let mut sending = Progress {
        inner: apk.reader(),
        sent: 0,
        size,
        shown: None,
        step: &step,
    };
    let outcome = connect(connector, None, false, Duration::from_secs(10))?
        .install_apk(&name, size, &mut sending)
        .map_err(|e| format!("{name} was not installed: {e}"))?;
    if !outcome.installed() {
        return Err(describe_refusal(&name, &outcome));
    }
    Ok(if invocation.json {
        install_json(&outcome).to_string()
    } else {
        format!(
            "Installed {} from {name}",
            outcome.package_name.as_deref().unwrap_or("the app")
        )
    })
}

/// `dsa arm-translation install`: checks the package before anything starts, starts Android
/// when it is not running, and sends the package's image.
fn install_arm_translation(
    invocation: &Invocation,
    mut client: HostApiClient,
    connector: &mut dyn Connector,
) -> Result<String, String> {
    let folder = invocation
        .file
        .as_deref()
        .unwrap_or(std::path::Path::new(""));
    let opened = arm_translation::open_package(folder)?;
    let sha256 = arm_translation::file_sha256(&opened.image)?;
    #[cfg(windows)]
    if invocation.endpoint.is_none() {
        ensure_window_manager()?;
    }
    client
        .ensure_running(invocation.timeout, Duration::from_millis(250), |_| {})
        .map_err(|e| format!("Android did not start: {e}"))?;
    drop(client);
    let mut image = std::fs::File::open(&opened.image)
        .map_err(|e| format!("{} cannot be read: {e}", opened.image.display()))?;
    let status = connect(connector, None, false, Duration::from_secs(10))?
        .install_arm_translation(&opened.package, opened.size, &sha256, &mut image)
        .map_err(|e| format!("the ARM translation package was not installed: {e}"))?;
    let done = format!(
        "Installed {} {}",
        opened.package.name, opened.package.version
    );
    Ok(show_arm_translation(invocation, &status, Some(&done)))
}

/// Where ARM translation stands as one JSON object.
pub fn arm_translation_json(status: &ArmTranslationStatus) -> Value {
    json!({
        "package": status.package.as_ref().map(|package| json!({
            "name": package.name,
            "version": package.version,
            "library": package.library,
            "abis": package.abis,
            "license": package.license,
        })),
        "enabled": status.enabled,
        "supported": status.package.is_some() && status.enabled,
        "android": status.guest.as_ref().map(|guest| json!({
            "installed": guest.installed,
            "active": guest.active,
            "abilist": guest.abilist,
        })),
        "restart_needed": status.restart_needed,
    })
}

/// Where ARM translation stands, one fact per line, after `done` when an action was taken.
pub fn describe_arm_translation(status: &ArmTranslationStatus, done: Option<&str>) -> String {
    let mut lines: Vec<String> = done.map(str::to_string).into_iter().collect();
    lines.push(match (&status.package, status.enabled) {
        (None, _) => "ARM apps: not supported (no translation package is installed)".to_string(),
        (Some(package), true) => format!(
            "ARM apps: supported by {} {} ({})",
            package.name,
            package.version,
            if package.license.is_empty() {
                "licence not given"
            } else {
                &package.license
            }
        ),
        (Some(package), false) => format!(
            "ARM apps: not supported ({} {} is installed but turned off; dsa settings set \
             arm-translation on)",
            package.name, package.version
        ),
    });
    match &status.guest {
        Some(guest) => lines.push(format!(
            "Android installs apps for: {}{}",
            if guest.abilist.is_empty() {
                "(not known)"
            } else {
                &guest.abilist
            },
            if guest.active {
                " (translation is on in this boot)"
            } else {
                ""
            }
        )),
        None => lines.push("Android is not running".to_string()),
    }
    if status.restart_needed {
        lines.push("Restart Android to apply this (dsa stop --wait, then dsa start)".to_string());
    }
    lines.join("\n")
}

fn show_arm_translation(
    invocation: &Invocation,
    status: &ArmTranslationStatus,
    done: Option<&str>,
) -> String {
    if invocation.json {
        arm_translation_json(status).to_string()
    } else {
        describe_arm_translation(status, done)
    }
}

/// The APK as it is read to be sent, saying how much of it has gone.
struct Progress<'a, F: Fn(&str)> {
    inner: &'a mut dyn std::io::Read,
    sent: u64,
    size: u64,
    shown: Option<u64>,
    step: &'a F,
}

impl<F: Fn(&str)> std::io::Read for Progress<'_, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.sent += read as u64;
        let percent = (self.sent * 100)
            .checked_div(self.size)
            .unwrap_or(100)
            .min(100);
        if self.shown != Some(percent) {
            self.shown = Some(percent);
            (self.step)(&format!("progress {percent}"));
            if percent == 100 {
                (self.step)("stage installing");
            }
        }
        Ok(read)
    }
}

/// What `dsa apk-info` says of an APK, as one JSON object.
pub fn apk_info_json(
    info: &ApkInfo,
    android: &str,
    installed: Option<&InstalledPackage>,
    kind: Option<InstallKind>,
) -> Value {
    let layer = |layer: &Layer| match layer {
        Layer::Image(png) => json!({ "kind": "image", "png": base64(png) }),
        Layer::Color(argb) => json!({ "kind": "color", "color": format!("#{argb:08X}") }),
        Layer::Unknown => json!({ "kind": "unknown" }),
    };
    let icon = match &info.icon {
        Some(Icon::Image(png)) => json!({ "kind": "image", "png": base64(png) }),
        Some(Icon::Adaptive {
            background,
            foreground,
        }) => json!({
            "kind": "adaptive",
            "background": layer(background),
            "foreground": layer(foreground),
        }),
        None => Value::Null,
    };
    json!({
        "package": info.package,
        "label": info.label,
        "version_code": info.version_code,
        "version_name": info.version_name,
        "min_sdk": info.min_sdk,
        "min_android": dsa_host_core::apk_info::permissions::android_version(info.min_sdk),
        "target_sdk": info.target_sdk,
        "target_android": dsa_host_core::apk_info::permissions::android_version(info.target_sdk),
        "size": info.size,
        "permissions": info.permissions.iter().map(|p| json!({
            "name": p.name,
            "description": p.description,
            "sensitive": p.sensitive,
        })).collect::<Vec<_>>(),
        "icon": icon,
        "signer": info.signer.as_ref().map(|signer| json!({
            "scheme": signer.scheme.name(),
            "sha256": signing::fingerprint(&signer.sha256()),
        })),
        "abis": info.abis,
        "android": android,
        "installed": installed.map(|installed| json!({
            "version_code": installed.version_code,
            "version_name": installed.version_name,
            "signers": installed
                .signer_sha256
                .iter()
                .map(|digest| signing::fingerprint(digest))
                .collect::<Vec<_>>(),
        })),
        "install": kind.map(InstallKind::code),
    })
}

/// What `dsa apk-info` prints of an APK for a person.
pub fn describe_apk(
    info: &ApkInfo,
    installed: Option<&InstalledPackage>,
    kind: Option<InstallKind>,
) -> String {
    let android = |sdk: u32| {
        dsa_host_core::apk_info::permissions::android_version(sdk)
            .map(|version| format!("Android {version} (API {sdk})"))
            .unwrap_or_else(|| format!("API {sdk}"))
    };
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} ({})",
        info.label.as_deref().unwrap_or(&info.package),
        info.package
    );
    let _ = writeln!(
        out,
        "version {} ({})",
        info.version_name.as_deref().unwrap_or("-"),
        info.version_code
    );
    let _ = writeln!(
        out,
        "needs {}, made for {}",
        android(info.min_sdk),
        android(info.target_sdk)
    );
    let _ = writeln!(out, "{} bytes", info.size);
    match &info.signer {
        Some(signer) => {
            let _ = writeln!(
                out,
                "signed ({}) by SHA-256 {}",
                signer.scheme.name(),
                signing::fingerprint(&signer.sha256())
            );
        }
        None => {
            let _ = writeln!(out, "not signed; Android will not install it");
        }
    }
    if info.permissions.is_empty() {
        let _ = writeln!(out, "asks for no permissions");
    } else {
        let _ = writeln!(out, "asks for:");
        for permission in &info.permissions {
            let _ = writeln!(out, "  {} ({})", permission.description, permission.name);
        }
    }
    let _ = write!(
        out,
        "{}",
        match (kind, installed) {
            (None, _) => "Android is not running, so what it has of the app is not known".to_string(),
            (Some(InstallKind::New), _) => "not installed yet".to_string(),
            (Some(kind), Some(installed)) => format!(
                "installed: version {} ({}); {}",
                installed.version_name,
                installed.version_code,
                match kind {
                    InstallKind::Update => "this APK updates it",
                    InstallKind::Reinstall => "this APK installs the same version again",
                    InstallKind::Downgrade =>
                        "this APK is an older version, which Android does not install over it",
                    InstallKind::SignatureMismatch =>
                        "this APK is signed with another key, which Android does not install over it",
                    InstallKind::New => "",
                }
            ),
            (Some(_), None) => String::new(),
        }
    );
    out
}

/// `dsa apk-info`: reads the APK, and when Android runs, asks what it has of the package.
/// Never starts Android or the service.
fn apk_info(invocation: &Invocation, connector: &mut dyn Connector) -> Result<String, String> {
    let path = invocation
        .file
        .as_deref()
        .unwrap_or(std::path::Path::new(""));
    let info = ApkInfo::read(path)?;
    let mut android = "unreachable";
    let mut installed = None;
    let mut kind = None;
    if let Ok(mut client) = connect(connector, None, false, Duration::from_secs(5)) {
        android = "stopped";
        if client
            .status()
            .is_ok_and(|status| matches!(status.state, SessionState::Active | SessionState::Dozing))
        {
            if let Ok(state) = client.package_state(&info.package) {
                android = "running";
                kind = Some(InstallKind::decide(
                    info.version_code,
                    &info.signers(),
                    state.as_ref(),
                ));
                installed = state;
            }
        }
    }
    Ok(if invocation.json {
        apk_info_json(&info, android, installed.as_ref(), kind).to_string()
    } else {
        describe_apk(&info, installed.as_ref(), kind)
    })
}

/// How long `dsa open` and `dsa share` wait for Android to start when it is not running:
/// as long as `dsa start --wait`.
const OPEN_START_TIMEOUT: Duration = START_TIMEOUT;

/// `dsa open` and `dsa share`: reads the file, makes sure Android runs (and, on Windows, the
/// window manager that shows the app Android opens it in), then sends the file.
fn open_file(
    invocation: &Invocation,
    mut client: HostApiClient,
    connector: &mut dyn Connector,
) -> Result<String, String> {
    let path = invocation
        .file
        .as_deref()
        .unwrap_or(std::path::Path::new(""));
    let action = if invocation.command == Command::Share {
        FileAction::Send
    } else {
        FileAction::View
    };
    // A file that cannot be sent is refused before Android is started for it.
    let file = FileTransfer::read(path, action)?;
    #[cfg(windows)]
    if invocation.endpoint.is_none() {
        ensure_window_manager()?;
    }
    client
        .ensure_running(OPEN_START_TIMEOUT, Duration::from_millis(250), |_| {})
        .map_err(|e| format!("Android did not start: {e}"))?;
    drop(client);
    connect(connector, None, false, Duration::from_secs(10))?
        .open_file(&file)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Android asks which app to {} {} with",
        if action == FileAction::Send {
            "share"
        } else {
            "open"
        },
        file.name
    ))
}

/// The task of `package` that `dsa tap` presses in: its focused task, else its newest.
pub fn tap_task<'a>(list: &'a TaskList, package: &str) -> Option<&'a Task> {
    let mut tasks = list
        .tasks
        .iter()
        .filter(|task| task.package_name == package);
    let newest = tasks.clone().max_by_key(|task| task.task_id);
    tasks.find(|task| task.focused).or(newest)
}

/// How long `dsa tap` holds the button down: about as long as a quick click.
const TAP_HOLD: Duration = Duration::from_millis(60);

/// Presses and releases at the invocation's point in a task of its package, through the local
/// API. The window is told first, so the input trace has the press.
fn tap(invocation: &Invocation, mut client: HostApiClient) -> Result<String, String> {
    use dsa_host_core::input::{buttons, PointerAction, PointerEvent, PointerTool};
    let package = invocation.package.as_deref().unwrap_or_default();
    let (x, y) = invocation.point.unwrap_or_default();
    let list = client.tasks().map_err(|e| e.to_string())?;
    let Some(task) = tap_task(&list, package) else {
        return Err(format!(
            "{package} has no task in Android; dsa launch {package} opens one"
        ));
    };
    let (width, height) = (task.bounds.width(), task.bounds.height());
    if width > 0 && height > 0 && (x >= width || y >= height) {
        return Err(format!(
            "({x}, {y}) is outside task {} of {package}, which is {width}x{height} pixels",
            task.task_id
        ));
    }
    let task_id = task.task_id;
    let traced = trace_press(task_id, (x, y));
    let mut stream = client.stream_input().map_err(|e| e.to_string())?;
    let at = (x as f32, y as f32);
    let mut down = PointerEvent::new(task_id, PointerAction::Down, PointerTool::Mouse, at);
    down.buttons = buttons::PRIMARY;
    down.pressure = 1.0;
    stream.send(&down).map_err(|e| e.to_string())?;
    thread::sleep(TAP_HOLD);
    let up = PointerEvent::new(task_id, PointerAction::Up, PointerTool::Mouse, at);
    stream.send(&up).map_err(|e| e.to_string())?;
    Ok(if invocation.json {
        json!({ "task_id": task_id, "x": x, "y": y, "traced": traced }).to_string()
    } else {
        format!(
            "Tapped task {task_id} of {package} at ({x}, {y}){}",
            if traced { ", traced" } else { "" }
        )
    })
}

/// Tells the task's window that a press is coming, for the input trace. Whether it took it.
#[cfg(windows)]
fn trace_press(task_id: i32, point: (u32, u32)) -> bool {
    dsa_host_windows::compositor::window_marks::trace_press(task_id, point)
}

#[cfg(not(windows))]
fn trace_press(_task_id: i32, _point: (u32, u32)) -> bool {
    false
}

/// When a launched app's window showed its first frame, from the launch request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FirstFrame {
    pub task_id: i32,
    /// From the request to the service's answer.
    pub request: Duration,
    /// From the request to the window presenting its first frame.
    pub presented: Duration,
    /// From the request to the vertical blank that frame was shown at, when known.
    pub shown: Option<Duration>,
}

impl FirstFrame {
    /// The times, each on the host clock, as times from the request at `start`.
    pub fn since(
        task_id: i32,
        start: Duration,
        answered: Duration,
        presented: Duration,
        shown: Option<Duration>,
    ) -> FirstFrame {
        FirstFrame {
            task_id,
            request: answered.saturating_sub(start),
            presented: presented.saturating_sub(start),
            shown: shown.map(|shown| shown.saturating_sub(start)),
        }
    }
}

/// A time in milliseconds, to a tenth.
fn tenths_ms(time: Duration) -> f64 {
    (time.as_secs_f64() * 10_000.0).round() / 10.0
}

/// What `dsa launch --wait-first-frame` prints.
pub fn describe_first_frame(package: &str, frame: &FirstFrame) -> String {
    let shown = match frame.shown {
        Some(shown) => format!("shown after {:.1} ms", tenths_ms(shown)),
        None => "not known to be shown (the window's swapchain gives no frame statistics)".into(),
    };
    format!(
        "{package} (task {}): first frame presented after {:.1} ms, {shown}; the launch \
         request was answered after {:.1} ms",
        frame.task_id,
        tenths_ms(frame.presented),
        tenths_ms(frame.request)
    )
}

/// What `dsa launch --wait-first-frame --json` prints.
pub fn first_frame_json(package: &str, frame: &FirstFrame) -> Value {
    json!({
        "package": package,
        "task_id": frame.task_id,
        "request_ms": tenths_ms(frame.request),
        "first_frame_ms": tenths_ms(frame.presented),
        "first_shown_ms": frame.shown.map(tenths_ms),
    })
}

/// How often `dsa launch --wait-first-frame` asks the service for the tasks.
#[cfg(windows)]
const TASK_POLL: Duration = Duration::from_millis(100);

/// How long `dsa launch --wait-first-frame` waits for a presented frame to be said to be shown.
#[cfg(windows)]
const SHOWN_WAIT: Duration = Duration::from_secs(2);

/// Launches `package` and waits until a window of it opened after this started has presented
/// its first frame and that frame was shown.
#[cfg(windows)]
fn launch_and_wait_first_frame(
    invocation: &Invocation,
    mut client: HostApiClient,
    package: &str,
) -> Result<String, String> {
    use dsa_host_windows::compositor::log::host_clock;
    use dsa_host_windows::compositor::window_marks::{
        read_time, task_windows, FIRST_FRAME_PROPERTY, FIRST_SHOWN_PROPERTY,
    };
    let before: Vec<isize> = task_windows().into_iter().map(|(hwnd, _)| hwnd).collect();
    let start = host_clock();
    client.launch_app(package).map_err(|e| e.to_string())?;
    let answered = host_clock();
    let deadline = Instant::now() + invocation.timeout;
    let mut tasks: Vec<i32> = Vec::new();
    let mut listed: Option<Instant> = None;
    let mut presented_at: Option<Instant> = None;
    loop {
        if listed.is_none_or(|at| at.elapsed() >= TASK_POLL) {
            tasks = client
                .tasks()
                .map_err(|e| e.to_string())?
                .tasks
                .iter()
                .filter(|task| task.package_name == package)
                .map(|task| task.task_id)
                .collect();
            listed = Some(Instant::now());
        }
        for (hwnd, task_id) in task_windows() {
            if before.contains(&hwnd) || !tasks.contains(&task_id) {
                continue;
            }
            let Some(presented) = read_time(hwnd, FIRST_FRAME_PROPERTY) else {
                continue;
            };
            if presented < start {
                continue;
            }
            let shown = read_time(hwnd, FIRST_SHOWN_PROPERTY);
            let waited = presented_at.get_or_insert_with(Instant::now).elapsed();
            if shown.is_some() || waited >= SHOWN_WAIT {
                let frame = FirstFrame::since(task_id, start, answered, presented, shown);
                return Ok(if invocation.json {
                    first_frame_json(package, &frame).to_string()
                } else {
                    describe_first_frame(package, &frame)
                });
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "no new window of {package} presented a frame within {} seconds",
                invocation.timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(not(windows))]
fn launch_and_wait_first_frame(
    _invocation: &Invocation,
    _client: HostApiClient,
    package: &str,
) -> Result<String, String> {
    Err(format!(
        "dsa launch {package} --wait-first-frame needs the Windows host, whose windows say \
         when their first frame came"
    ))
}

/// Starts this session's window manager when it is not running, as opening an app does.
#[cfg(windows)]
fn ensure_window_manager() -> Result<(), String> {
    use dsa_host_windows::app_window::launch::{
        ensure_window_manager, pipe_exists, run_program, window_manager_path, POLL,
        START_WINDOW_MANAGER,
    };
    use dsa_host_windows::compositor::{current_session, hosts::pipe_name};
    let pipe = pipe_name(current_session());
    let program = std::env::current_exe().ok();
    ensure_window_manager(
        || pipe_exists(&pipe),
        || match window_manager_path(program.as_deref()) {
            Some(window_manager) => run_program(&window_manager, ""),
            None => Err("the DSA Window Manager is not installed".into()),
        },
        START_WINDOW_MANAGER,
        POLL,
    )
}

/// Runs `invocation` against the service `connector` reaches. Returns what to print.
pub fn run(invocation: &Invocation, connector: &mut dyn Connector) -> Result<String, String> {
    let starter = default_starter(invocation.endpoint.as_deref());
    run_with_starter(invocation, connector, starter.as_deref())
}

/// Runs `invocation` against the service `connector` reaches, using `starter` to start it
/// if stopped.
pub fn run_with_starter(
    invocation: &Invocation,
    connector: &mut dyn Connector,
    starter: Option<&dyn ServiceStarter>,
) -> Result<String, String> {
    if invocation.command == Command::ApkInfo {
        return apk_info(invocation, connector);
    }
    if invocation.command == Command::Gpus {
        let gpus = gpus_json();
        return Ok(if invocation.json {
            gpus.to_string()
        } else {
            describe_gpus(&gpus)
        });
    }
    // Stopping or freezing needs an Android that runs, so neither starts the service.
    let can_start = !matches!(invocation.command, Command::Stop | Command::Freeze);
    let mut client = connect(connector, starter, can_start, Duration::from_secs(10))?;
    let show = |status: &Status| {
        if invocation.json {
            status_json(status).to_string()
        } else {
            describe(status)
        }
    };
    let status = match invocation.command {
        Command::Status => client.status(),
        Command::Start => client.start(),
        Command::Stop => client.stop(),
        Command::Freeze => client.freeze(),
        Command::Update => return update(invocation, client, connector),
        Command::Launch => {
            let package = invocation.package.as_deref().unwrap_or_default();
            if invocation.wait_first_frame {
                return launch_and_wait_first_frame(invocation, client, package);
            }
            client.launch_app(package).map_err(|e| e.to_string())?;
            return Ok(format!("Launching {package}"));
        }
        Command::Tap => return tap(invocation, client),
        Command::StopApp => {
            let package = invocation.package.as_deref().unwrap_or_default();
            client.stop_app(package).map_err(|e| e.to_string())?;
            return Ok(format!("Stopping {package} and giving its memory back"));
        }
        Command::Install => return install(invocation, client, connector),
        Command::ArmTranslationStatus => {
            let status = client.arm_translation().map_err(|e| e.to_string())?;
            return Ok(show_arm_translation(invocation, &status, None));
        }
        Command::ArmTranslationInstall => {
            return install_arm_translation(invocation, client, connector);
        }
        Command::ArmTranslationRemove => {
            let status = client.remove_arm_translation().map_err(|e| e.to_string())?;
            let done = "Removed the ARM translation package";
            return Ok(show_arm_translation(invocation, &status, Some(done)));
        }
        Command::Widgets => return widgets(invocation, client, connector),
        Command::Apps => return apps(invocation, client, connector),
        Command::Background => return background(invocation, client),
        Command::AppWindow => return app_window(invocation, client),
        Command::Open | Command::Share => {
            return open_file(invocation, client, connector);
        }
        Command::Tasks => {
            let list = client.tasks().map_err(|e| e.to_string())?;
            return Ok(describe_tasks(&list));
        }
        Command::Settings => return settings(invocation, &mut client),
        Command::Files | Command::FilesOpen => return android_files(invocation, &mut client),
        Command::Network => {
            let report = client.settings().map_err(|e| e.to_string())?;
            return Ok(if invocation.json {
                network_json(report.network.as_ref()).to_string()
            } else {
                describe_network(&report)
            });
        }
        Command::ResetData => return reset_data(invocation, &mut client),
        Command::AndroidVersion | Command::AndroidUse => {
            return android_version(invocation, &mut client)
        }
        Command::Memory => {
            let memory = client.memory().map_err(|e| e.to_string())?;
            return Ok(if invocation.json {
                memory_json(&memory).to_string()
            } else {
                memory.to_string().trim_end().to_string()
            });
        }
        Command::Logcat => {
            let filter = invocation.log_filter.clone().unwrap_or_default();
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            return logs::logcat(client, &filter, invocation.dump, invocation.json, &mut out);
        }
        Command::LogsCollect => {
            let path = invocation.output.clone().unwrap_or_default();
            return logs::collect(client, &path);
        }
        Command::Gpus | Command::ApkInfo => unreachable!("answered before connecting"),
    }
    .map_err(|e| e.to_string())?;
    if !invocation.wait || matches!(invocation.command, Command::Status | Command::Freeze) {
        return Ok(show(&status));
    }
    let deadline = Instant::now() + invocation.timeout;
    // The service answers a start once the session has moved on, so a Stopped answer
    // means the start has already failed.
    let (done, failed): (&[SessionState], &[SessionState]) = match invocation.command {
        Command::Start => (
            &[SessionState::Active, SessionState::Dozing],
            &[SessionState::Stopped, SessionState::Stalled],
        ),
        _ => (&[SessionState::Stopped], &[]),
    };
    let goal = if invocation.command == Command::Start {
        "started"
    } else {
        "stopped"
    };
    let mut status = status;
    loop {
        if done.contains(&status.state) {
            return Ok(show(&status));
        }
        if failed.contains(&status.state) {
            return Err(format!("Android did not start: {}", describe(&status)));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Android has not {goal} after {} seconds: {}",
                invocation.timeout.as_secs(),
                describe(&status)
            ));
        }
        thread::sleep(Duration::from_millis(250));
        status = client.status().map_err(|e| e.to_string())?;
    }
}

/// The connector for the platform's usual place, or for `endpoint`.
pub fn connector(endpoint: Option<&str>) -> Result<Box<dyn Connector>, String> {
    #[cfg(windows)]
    {
        let name = endpoint
            .map(str::to_string)
            .or_else(|| std::env::var("DSA_PIPE").ok())
            .unwrap_or_else(|| "dsa-host".into());
        Ok(Box::new(dsa_host_windows::pipe::NamedPipeConnector {
            name,
        }))
    }
    #[cfg(unix)]
    {
        let path = match endpoint
            .map(str::to_string)
            .or_else(|| std::env::var("DSA_SOCKET").ok())
        {
            Some(path) => std::path::PathBuf::from(path),
            None => default_socket()?,
        };
        Ok(Box::new(dsa_host_unix::socket::UnixLocalConnector { path }))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = endpoint;
        Err("this platform has no DSA service".into())
    }
}

#[cfg(unix)]
fn default_socket() -> Result<std::path::PathBuf, String> {
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        Ok(std::path::PathBuf::from(home).join("Library/Application Support/DSA/host.sock"))
    } else {
        Ok("/run/dsa/host.sock".into())
    }
}
