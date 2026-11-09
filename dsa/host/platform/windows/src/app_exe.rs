//! Each app's own window program: a copy of the app window host carrying the app's name and
//! icon.
//!
//! Task Manager and the taskbar group windows by program, and take the name and icon from
//! the program file. So each Android app gets its own copy of `dsa-app-window.exe`, at
//! `%LOCALAPPDATA%\DSA\apps\<package>\<Label>.exe`, named after the app, whose version
//! information names the app and whose icon is the app's launcher icon. Settings then shows
//! as `Settings.exe` and Settings, with its own icon and its own entry, apart from every other
//! app. Each package has its own folder, so two apps of the same name never share a file.
//!
//! A copy is made again when the installed window host changes (DSA was updated) or the app's
//! name or icon does. A note beside it (`made.json`) records what it was made from. A copy
//! under the app's old name is removed once it no longer runs. A copy that is running cannot
//! be replaced; when its name stays the same, it stays as it is until the app's windows close,
//! and is made again then.
//!
//! The guest describes an app in two ways: before its window opens ([`ensure`]), with the
//! icon drawn large for the splash and the title bar's icon beside it, and in the list of
//! installed apps ([`ensure_installed`]), with the launcher icon at 256 pixels and the app's
//! version. The two drawings of one icon never have the same bytes, so a program made from the
//! first is kept while the list describes the same app at the same version; otherwise every
//! window and every list would make the program again in turn, and the copy in use would stay
//! as it was. An icon the guest could not draw never takes the place of one it drew.
//!
//! Explorer keeps the icon of a program by its file and index, so a program made again with
//! another icon under the same name would go on showing the old one in the Start menu and on
//! the taskbar, which take it from the app's shortcut. Explorer is told the file changed and to
//! read icons again. A window's own icon, which Alt+Tab shows, is read from the program when it
//! starts.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use dsa_exe_resources::{
    dib_icon, icon_group, icon_image_side, names, png_icon, version_info, IconImage,
    VersionStrings, CAPTION_ICON_ID, ICON_GROUP_ID, LANG_EN_US, MAX_ICON_IMAGE_SIDE, RT_GROUP_ICON,
    RT_ICON, RT_RCDATA, RT_VERSION, SPLASH_ICON_ID, VERSION_ID,
};
use dsa_host_core::compositor::window_hosts::is_valid_package;
use dsa_host_core::tasks::AppInfo;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::{
    BeginUpdateResourceW, EndUpdateResourceW, FindResourceExW, LoadLibraryExW, UpdateResourceW,
    LOAD_LIBRARY_AS_DATAFILE,
};

/// The installed app window host, beside the window manager.
pub const TEMPLATE_NAME: &str = "dsa-app-window.exe";

/// The most icon images a program is looked through for.
const MAX_ICON_IMAGES: u16 = 64;

/// The note in each app's folder that records what its program was made from.
pub const NOTE_NAME: &str = "made.json";

/// The longest file name an app's program gets, in characters, before `.exe`. Paths in the
/// user's profile stay well under the classic 260-character limit with it.
const MAX_NAME: usize = 64;

/// The file name, without `.exe`, of the program named `label`: the label with what Windows
/// does not allow in a file name taken out. `package` stands in when nothing usable is left.
pub fn program_name(package: &str, label: &str) -> String {
    const DEVICES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    let cleaned: String = label
        .chars()
        .filter(|c| !c.is_control() && !"<>:\"/\\|?*".contains(*c))
        .collect();
    let mut name: String = cleaned.trim().chars().take(MAX_NAME).collect();
    // Windows drops trailing dots and spaces from a file name.
    name.truncate(name.trim_end_matches(['.', ' ']).len());
    let name = name.trim_start().to_string();
    if name.is_empty() {
        return package.to_string();
    }
    // A name whose part before the first dot is a device's (`CON`, `COM1`) opens the device.
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_uppercase();
    let numbered = stem.chars().count() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem
            .chars()
            .nth(3)
            .is_some_and(|c| c.is_ascii_digit() || "\u{b9}\u{b2}\u{b3}".contains(c));
    if DEVICES.contains(&stem.as_str()) || numbered {
        return format!("{name}_");
    }
    name
}

