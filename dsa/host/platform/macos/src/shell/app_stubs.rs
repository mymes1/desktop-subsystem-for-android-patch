//! Android apps in Launchpad, Spotlight and the Dock.
//!
//! Launchpad and Spotlight list application bundles, and the Dock shows one icon per running
//! application, so every Android app gets a bundle of its own in `~/Applications/DSA Apps`:
//! its name, its icon (as `.icns`), and as its executable a copy of the window host
//! (`dsa-app-window`). Opened from Launchpad, Spotlight or the Dock, the copy asks DSA to
//! start the app; started by the window manager, it shows the app's windows. Either way the
//! app's windows belong to a process of the app's own bundle, so the Dock shows the app as
//! itself, keeps it on request, and Command-Tab switches to it.
//!
//! The alternatives fall short: one DSA process owning every window would show every
//! Android app under a single Dock icon, and a launcher window of DSA's own would leave the
//! apps out of Launchpad and Spotlight. A bundle per app costs one copy of a small program.
//!
//! A bundle is marked as DSA's by its identifier and by the `DSAPackage` key in its
//! `Info.plist`, so [`sync`] never touches an application it did not make.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::icns;

/// What every stub's bundle identifier starts with.
pub const BUNDLE_PREFIX: &str = "org.thisisthepy.dsa.app.";

/// The folder, under the user's `~/Applications`, the stubs go in.
pub const FOLDER: &str = "DSA Apps";

/// The `Info.plist` key that names the Android package a stub launches.
pub const PACKAGE_KEY: &str = "DSAPackage";

/// The stub's executable, in `Contents/MacOS`: the window host.
pub const EXECUTABLE: &str = "dsa-app-window";

/// One Android app as the Mac shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    pub package: String,
    pub label: String,
    pub version: String,
    /// The app's launcher icon, a PNG; empty for none (the bundle then has the generic icon).
    pub icon_png: Vec<u8>,
}

/// The default folder of the stubs: `~/Applications/DSA Apps`.
pub fn default_folder(home: &Path) -> PathBuf {
    home.join("Applications").join(FOLDER)
}

/// The bundle identifier of `package`'s stub: the package with every character a bundle
/// identifier does not take (letters, digits, `-` and `.`) replaced by `-`.
pub fn bundle_identifier(package: &str) -> String {
    let clean: String = package
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{BUNDLE_PREFIX}{clean}")
}

