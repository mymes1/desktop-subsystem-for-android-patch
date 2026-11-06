//! A launcher entry (a `.desktop` file) and an icon for each app installed in the guest.
//!
//! The entries go in the user's `applications` folder, where every desktop's app menu and
//! search look, each with the app's own name and icon. Starting one asks the guest to start
//! the app, which opens its window. The entry's ID is also the `app_id` of the app's windows,
//! so the desktop groups the windows under the app's icon in its dock or task bar, and its
//! actions open the app's settings or uninstall it, as the Windows Start menu does.
//!
//! Every entry this host writes carries [`PACKAGE_KEY`], so an entry of an app that is gone
//! is found and removed without touching anything else in the folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use dsa_host_core::compositor::window_hosts::is_valid_package;
use dsa_host_core::tasks::AppInfo;

/// The key each entry names its package with.
pub const PACKAGE_KEY: &str = "X-DSA-Package";

/// The prefix of every entry's ID.
pub const PREFIX: &str = "dsa-";

/// The ID of the app's entry and of its windows (`xdg_toplevel.set_app_id`).
pub fn app_id(package: &str) -> String {
    format!("{PREFIX}{package}")
}

/// The file name of the app's entry.
pub fn file_name(package: &str) -> String {
    format!("{}.desktop", app_id(package))
}

/// A value as a `.desktop` string: the spec's escapes for backslash and line breaks.
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_control() => {}
            c => escaped.push(c),
        }
    }
    escaped
}

/// An argument of `Exec`, quoted when it has to be, as the spec quotes them.
fn exec_argument(argument: &str) -> String {
    let plain = argument
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/-_.+=:,@".contains(c));
    if plain && !argument.is_empty() {
        return argument.to_string();
    }
    let mut quoted = String::from("\"");
    for c in argument.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        if c == '%' {
            quoted.push('%');
        }
        quoted.push(c);
    }
    quoted.push('"');
    // The spec reads the whole value as a string first, so every backslash doubles again.
    quoted.replace('\\', "\\\\")
}

/// The command line that runs `program` with `arguments`, for `Exec`.
fn exec(program: &Path, arguments: &[&str]) -> String {
    let mut line = exec_argument(&program.to_string_lossy());
    for argument in arguments {
        line.push(' ');
        line.push_str(&exec_argument(argument));
    }
    line
}

/// The entry of `app`, started through `program` (this host's compositor), with its icon at
/// `icon` when it has one.
pub fn entry(app: &AppInfo, program: &Path, icon: Option<&Path>) -> String {
    let package = &app.package_name;
    let name = if app.label.trim().is_empty() {
        package.as_str()
    } else {
        app.label.as_str()
    };
    let mut text = String::new();
    text.push_str("[Desktop Entry]\n");
    text.push_str("Type=Application\n");
    text.push_str("Version=1.5\n");
    text.push_str(&format!("Name={}\n", escape(name)));
    text.push_str("Comment=Desktop Subsystem for Android\n");
    if let Some(icon) = icon {
        text.push_str(&format!("Icon={}\n", escape(&icon.to_string_lossy())));
    }
    text.push_str(&format!("Exec={}\n", exec(program, &["--launch", package])));
    text.push_str("Terminal=false\n");
    text.push_str("Categories=Utility;\n");
    text.push_str(&format!("StartupWMClass={}\n", app_id(package)));
    text.push_str("StartupNotify=false\n");
    text.push_str("Actions=settings;uninstall;\n");
    text.push_str(&format!("{PACKAGE_KEY}={package}\n"));
    text.push_str("\n[Desktop Action settings]\n");
    text.push_str("Name=App settings\n");
    text.push_str(&format!(
        "Exec={}\n",
        exec(program, &["--app-settings", package])
    ));
    text.push_str("\n[Desktop Action uninstall]\n");
    text.push_str("Name=Uninstall\n");
    text.push_str(&format!(
        "Exec={}\n",
        exec(program, &["--uninstall", package])
    ));
    text
}

/// The package an entry this host wrote is for; `None` for any other file.
pub fn package_of(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix(PACKAGE_KEY)?.strip_prefix('='))
        .map(str::trim)
}

/// Where the entries and the icons go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folders {
    /// `$XDG_DATA_HOME/applications`.
    pub applications: PathBuf,
    /// The folder the icons are kept in.
    pub icons: PathBuf,
}