/// Where the program of `package` named `label` goes: its own folder, in `apps_dir`.
pub fn program_path(apps_dir: &Path, package: &str, label: &str) -> PathBuf {
    apps_dir
        .join(package)
        .join(format!("{}.exe", program_name(package, label)))
}

/// Removes what is left of the app's older programs: the ones under another name in its
/// folder, unfinished copies, and the program and note of the earlier layout, where every
/// app's program was `<package>.exe` directly in `apps_dir`. A program that still runs
/// cannot be removed; it is tried again the next time.
fn remove_old(apps_dir: &Path, package: &str, keep: &Path) {
    let _ = fs::remove_file(apps_dir.join(format!("{package}.exe")));
    let _ = fs::remove_file(apps_dir.join(format!("{package}.exe.new")));
    let _ = fs::remove_file(apps_dir.join(format!("{package}.json")));
    let Some(dir) = keep.parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if path != keep && (name.ends_with(".exe") || name.ends_with(".exe.new")) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Where each app's window program goes: `%LOCALAPPDATA%\DSA\apps`.
pub fn default_apps_dir() -> Option<PathBuf> {
    crate::paths::UserRoot::current().map(|root| root.apps())
}

/// The installed app window host: beside this program.
pub fn default_template() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|dir| dir.join(TEMPLATE_NAME))
}

/// What a copy was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Made {
    /// The installed window host's size and modification time.
    template: String,
    label: String,
    /// SHA-256 of the icon, in hexadecimal; empty for none.
    icon: String,
    /// SHA-256 of the title bar's icon (an adaptive icon's foreground), in hexadecimal; empty
    /// for none. A note written before it was recorded reads as none.
    #[serde(default)]
    caption_icon: String,
    /// What the program was made from: [`FROM_WINDOW`] or [`FROM_INSTALLED`]. A note written
    /// before it was recorded reads as neither.
    #[serde(default)]
    source: String,
    /// The installed version (`<version code>-<last update time>`) of the app the program was
    /// last held against, from the list of installed apps; empty while none was.
    #[serde(default)]
    updated: String,
}

/// A program made from the description the guest gives before the app's window opens.
const FROM_WINDOW: &str = "window";

/// A program made from the guest's list of installed apps.
const FROM_INSTALLED: &str = "installed";

/// The installed version of `app` as a note records it; empty when the guest gave none.
fn installed_version(app: &AppInfo) -> String {
    if app.version_code == 0 && app.last_update_time_ms == 0 {
        return String::new();
    }
    format!("{}-{}", app.version_code, app.last_update_time_ms)
}

/// What becomes of a program when it is asked for again.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Plan {
    /// The program stays as it is, and its note says this.
    Keep(Made),
    /// The program is made again, and its note says this.
    Make(Made),
}

/// What becomes of the program made as `current` (`None` when there is none) when it is asked
/// for as `wanted`.
fn plan(current: Option<&Made>, mut wanted: Made) -> Plan {
    let from_window = wanted.source == FROM_WINDOW;
    let Some(current) = current else {
        return Plan::Make(wanted);
    };
    if from_window {
        // The window's description says nothing of the installed version, so the version the
        // program was last held against stays.
        wanted.updated = current.updated.clone();
    }
    if current.template != wanted.template || current.label != wanted.label {
        return Plan::Make(wanted);
    }
    let kept = |updated: String| {
        Plan::Keep(Made {
            updated,
            ..current.clone()
        })
    };
    if current.icon == wanted.icon && current.caption_icon == wanted.caption_icon {
        return kept(wanted.updated);
    }
    // An icon the guest could not draw this time never takes the place of one it drew.
    if wanted.icon.is_empty() && !current.icon.is_empty() {
        return kept(wanted.updated);
    }
    // The list draws the same icon smaller and without the title bar's icon. A program made
    // from the window's description of the app at the version listed, or before any version
    // was, already shows it.
    if !from_window
        && current.source == FROM_WINDOW
        && !current.icon.is_empty()
        && (current.updated.is_empty() || current.updated == wanted.updated)
    {
        return kept(wanted.updated);
    }
    Plan::Make(wanted)
}