/// The stub's file name: the app's label, with what a file name cannot hold replaced, and
/// the package after it when two apps share a label.
pub fn bundle_name(label: &str, package: &str, label_taken: bool) -> String {
    let clean: String = label
        .chars()
        .map(|c| match c {
            '/' | ':' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let clean = clean.trim().trim_start_matches('.').trim();
    let base = if clean.is_empty() { package } else { clean };
    if label_taken {
        format!("{base} ({package}).app")
    } else {
        format!("{base}.app")
    }
}

fn xml_escape(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\n'))
        .fold(String::new(), |mut out, c| {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                c => out.push(c),
            }
            out
        })
}

/// The stub's `Info.plist`.
pub fn info_plist(app: &AppEntry, has_icon: bool) -> String {
    let icon = if has_icon {
        "\t<key>CFBundleIconFile</key>\n\t<string>icon</string>\n"
    } else {
        ""
    };
    let version = if app.version.is_empty() {
        "1.0"
    } else {
        app.version.as_str()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>{EXECUTABLE}</string>
{icon}	<key>CFBundleIdentifier</key>
	<string>{identifier}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{label}</string>
	<key>CFBundleDisplayName</key>
	<string>{label}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>13.0</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.utilities</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>{PACKAGE_KEY}</key>
	<string>{package}</string>
</dict>
</plist>
"#,
        identifier = xml_escape(&bundle_identifier(&app.package)),
        label = xml_escape(&app.label),
        version = xml_escape(version),
        package = xml_escape(&app.package),
    )
}

/// The package a bundle at `bundle` launches, if it is one of DSA's stubs.
pub fn stub_package(bundle: &Path) -> Option<String> {
    let plist = fs::read_to_string(bundle.join("Contents").join("Info.plist")).ok()?;
    if !plist.contains(BUNDLE_PREFIX) {
        return None;
    }
    let key = format!("<key>{PACKAGE_KEY}</key>");
    let rest = &plist[plist.find(&key)? + key.len()..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")? + start;
    Some(
        rest[start..end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

/// Writes `app`'s stub as `folder/name`, replacing an older one. Its executable is a copy of
/// `window_host`, the installed window host.
pub fn write_stub(
    folder: &Path,
    name: &str,
    app: &AppEntry,
    window_host: &Path,
) -> io::Result<PathBuf> {
    let bundle = folder.join(name);
    let contents = bundle.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;
    let icon = if app.icon_png.is_empty() {
        None
    } else {
        icns::from_png(&app.icon_png).ok()
    };
    match &icon {
        Some(icon) => fs::write(resources.join("icon.icns"), icon)?,
        None => {
            let _ = fs::remove_file(resources.join("icon.icns"));
        }
    }
    fs::write(contents.join("Info.plist"), info_plist(app, icon.is_some()))?;
    fs::write(contents.join("PkgInfo"), "APPL????")?;
    let executable = macos.join(EXECUTABLE);
    // A copy, not a link: Launch Services takes only a program inside the bundle as the
    // bundle's own.
    let _ = fs::remove_file(&executable);
    fs::copy(window_host, &executable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
    }
    Ok(bundle)
}

/// What [`sync`] changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
}

/// Makes `folder` hold exactly one stub for each of `apps`: writes them all (an app's label
/// or icon may have changed) and removes DSA's stubs of apps no longer installed. Bundles
/// that are not DSA's are left alone.
pub fn sync(folder: &Path, apps: &[AppEntry], window_host: &Path) -> io::Result<SyncReport> {
    fs::create_dir_all(folder)?;
    let mut report = SyncReport::default();
    let mut names = Vec::new();
    for app in apps {
        let lower = app.label.to_lowercase();
        let taken = apps
            .iter()
            .filter(|other| other.label.to_lowercase() == lower)
            .count()
            > 1;
        let name = bundle_name(&app.label, &app.package, taken);
        report.written.push(write_stub(folder, &name, app, window_host)?);
        names.push(name);
    }
    for entry in fs::read_dir(folder)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if names.contains(&name) || stub_package(&path).is_none() {
            continue;
        }
        fs::remove_dir_all(&path)?;
        report.removed.push(path);
    }
    Ok(report)
}

/// Tells Launch Services about the stubs `report` wrote and removed, so Launchpad and
/// Spotlight show them at once rather than at their next scan.
#[cfg(target_os = "macos")]
pub fn register(report: &SyncReport) -> Result<(), String> {
    const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
                              LaunchServices.framework/Support/lsregister";
    let mut failed = Vec::new();
    for (bundle, flag) in report
        .written
        .iter()
        .map(|b| (b, "-f"))
        .chain(report.removed.iter().map(|b| (b, "-u")))
    {
        let status = std::process::Command::new(LSREGISTER)
            .arg(flag)
            .arg(bundle)
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            failed.push(bundle.display().to_string());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Launch Services did not take these app bundles: {}",
            failed.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(package: &str, label: &str) -> AppEntry {
        AppEntry {
            package: package.into(),
            label: label.into(),
            version: "2.1".into(),
            icon_png: Vec::new(),
        }
    }

    #[test]
    fn fr7_each_app_has_its_own_bundle_identifier() {
        assert_eq!(
            bundle_identifier("com.example.notes"),
            "org.thisisthepy.dsa.app.com.example.notes"
        );
        assert_eq!(
            bundle_identifier("com.example_app.x"),
            "org.thisisthepy.dsa.app.com.example-app.x"
        );
    }

    #[test]
    fn fr7_a_bundle_is_named_after_the_app_and_never_escapes_its_folder() {
        assert_eq!(bundle_name("Notes", "com.n", false), "Notes.app");
        assert_eq!(bundle_name("a/b:c", "com.n", false), "a-b-c.app");
        assert_eq!(bundle_name("../x", "com.n", false), "-x.app");
        assert_eq!(bundle_name("  ", "com.n", false), "com.n.app");
        assert_eq!(bundle_name("Notes", "com.n", true), "Notes (com.n).app");
    }

    #[test]
    fn fr7_the_info_plist_names_the_app_and_escapes_its_text() {
        let plist = info_plist(&app("com.example", "Tom & Jerry <3>"), true);
        assert!(plist.contains("<string>Tom &amp; Jerry &lt;3&gt;</string>"));
        assert!(plist.contains("<string>org.thisisthepy.dsa.app.com.example</string>"));
        assert!(plist.contains("<key>CFBundleIconFile</key>"));
        assert!(plist.contains("<key>DSAPackage</key>\n\t<string>com.example</string>"));
        assert!(!info_plist(&app("p", "l"), false).contains("CFBundleIconFile"));
    }

    #[test]
    fn fr23_the_bundles_follow_the_installed_apps_and_leave_others_alone() {
        let root = std::env::temp_dir().join(format!("dsa-app-stubs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let folder = root.join(FOLDER);
        let window_host = root.join("dsa-app-window");
        fs::create_dir_all(&root).unwrap();
        fs::write(&window_host, b"window host").unwrap();
        let foreign = folder.join("Other.app/Contents");
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("Info.plist"), "<plist/>").unwrap();

        let mut notes = app("com.example.notes", "Notes");
        notes.icon_png = crate::shell::icns::tests::png_header(128);
        let report = sync(
            &folder,
            &[notes, app("com.example.mail", "Mail")],
            &window_host,
        )
        .unwrap();
        assert_eq!(report.written.len(), 2);
        let executable = folder.join("Notes.app/Contents/MacOS").join(EXECUTABLE);
        assert_eq!(fs::read(&executable).unwrap(), b"window host");
        assert!(folder
            .join("Notes.app/Contents/Resources/icon.icns")
            .exists());
        assert_eq!(
            stub_package(&folder.join("Mail.app")),
            Some("com.example.mail".into())
        );

        // Mail is uninstalled: its bundle goes, and the bundle DSA did not make stays.
        let report = sync(&folder, &[app("com.example.notes", "Notes")], &window_host).unwrap();
        assert_eq!(report.removed, vec![folder.join("Mail.app")]);
        assert!(!folder.join("Mail.app").exists());
        assert!(folder.join("Other.app").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