impl Folders {
    /// The user's folders: `$XDG_DATA_HOME` (`~/.local/share` when unset).
    pub fn of_user(data_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<Folders> {
        let data = data_home
            .filter(|path| path.has_root())
            .or_else(|| home.map(|home| home.join(".local/share")))?;
        Some(Folders {
            applications: data.join("applications"),
            icons: data.join("dsa/icons"),
        })
    }

    /// The user's folders, from the environment.
    pub fn from_env() -> Option<Folders> {
        Folders::of_user(
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }
}

/// Writes the entries of `apps` and removes those of apps no longer installed. An app with
/// a name no file can take is left out. Returns how many entries were written.
pub fn sync(folders: &Folders, apps: &[AppInfo], program: &Path) -> io::Result<usize> {
    fs::create_dir_all(&folders.applications)?;
    fs::create_dir_all(&folders.icons)?;
    let mut written = 0;
    for app in apps {
        if !is_valid_package(&app.package_name) {
            continue;
        }
        let icon = if app.icon_png.is_empty() {
            None
        } else {
            let path = folders.icons.join(format!("{}.png", app.package_name));
            write_if_changed(&path, &app.icon_png)?;
            Some(path)
        };
        let text = entry(app, program, icon.as_deref());
        write_if_changed(
            &folders.applications.join(file_name(&app.package_name)),
            text.as_bytes(),
        )?;
        written += 1;
    }
    for item in fs::read_dir(&folders.applications)?.flatten() {
        let path = item.path();
        let ours = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(PREFIX) && name.ends_with(".desktop"));
        if !ours {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(package) = package_of(&text) {
            if !apps.iter().any(|app| app.package_name == package) {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(folders.icons.join(format!("{package}.png")));
            }
        }
    }
    Ok(written)
}

/// Writes `content` to `path` unless it already holds it, through a file beside it so a
/// desktop reading the folder never sees half an entry.
fn write_if_changed(path: &Path, content: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|old| old == content) {
        return Ok(());
    }
    let partial = path.with_extension("partial");
    fs::write(&partial, content)?;
    fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(package: &str, label: &str) -> AppInfo {
        AppInfo {
            package_name: package.into(),
            label: label.into(),
            ..AppInfo::default()
        }
    }

    #[test]
    fn fr7_each_app_gets_an_entry_with_its_name_icon_and_window_class() {
        let text = entry(
            &app("com.example.notes", "Notes"),
            Path::new("/usr/lib/dsa/dsa-compositor"),
            Some(Path::new(
                "/home/me/.local/share/dsa/icons/com.example.notes.png",
            )),
        );
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("\nName=Notes\n"));
        assert!(text.contains("\nIcon=/home/me/.local/share/dsa/icons/com.example.notes.png\n"));
        assert!(text.contains("\nExec=/usr/lib/dsa/dsa-compositor --launch com.example.notes\n"));
        assert!(text.contains("\nStartupWMClass=dsa-com.example.notes\n"));
        assert!(text.contains("--app-settings com.example.notes"));
        assert!(text.contains("--uninstall com.example.notes"));
        assert_eq!(package_of(&text), Some("com.example.notes"));
    }

    #[test]
    fn fr7_a_name_or_a_path_with_special_characters_is_escaped() {
        let text = entry(
            &app("com.example.a", "Two\nLines \\ here"),
            Path::new("/opt/My Apps/dsa-compositor"),
            None,
        );
        assert!(text.contains("\nName=Two\\nLines \\\\ here\n"));
        assert!(text.contains("\nExec=\"/opt/My Apps/dsa-compositor\" --launch com.example.a\n"));
        assert!(!text.contains("Icon="));
    }

    #[test]
    fn fr7_an_app_without_a_label_is_named_by_its_package() {
        let text = entry(&app("com.example.b", " "), Path::new("/x"), None);
        assert!(text.contains("\nName=com.example.b\n"));
    }

    #[test]
    fn fr7_the_window_class_matches_the_entry() {
        assert_eq!(app_id("com.example.c"), "dsa-com.example.c");
        assert_eq!(file_name("com.example.c"), "dsa-com.example.c.desktop");
    }

    #[test]
    fn fr7_entries_go_in_the_users_applications_folder() {
        let folders = Folders::of_user(None, Some(PathBuf::from("/home/me"))).unwrap();
        assert_eq!(
            folders.applications,
            PathBuf::from("/home/me/.local/share/applications")
        );
        let folders = Folders::of_user(Some(PathBuf::from("/data")), None).unwrap();
        assert_eq!(folders.icons, PathBuf::from("/data/dsa/icons"));
        assert_eq!(
            Folders::of_user(Some(PathBuf::from("relative")), None),
            None,
            "a relative XDG_DATA_HOME is ignored, as the spec says"
        );
    }
}