fn template_stamp(template: &Path) -> io::Result<String> {
    let meta = fs::metadata(template)?;
    let modified = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Ok(format!("{}-{modified}", meta.len()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The name a copy carries: the app's own, else its package.
fn label_of(package: &str, app: Option<&AppInfo>) -> String {
    app.map(|app| app.label.trim())
        .filter(|label| !label.is_empty())
        .unwrap_or(package)
        .to_string()
}

/// Makes sure `apps_dir` holds the window program of `package`, made from `template` with the
/// app's name and icon (as the guest describes the app before its window opens) and named
/// after the app, and returns its path. An existing copy that is up to date is used as it is;
/// an outdated one of the same name that is running is used too, and the reason is returned
/// with it.
pub fn ensure(
    template: &Path,
    apps_dir: &Path,
    package: &str,
    app: Option<&AppInfo>,
) -> Result<(PathBuf, Option<String>), String> {
    make(template, apps_dir, package, app, FROM_WINDOW)
}

/// [`ensure`] for an app as the guest's list of installed apps describes it, for its Start
/// menu shortcut. A program made from the window's description of the same app at the same
/// version is kept: it shows the same icon, drawn larger, with the title bar's icon beside it.
pub fn ensure_installed(
    template: &Path,
    apps_dir: &Path,
    app: &AppInfo,
) -> Result<(PathBuf, Option<String>), String> {
    make(
        template,
        apps_dir,
        &app.package_name,
        Some(app),
        FROM_INSTALLED,
    )
}

fn make(
    template: &Path,
    apps_dir: &Path,
    package: &str,
    app: Option<&AppInfo>,
    source: &str,
) -> Result<(PathBuf, Option<String>), String> {
    if !is_valid_package(package) {
        return Err(format!("{package:?} is not an Android package name"));
    }
    let label = label_of(package, app);
    let target = program_path(apps_dir, package, &label);
    let Some(folder) = target.parent().map(Path::to_path_buf) else {
        return Err(format!("{} has no folder", target.display()));
    };
    let note = folder.join(NOTE_NAME);
    let icon = app
        .map(|app| app.icon_png.as_slice())
        .filter(|png| png_icon(png).is_some());
    // The title bar's icon goes in only beside the app's own: it never stands alone.
    let caption_icon = app
        .map(|app| app.caption_icon_png.as_slice())
        .filter(|png| icon.is_some() && crate::compositor::widgets::png::size(png).is_some());
    let made = Made {
        template: template_stamp(template)
            .map_err(|e| format!("the app window host {}: {e}", template.display()))?,
        label,
        icon: icon
            .map(|png| hex(&Sha256::digest(png)))
            .unwrap_or_default(),
        caption_icon: caption_icon
            .map(|png| hex(&Sha256::digest(png)))
            .unwrap_or_default(),
        source: source.to_string(),
        updated: match app {
            Some(app) if source == FROM_INSTALLED => installed_version(app),
            _ => String::new(),
        },
    };
    let existed = target.exists();
    let current = fs::read_to_string(&note)
        .ok()
        .and_then(|text| serde_json::from_str::<Made>(&text).ok());
    let made = match plan(current.as_ref().filter(|_| existed), made) {
        Plan::Keep(kept) => {
            if current.as_ref() != Some(&kept) {
                let _ = fs::write(&note, serde_json::to_string(&kept).unwrap_or_default());
            }
            remove_old(apps_dir, package, &target);
            return Ok((target, None));
        }
        Plan::Make(made) => made,
    };
    fs::create_dir_all(&folder)
        .map_err(|e| format!("could not create {}: {e}", folder.display()))?;
    let mut fresh = target.clone().into_os_string();
    fresh.push(".new");
    let fresh = PathBuf::from(fresh);
    fs::copy(template, &fresh).map_err(|e| {
        format!(
            "could not copy {} to {}: {e}",
            template.display(),
            fresh.display()
        )
    })?;
    if let Err(why) = write_resources(&fresh, package, &made.label, icon, caption_icon) {
        let _ = fs::remove_file(&fresh);
        return Err(why);
    }
    match fs::rename(&fresh, &target) {
        Ok(()) => {
            let text = serde_json::to_string(&made).unwrap_or_default();
            let _ = fs::write(&note, text);
            remove_old(apps_dir, package, &target);
            if existed && current.is_none_or(|current| current.icon != made.icon) {
                tell_explorer_the_icon_changed(&target);
            }
            Ok((target, None))
        }
        Err(e) if target.exists() => {
            let _ = fs::remove_file(&fresh);
            Ok((
                target,
                Some(format!(
                    "{package}'s window program is in use, so it keeps its old name and icon \
                     until its windows close ({e})"
                )),
            ))
        }
        Err(e) => {
            let _ = fs::remove_file(&fresh);
            Err(format!(
                "could not put {package}'s window program in place: {e}"
            ))
        }
    }
}

/// Tells Explorer that the program at `path` was made again with another icon, so the Start
/// menu and the taskbar read it again rather than show the one Explorer keeps for the file's
/// name and index.
fn tell_explorer_the_icon_changed(path: &Path) {
    use windows_sys::Win32::UI::Shell::{
        SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNE_UPDATEITEM, SHCNF_IDLIST, SHCNF_PATHW,
    };
    let path = wide(&path.to_string_lossy());
    // SAFETY: a NUL-terminated path that outlives the call, and an event with no items.
    unsafe {
        SHChangeNotify(
            SHCNE_UPDATEITEM as i32,
            SHCNF_PATHW,
            path.as_ptr().cast(),
            std::ptr::null(),
        );
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// How many icon images the program at `path` has, numbered from 1.
fn icon_images(path: &Path) -> u16 {
    let path_w = wide(&path.to_string_lossy());
    // SAFETY: loaded as data only, never run; freed below.
    let module = unsafe {
        LoadLibraryExW(
            path_w.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_AS_DATAFILE,
        )
    };
    if module.is_null() {
        return 0;
    }
    let mut count = 0;
    while count < MAX_ICON_IMAGES {
        // SAFETY: integer resource IDs, in a module loaded above.
        let found = unsafe {
            FindResourceExW(
                module,
                RT_ICON as usize as *const u16,
                (count + 1) as usize as *const u16,
                0,
            )
        };
        if found.is_null() {
            break;
        }
        count += 1;
    }
    // SAFETY: the module loaded above.
    unsafe { FreeLibrary(module) };
    count
}

/// The program's icon image from the app's icon `png`. A PNG an icon image holds (256 pixels
/// or less) goes in as it is. A larger one, which the guest draws so the splash is never
/// scaled up, is scaled down to 256 pixels and goes in as a bitmap, because Windows reads a
/// PNG icon image only up to 256 pixels; when it cannot be scaled, it goes in as it is.
fn icon_image(png: &[u8]) -> Option<IconImage> {
    let image = png_icon(png)?;
    let (width, height) = crate::compositor::widgets::png::size(png)?;
    if width <= MAX_ICON_IMAGE_SIDE && height <= MAX_ICON_IMAGE_SIDE {
        return Some(image);
    }
    let side = icon_image_side(width.max(height));
    let scaled = crate::compositor::widgets::png::with_com(|| {
        let decoder = crate::compositor::widgets::png::Decoder::new().ok()?;
        decoder.decode_straight(png, side, side).ok()
    });
    scaled
        .and_then(|pixels| dib_icon(&pixels, side))
        .or(Some(image))
}

/// Rewrites the version information and icon of the program at `path`, and puts the title
/// bar's icon (`caption_icon`, an adaptive icon's foreground layer) beside it when the app has
/// one. The taskbar and the Start menu show the program's icon, plate and all; only the
/// window's title bar shows the foreground.
fn write_resources(
    path: &Path,
    package: &str,
    label: &str,
    icon: Option<&[u8]>,
    caption_icon: Option<&[u8]>,
) -> Result<(), String> {
    let version = crate::VERSION.to_string();
    let strings = VersionStrings {
        description: label.to_string(),
        product: names::PRODUCT.to_string(),
        company: names::COMPANY.to_string(),
        internal_name: package.to_string(),
        version,
    };
    let images = icon_images(path);
    let path_w = wide(&path.to_string_lossy());
    // SAFETY: a NUL-terminated path; the handle is ended below on every path.
    let update = unsafe { BeginUpdateResourceW(path_w.as_ptr(), 0) };
    if update.is_null() {
        return Err(format!(
            "could not open {} to name it: {}",
            path.display(),
            io::Error::last_os_error()
        ));
    }
    let put = |kind: u16, id: u16, language: u16, data: Option<&[u8]>| -> bool {
        let (pointer, length) = match data {
            Some(data) => (data.as_ptr() as *const std::ffi::c_void, data.len() as u32),
            None => (std::ptr::null(), 0),
        };
        // SAFETY: the type and name are integer resource IDs; the data outlives the call.
        unsafe {
            UpdateResourceW(
                update,
                kind as usize as *const u16,
                id as usize as *const u16,
                language,
                pointer,
                length,
            ) != 0
        }
    };
    let mut ok = put(
        RT_VERSION,
        VERSION_ID,
        LANG_EN_US,
        Some(&version_info(&strings, LANG_EN_US)),
    );
    // The installed window host has only an English name, which this replaces; the app's
    // own name already comes in the language Android shows.
    if let Some(image) = icon.and_then(icon_image) {
        ok &= put(RT_ICON, 1, 0, Some(&image.data));
        // Only the app's image stays, so no size of the window host's own icon shows.
        for id in 2..=images {
            ok &= put(RT_ICON, id, 0, None);
        }
        ok &= put(
            RT_GROUP_ICON,
            ICON_GROUP_ID,
            0,
            Some(&icon_group(&[image], 1)),
        );
    }
    // The icon as the guest drew it, which the window's splash is drawn from at the size it
    // has on the window's monitor.
    if let Some(png) = icon {
        ok &= put(RT_RCDATA, SPLASH_ICON_ID, 0, Some(png));
    }
    if let Some(png) = caption_icon {
        ok &= put(RT_RCDATA, CAPTION_ICON_ID, 0, Some(png));
    }
    let error = io::Error::last_os_error();
    // SAFETY: ends the update begun above; discards it when something failed.
    let written = unsafe { EndUpdateResourceW(update, if ok { 0 } else { 1 }) } != 0;
    if !ok {
        return Err(format!(
            "could not write {package}'s name and icon into {}: {error}",
            path.display()
        ));
    }
    if !written {
        return Err(format!(
            "could not save {package}'s name and icon into {}: {}",
            path.display(),
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Starts each app's window program: makes the app's copy of the window host when it is
/// missing or outdated, then runs it.
pub struct ProgramLauncher {
    pub template: PathBuf,
    pub apps_dir: PathBuf,
    pub logger: Option<std::sync::Arc<crate::compositor::log::CompositorLogger>>,
    /// The apps whose program ended while it could not take the app's name and icon, with
    /// them and how many more ticks it is tried.
    waiting: std::collections::BTreeMap<String, (AppInfo, u32)>,
}

/// How many ticks of the window manager a program that ended is tried again while its file is
/// still held: its process may end a little after its connection.
const REMAKE_TRIES: u32 = 20;

impl ProgramLauncher {
    pub fn new(
        template: PathBuf,
        apps_dir: PathBuf,
        logger: Option<std::sync::Arc<crate::compositor::log::CompositorLogger>>,
    ) -> ProgramLauncher {
        ProgramLauncher {
            template,
            apps_dir,
            logger,
            waiting: std::collections::BTreeMap::new(),
        }
    }

    /// Makes the program of `app` again if it is outdated. True when nothing is left to do:
    /// it is up to date, or it cannot be made at all.
    fn remake(&self, app: &AppInfo) -> bool {
        match ensure(&self.template, &self.apps_dir, &app.package_name, Some(app)) {
            Ok((_, None)) => true,
            Ok((_, Some(_))) => false,
            Err(why) => {
                crate::compositor::log::note(self.logger.as_deref(), &why);
                true
            }
        }
    }
}

impl crate::compositor::hosts::HostLauncher for ProgramLauncher {
    fn start(&mut self, package: &str, app: Option<&AppInfo>) -> Result<(), String> {
        self.waiting.remove(package);
        let (program, note) = ensure(&self.template, &self.apps_dir, package, app)?;
        if let Some(note) = note {
            crate::compositor::log::note(self.logger.as_deref(), &note);
        }
        spawn(&program, package)
    }

    fn ended(&mut self, package: &str, app: Option<&AppInfo>) {
        let Some(app) = app.filter(|app| app.package_name == package) else {
            return;
        };
        if !self.remake(app) {
            self.waiting
                .insert(package.to_string(), (app.clone(), REMAKE_TRIES));
        }
    }

    fn tick(&mut self) {
        if self.waiting.is_empty() {
            return;
        }
        let waiting = std::mem::take(&mut self.waiting);
        for (package, (app, tries)) in waiting {
            if !self.remake(&app) && tries > 1 {
                self.waiting.insert(package, (app, tries - 1));
            }
        }
    }
}

/// Runs `program` as the window host of `package` the window manager started.
fn spawn(program: &Path, package: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
    };
    let application = wide(&program.to_string_lossy());
    // The package is named too, though the program's folder names it, so the window host
    // never has to tell it from the file's place.
    let mut command_line = wide(&format!(
        "\"{}\" {} {} {package}",
        program.display(),
        crate::app_window::STARTED_BY_WINDOW_MANAGER,
        crate::app_window::PACKAGE_ARGUMENT
    ));
    // SAFETY: plain structures, zero is their documented initial state.
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated strings; the command line buffer is writable, as the call needs.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(format!(
            "could not run {}: {}",
            program.display(),
            io::Error::last_os_error()
        ));
    }
    // The window host connects by itself; nothing waits on the process.
    // SAFETY: handles CreateProcessW returned, closed once.
    unsafe {
        CloseHandle(process.hProcess);
        CloseHandle(process.hThread);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(source: &str, icon: &str, caption_icon: &str, updated: &str) -> Made {
        Made {
            template: "template".into(),
            label: "Google Play Store".into(),
            icon: icon.into(),
            caption_icon: caption_icon.into(),
            source: source.into(),
            updated: updated.into(),
        }
    }

    #[test]
    fn fr7_the_installed_list_keeps_the_program_its_window_made_at_the_same_version() {
        // The window's description draws the icon at 512 pixels with the title bar's icon;
        // the list draws it at 256 without. They are the same icon, so the program stays.
        let window = made(FROM_WINDOW, "large", "caption", "85073430-1");
        let listed = made(FROM_INSTALLED, "small", "", "85073430-1");
        assert_eq!(plan(Some(&window), listed), Plan::Keep(window.clone()));
        // A program the window made before any list was seen takes the list's version.
        let unversioned = made(FROM_WINDOW, "large", "caption", "");
        assert_eq!(
            plan(
                Some(&unversioned),
                made(FROM_INSTALLED, "small", "", "85073430-1")
            ),
            Plan::Keep(made(FROM_WINDOW, "large", "caption", "85073430-1"))
        );
    }

    #[test]
    fn fr7_an_updated_app_gets_its_new_icon_from_the_installed_list() {
        // Play updates itself: a new version is listed, whose icon may be new.
        let window = made(FROM_WINDOW, "large", "caption", "85073430-1");
        let listed = made(FROM_INSTALLED, "new", "", "86000000-2");
        assert_eq!(plan(Some(&window), listed.clone()), Plan::Make(listed));
    }

    #[test]
    fn fr7_opening_a_window_makes_the_program_from_the_windows_icon() {
        // The window's description is the richer one, and keeps the version already held.
        let listed = made(FROM_INSTALLED, "small", "", "85073430-1");
        let window = made(FROM_WINDOW, "large", "caption", "");
        assert_eq!(
            plan(Some(&listed), window),
            Plan::Make(made(FROM_WINDOW, "large", "caption", "85073430-1"))
        );
        // Once made, the list at that version and the next window leave it as it is.
        let current = made(FROM_WINDOW, "large", "caption", "85073430-1");
        assert_eq!(
            plan(
                Some(&current),
                made(FROM_INSTALLED, "small", "", "85073430-1")
            ),
            Plan::Keep(current.clone())
        );
        assert_eq!(
            plan(Some(&current), made(FROM_WINDOW, "large", "caption", "")),
            Plan::Keep(current.clone())
        );
    }

    #[test]
    fn fr7_an_icon_the_guest_could_not_draw_never_replaces_one_it_drew() {
        // A system app read before its icon could be drawn comes with none.
        let current = made(FROM_INSTALLED, "icon", "", "1-1");
        assert_eq!(
            plan(Some(&current), made(FROM_WINDOW, "", "", "")),
            Plan::Keep(current.clone())
        );
        assert_eq!(
            plan(Some(&current), made(FROM_INSTALLED, "", "", "1-1")),
            Plan::Keep(current.clone())
        );
    }

    #[test]
    fn fr7_a_program_made_without_an_icon_takes_it_once_it_is_known() {
        // Started before the guest could draw the app's icon, the program carries none; the
        // next description that has one makes it again, from the list or from a window.
        let bare = made(FROM_WINDOW, "", "", "");
        let listed = made(FROM_INSTALLED, "icon", "", "1-1");
        assert_eq!(plan(Some(&bare), listed.clone()), Plan::Make(listed));
        let window = made(FROM_WINDOW, "large", "caption", "");
        assert_eq!(plan(Some(&bare), window.clone()), Plan::Make(window));
    }

    #[test]
    fn fr7_a_renamed_app_or_an_upgraded_dsa_always_makes_the_program_again() {
        let current = made(FROM_WINDOW, "large", "caption", "1-1");
        let renamed = Made {
            label: "Play Store".into(),
            ..made(FROM_INSTALLED, "small", "", "1-1")
        };
        assert_eq!(plan(Some(&current), renamed.clone()), Plan::Make(renamed));
        let upgraded = Made {
            template: "newer".into(),
            ..made(FROM_WINDOW, "large", "caption", "")
        };
        assert!(matches!(plan(Some(&current), upgraded), Plan::Make(_)));
    }

    #[test]
    fn fr7_a_note_from_before_the_source_was_recorded_is_made_again_once() {
        // An earlier release did not say what made the program; its note reads as neither.
        let old: Made = serde_json::from_str(
            r#"{"template":"template","label":"Google Play Store","icon":"small"}"#,
        )
        .unwrap();
        assert_eq!(old.source, "");
        // The same drawing stays, and records the version.
        assert_eq!(
            plan(Some(&old), made(FROM_INSTALLED, "small", "", "1-1")),
            Plan::Keep(Made {
                updated: "1-1".into(),
                ..old.clone()
            })
        );
        let window = made(FROM_WINDOW, "large", "caption", "");
        assert_eq!(plan(Some(&old), window.clone()), Plan::Make(window));
    }

    #[test]
    fn fr7_the_installed_version_is_the_version_code_and_update_time() {
        let app = AppInfo {
            version_code: 85073430,
            last_update_time_ms: 1_700_000_000_000,
            ..Default::default()
        };
        assert_eq!(installed_version(&app), "85073430-1700000000000");
        assert_eq!(installed_version(&AppInfo::default()), "");
    }
}
