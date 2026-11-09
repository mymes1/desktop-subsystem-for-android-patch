//! Turning a LiteGapps Core build into the system image the guest lays over its system.
//!
//! A build is a flashable zip:
//!
//! ```text
//! module.prop                   litegapps_variant=core, litegapps_arch=x86_64, litegapps_sdk=36
//! files/files.tar.xz            <arch>/<api>/system/{product,system_ext}/... : the apps
//! modules/core/<Name>/<Name>-LiteGapps-Addon-<arch>-<android>.zip
//!                               module.prop (android_arch, android_sdk), system/...: more apps
//! META-INF/, bin/, *.sh         the recovery installer and its tools, which are not needed
//! ```
//!
//! What goes into the image are the files under `product/` and `system_ext/` that are apps
//! (`app`, `priv-app`), permission lists, system configuration, default permissions and
//! preferred apps (`etc/permissions`, `etc/sysconfig`, `etc/default-permissions`,
//! `etc/preferred-apps`) or framework files (`framework`); nothing else in the build is taken.
//! The guest lays each of those folders over its own, so the image holds all of them, empty
//! where the build has nothing ([`overlaid_folders`]). The privileged permission lists stay in
//! the same partition as the apps they name, since Android reads a partition's lists only for
//! that partition's apps. The setup wizards are left out: the host is the shell, and a setup
//! wizard would take over as the home screen. Every path is checked before anything is used:
//! one that leaves the build refuses it whole, as does a build for another architecture or
//! Android version.
//!
//! Each partition's `etc/permissions` also gets the privileged permission allowlist
//! ([`super::privapp`]) of the privileged apps the image puts there, made from what their
//! manifests ask for, so the guest grants them and never stops at a permission LiteGapps'
//! lists left out.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::tar::{Tar, TarSink};
use super::{privapp, xz, Target, VARIANT};
use crate::apk_info::zip::Archive;
use crate::erofs::Stream;

/// The partitions the image lays files over.
pub const PARTITIONS: [&str; 2] = ["product", "system_ext"];
/// The folders of those partitions it takes files from.
pub const FOLDERS: [&str; 4] = ["app", "priv-app", "etc", "framework"];
/// The folders of each partition's `etc` it takes files from: the privileged permission lists,
/// the system configuration, the default permissions and the preferred apps, which the package
/// manager reads from there for that partition's apps.
pub const ETC_FOLDERS: [&str; 4] = [
    "permissions",
    "sysconfig",
    "default-permissions",
    "preferred-apps",
];

/// The folders of each partition the guest lays the image over, one overlay each, never the
/// partition's root: the image holds every one, empty where the build has nothing, so each
/// overlay finds its layer.
pub fn overlaid_folders() -> Vec<String> {
    let mut folders = Vec::new();
    for partition in PARTITIONS {
        for folder in FOLDERS {
            if folder == "etc" {
                for inner in ETC_FOLDERS {
                    folders.push(format!("{partition}/etc/{inner}"));
                }
            } else {
                folders.push(format!("{partition}/{folder}"));
            }
        }
    }
    folders
}
/// The add-ons of the Core build left out: the setup wizards and the restore step they run.
pub const LEFT_OUT: [&str; 3] = ["SetupWizard", "PixelSetupWizard", "GoogleRestore"];
/// The apps a Core build must hold: Google Play services and the Play Store.
pub const REQUIRED: [&str; 2] = ["priv-app/GmsCore/", "priv-app/Phonesky/"];

const MAX_PROP: usize = 64 << 10;
const MAX_ARCHIVE: usize = 1 << 30;
const MAX_ADDON: usize = 256 << 20;
const MAX_FILE: usize = 1 << 30;
/// The most the build's tar archive may hold once decompressed.
const MAX_TAR: u64 = 4 << 30;

/// What making an image found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    /// How many files the image holds.
    pub files: usize,
    /// How many files of the build were left out.
    pub left_out: usize,
    /// How many permissions the allowlists DSA adds grant, over every package.
    pub allowlisted: usize,
    /// The privileged apps whose manifest could not be read, so the allowlists say nothing for
    /// them.
    pub unread: Vec<String>,
}

/// The largest permission list read for the allowlists.
const MAX_LIST: usize = 4 << 20;

/// Whether the image file `path` is a privileged app's APK, whose manifest the allowlist reads.
fn privileged_apk(path: &str) -> bool {
    PARTITIONS
        .iter()
        .any(|partition| path.starts_with(&format!("{partition}/priv-app/")))
        && path.to_ascii_lowercase().ends_with(".apk")
}

/// Whether the image file `path` is a list Android reads `privapp-permissions` from: the XML
/// files of a partition's `etc/permissions` and `etc/sysconfig`.
fn permission_list(path: &str) -> bool {
    PARTITIONS.iter().any(|partition| {
        ["permissions", "sysconfig"]
            .iter()
            .any(|folder| path.starts_with(&format!("{partition}/etc/{folder}/")))
    }) && path.to_ascii_lowercase().ends_with(".xml")
}

/// What the image's privileged apps ask for and what the build's lists decide, by image path:
/// a later file at the same path replaces an earlier one in the image, and here too.
#[derive(Default)]
struct Permissions {
    /// A privileged app's APK: its package and the permissions it asks for.
    apps: BTreeMap<String, (String, Vec<String>)>,
    /// A permission list's text.
    lists: BTreeMap<String, String>,
    unread: Vec<String>,
}

impl Permissions {
    /// Takes note of the image file `path` holding `data`, when the allowlist needs it.
    fn note(&mut self, path: &str, data: &[u8]) {
        if privileged_apk(path) {
            self.read_apk(path, Cursor::new(data));
        } else if permission_list(path) && data.len() <= MAX_LIST {
            self.lists
                .insert(path.to_string(), String::from_utf8_lossy(data).into_owned());
        }
    }

    fn read_apk<R: Read + Seek>(&mut self, path: &str, reader: R) {
        match crate::apk_info::requested_permissions(reader) {
            Ok(read) => {
                self.unread.retain(|unread| unread != path);
                self.apps.insert(path.to_string(), read);
            }
            Err(why) => {
                eprintln!("gapps: {path}: {why}; the allowlist says nothing for it");
                self.apps.remove(path);
                if !self.unread.iter().any(|unread| unread == path) {
                    self.unread.push(path.to_string());
                }
            }
        }
    }

    /// Adds each partition's allowlist to the image, returning how many permissions they grant.
    fn write<W: Write + Seek>(&self, builder: &mut Stream<W>) -> Result<usize, String> {
        let mut granted = 0;
        for partition in PARTITIONS {
            let mut requested: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for (path, (package, permissions)) in &self.apps {
                if path.starts_with(&format!("{partition}/")) {
                    requested
                        .entry(package.clone())
                        .or_default()
                        .extend(permissions.iter().cloned());
                }
            }
            let own = format!("{partition}/etc/permissions/{}", privapp::FILE_NAME);
            let mut shipped = BTreeMap::new();
            for (path, text) in &self.lists {
                if path.starts_with(&format!("{partition}/")) && *path != own {
                    privapp::read_list(text, &mut shipped);
                }
            }
            let grants = privapp::missing(&requested, &shipped);
            granted += grants.values().map(BTreeSet::len).sum::<usize>();
            builder.add_file(&own, privapp::write_list(&grants).as_bytes())?;
        }
        Ok(granted)
    }
}

/// The `key=value` lines of a `module.prop`.
fn props(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

fn prop<'a>(props: &'a [(String, String)], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, value)| value.as_str())
}

/// `path` as a path inside the build: separated by `/`, with no part that leaves the build. A
/// leading `./` is dropped, and a directory's trailing `/` with it.
pub fn inside(path: &str) -> Result<String, String> {
    let trimmed = path.strip_prefix("./").unwrap_or(path);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let leaves = trimmed.starts_with('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
        || trimmed
            .split('/')
            .next()
            .is_some_and(|first| first.contains(':'))
        || trimmed.split('/').any(|part| part == ".." || part == ".");
    if leaves || trimmed.is_empty() {
        return Err(format!(
            "the build holds {path:?}, a path outside the build, so it is not used"
        ));
    }
    Ok(trimmed.to_string())
}

/// Where a file of the build's system (`rest`, the path after `system/`) goes in the image, or
/// `None` when it is left out.
pub fn image_path(rest: &str) -> Option<String> {
    let mut parts = rest.split('/');
    let partition = parts.next()?;
    let folder = parts.next()?;
    let name = parts.next()?;
    if !PARTITIONS.contains(&partition) || !FOLDERS.contains(&folder) || name.is_empty() {
        return None;
    }
    // In etc, only the folders the guest lays over: a file elsewhere there would never be seen.
    let deeper = parts.next().is_some_and(|part| !part.is_empty());
    if folder == "etc" && (!ETC_FOLDERS.contains(&name) || !deeper) {
        return None;
    }
    crate::erofs::split_path(rest).ok()?;
    Some(rest.to_string())
}

/// Files of the build's tar archive, into the image.
struct IntoImage<'a, W: std::io::Write + Seek> {
    builder: &'a mut Stream<W>,
    /// `<arch>/<api>/system/`, which the files for the guest are under.
    prefix: String,
    open: bool,
    kept: Vec<String>,
    left_out: usize,
    permissions: &'a mut Permissions,
    /// Where a privileged app's APK is copied as it streams past, so its manifest can be read
    /// (an APK's directory is at its end); removed once read.
    scan_path: PathBuf,
    /// The file being copied there: the APK's image path, and the copy.
    scan: Option<(String, BufWriter<File>)>,
    /// A permission list being collected: its image path and its bytes so far.
    list: Option<(String, Vec<u8>)>,
}

impl<W: std::io::Write + Seek> TarSink for IntoImage<'_, W> {
    fn begin(&mut self, path: &str, size: u64) -> Result<(), String> {
        let path = inside(path)?;
        let target = path.strip_prefix(&self.prefix).and_then(image_path);
        match target {
            Some(target) => {
                self.builder.begin_file(&target, size)?;
                if privileged_apk(&target) {
                    let copy = File::create(&self.scan_path).map_err(|e| {
                        format!("{} cannot be written: {e}", self.scan_path.display())
                    })?;
                    self.scan = Some((target.clone(), BufWriter::new(copy)));
                } else if permission_list(&target) && size <= MAX_LIST as u64 {
                    self.list = Some((target.clone(), Vec::new()));
                }
                self.kept.push(target);
                self.open = true;
            }
            None => {
                self.left_out += 1;
                self.open = false;
            }
        }
        Ok(())
    }

    fn data(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.open {
            self.builder.write_data(bytes)?;
            if let Some((_, copy)) = &mut self.scan {
                copy.write_all(bytes)
                    .map_err(|e| format!("{} cannot be written: {e}", self.scan_path.display()))?;
            }
            if let Some((_, list)) = &mut self.list {
                list.extend_from_slice(bytes);
            }
        }
        Ok(())
    }

    fn end(&mut self) -> Result<(), String> {
        if self.open {
            self.open = false;
            self.builder.end_file()?;
        }
        if let Some((target, copy)) = self.scan.take() {
            let scan_path = self.scan_path.clone();
            // Flushed and closed before it is read.
            drop(copy.into_inner().map_err(|e| {
                format!("{} cannot be written: {}", scan_path.display(), e.error())
            })?);
            let copy = File::open(&scan_path)
                .map_err(|e| format!("{} cannot be read: {e}", scan_path.display()))?;
            self.permissions.read_apk(&target, BufReader::new(copy));
            let _ = std::fs::remove_file(&scan_path);
        }
        if let Some((target, list)) = self.list.take() {
            self.permissions.note(&target, &list);
        }
        Ok(())
    }
}

/// Checks the build's `module.prop`: a Core build for `target`.
fn check_build<R: Read + Seek>(zip: &mut Archive<R>, target: &Target) -> Result<(), String> {
    let entry = zip
        .entry("module.prop")
        .cloned()
        .ok_or("the file is not a LiteGapps build: it has no module.prop")?;
    let text = zip.read(&entry, MAX_PROP)?;
    let props = props(&String::from_utf8_lossy(&text));
    let arch = prop(&props, "litegapps_arch")
        .ok_or("the file is not a LiteGapps build: its module.prop names no architecture")?;
    let sdk = prop(&props, "litegapps_sdk")
        .ok_or("the file is not a LiteGapps build: its module.prop names no Android version")?;
    if arch != target.arch {
        return Err(format!(
            "the build is for {arch} Android, but Android here runs on {}",
            target.arch
        ));
    }
    if sdk != target.sdk.to_string() {
        return Err(format!(
            "the build is for Android API level {sdk}, but Android here has API level {}",
            target.sdk
        ));
    }
    match prop(&props, "litegapps_variant") {
        Some(VARIANT) | None => Ok(()),
        Some(other) => Err(format!(
            "the build is LiteGapps' {other} build; DSA installs its {VARIANT} build"
        )),
    }
}

/// The name of an add-on (`modules/core/<Name>/<file>.zip`), if `path` is one.
fn addon_name(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(&format!("modules/{VARIANT}/"))?;
    let (name, file) = rest.split_once('/')?;
    (file.ends_with(".zip") && !file.contains('/')).then_some(name)
}

/// Adds one add-on's system files to the image.
fn add_addon<W: std::io::Write + Seek>(
    data: Vec<u8>,
    name: &str,
    target: &Target,
    builder: &mut Stream<W>,
    built: &mut (Vec<String>, usize),
    permissions: &mut Permissions,
) -> Result<(), String> {
    let mut addon = Archive::open(Cursor::new(data))
        .map_err(|why| format!("the build's add-on {name} is damaged: {why}"))?;
    for entry in &addon.entries {
        inside(&entry.name)?;
    }
    let entry = addon
        .entry("module.prop")
        .cloned()
        .ok_or_else(|| format!("the build's add-on {name} has no module.prop"))?;
    let props = props(&String::from_utf8_lossy(&addon.read(&entry, MAX_PROP)?));
    if prop(&props, "android_arch") != Some(target.arch)
        || prop(&props, "android_sdk") != Some(target.sdk.to_string().as_str())
    {
        return Err(format!(
            "the build's add-on {name} is for another architecture or Android version than \
             {} Android at API level {}",
            target.arch, target.sdk
        ));
    }
    let entries = addon.entries.clone();
    for entry in entries {
        let path = inside(&entry.name)?;
        if entry.name.ends_with('/') {
            continue;
        }
        let Some(target_path) = path.strip_prefix("system/").and_then(image_path) else {
            built.1 += 1;
            continue;
        };
        let data = addon.read(&entry, MAX_FILE)?;
        builder.add_file(&target_path, &data)?;
        permissions.note(&target_path, &data);
        built.0.push(target_path);
    }
    Ok(())
}

/// The content of the large entry `entry` of the archive in the file `zip`, inflated as it is
/// read, so the compressed bytes are never held beside it.
fn read_large(
    archive: &mut Archive<File>,
    entry: &crate::apk_info::zip::Entry,
    zip: &Path,
) -> Result<Vec<u8>, String> {
    let unreadable = |e: std::io::Error| format!("{} cannot be read: {e}", zip.display());
    if entry.size() > MAX_ARCHIVE as u64 {
        return Err(format!(
            "the build's {} is larger than any LiteGapps build holds",
            entry.name
        ));
    }
    let (method, offset, length) = archive.data_span(entry)?;
    let mut file = File::open(zip).map_err(unreadable)?;
    file.seek(SeekFrom::Start(offset)).map_err(unreadable)?;
    let mut input = file.take(length);
    let mut data = Vec::with_capacity(entry.size() as usize);
    match method {
        0 => {
            input.read_to_end(&mut data).map_err(unreadable)?;
        }
        8 => {
            crate::arm_translation::google::inflate::inflate(input, &mut |piece: &[u8]| {
                if data.len() + piece.len() > MAX_ARCHIVE {
                    return Err("the build's files.tar.xz is larger than it says".to_string());
                }
                data.extend_from_slice(piece);
                Ok(true)
            })
            .map_err(|why| format!("the build's {} is damaged: {why}", entry.name))?;
        }
        other => {
            return Err(format!(
                "the build's {} is compressed in a way zip archives seldom use (method {other})",
                entry.name
            ))
        }
    }
    Ok(data)
}

/// Makes the image at `image` from the LiteGapps build `zip`, which must be a Core build for
/// `target`. On any failure the image is removed.
pub fn build_image(zip: &Path, target: &Target, image: &Path) -> Result<Built, String> {
    let result = write_image(zip, target, image);
    if result.is_err() {
        let _ = std::fs::remove_file(image);
    }
    result
}

fn write_image(zip: &Path, target: &Target, image: &Path) -> Result<Built, String> {
    let file = File::open(zip).map_err(|e| format!("{} cannot be read: {e}", zip.display()))?;
    let mut archive = Archive::open(file)
        .map_err(|why| format!("the downloaded build is not a zip archive: {why}"))?;
    for entry in &archive.entries {
        inside(&entry.name)?;
    }
    check_build(&mut archive, target)?;
    let out =
        File::create(image).map_err(|e| format!("{} cannot be written: {e}", image.display()))?;
    let mut builder = Stream::new(BufWriter::new(out))?;
    for folder in overlaid_folders() {
        builder.add_dir(&folder)?;
    }
    let tar_entry = archive
        .entry("files/files.tar.xz")
        .or_else(|| archive.entry("files.tar.xz"))
        .cloned()
        .ok_or("the build has no files/files.tar.xz, where LiteGapps keeps its apps")?;
    let packed = read_large(&mut archive, &tar_entry, zip)?;
    let mut permissions = Permissions::default();
    let scan_path = image.with_extension("scan.apk");
    let mut sink = IntoImage {
        builder: &mut builder,
        prefix: format!("{}/{}/system/", target.arch, target.sdk),
        open: false,
        kept: Vec::new(),
        left_out: 0,
        permissions: &mut permissions,
        scan_path: scan_path.clone(),
        scan: None,
        list: None,
    };
    let streamed = {
        let mut tar = Tar::new(&mut sink);
        xz::decode(&packed, &mut tar, MAX_TAR)
            .map_err(|why| format!("the build's files.tar.xz cannot be read: {why}"))
            .and_then(|_| {
                tar.finish()
                    .map_err(|why| format!("the build's files.tar.xz cannot be read: {why}"))
            })
    };
    // A copy left open by a build that broke off is closed before it is removed.
    sink.scan = None;
    let _ = std::fs::remove_file(&scan_path);
    streamed?;
    drop(packed);
    let mut built = (std::mem::take(&mut sink.kept), sink.left_out);
    let addons: Vec<_> = archive
        .entries
        .iter()
        .filter_map(|entry| addon_name(&entry.name).map(|name| (name.to_string(), entry.clone())))
        .collect();
    for (name, entry) in addons {
        if LEFT_OUT.contains(&name.as_str()) {
            built.1 += 1;
            continue;
        }
        let data = archive.read(&entry, MAX_ADDON)?;
        add_addon(
            data,
            &name,
            target,
            &mut builder,
            &mut built,
            &mut permissions,
        )?;
    }
    for required in REQUIRED {
        let found = built.0.iter().any(|path| {
            PARTITIONS
                .iter()
                .any(|partition| path.starts_with(&format!("{partition}/{required}")))
        });
        if !found {
            return Err(format!(
                "the build holds no {} for {} Android at API level {}, so it is not a Core build \
                 for this Android",
                required
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(required),
                target.arch,
                target.sdk
            ));
        }
    }
    let allowlisted = permissions.write(&mut builder)?;
    let mut out = builder.finish()?;
    out.flush()
        .map_err(|e| format!("{} cannot be written: {e}", image.display()))?;
    let mut kept = built.0;
    for partition in PARTITIONS {
        kept.push(format!(
            "{partition}/etc/permissions/{}",
            privapp::FILE_NAME
        ));
    }
    kept.sort();
    kept.dedup();
    Ok(Built {
        files: kept.len(),
        left_out: built.1,
        allowlisted,
        unread: permissions.unread,
    })
}

#[cfg(any(test, feature = "conformance"))]
pub mod fixture {
    //! LiteGapps builds for the tests: the layout of a real one, with small files.

    use crate::apk_info::fixture::{archive, document, Attr, El};
    use crate::gapps::tar::fixture as tar;

    /// What the sample's Google Play services asks for: one permission LiteGapps' list in the
    /// sample grants, and one privileged permission it leaves out, as LiteGapps' list for
    /// Android 17 left out `ACCESS_COMPANION_INFO`.
    pub const GMS_PERMISSIONS: [&str; 2] = [
        "android.permission.BACKUP",
        "android.permission.ACCESS_COMPANION_INFO",
    ];
    /// What the sample's services framework (in `system_ext`) asks for, which no list grants.
    pub const GSF_PERMISSIONS: [&str; 1] = ["android.permission.WRITE_SECURE_SETTINGS"];

    /// An APK, unsigned, whose manifest names `package` and asks for `permissions`.
    pub fn apk(package: &str, permissions: &[&str]) -> Vec<u8> {
        let mut elements = vec![El(
            "manifest",
            vec![Attr::Str("package", 0, package)],
            false,
        )];
        for permission in permissions {
            elements.push(El(
                "uses-permission",
                vec![Attr::Str("name", crate::apk_info::attr::NAME, permission)],
                true,
            ));
        }
        let manifest = document(&elements);
        archive(&[("AndroidManifest.xml", &manifest[..])], b"")
    }

    /// An xz file holding `data` in uncompressed LZMA2 chunks, as `xz` writes data it cannot
    /// compress.
    pub fn xz(data: &[u8]) -> Vec<u8> {
        let mut out = b"\xFD7zXZ\0\x00\x00\0\0\0\0".to_vec();
        out.extend_from_slice(&[2, 0, 0x21, 1, 22, 0, 0, 0, 0, 0, 0, 0]);
        for (index, chunk) in data.chunks(65_536).enumerate() {
            out.push(if index == 0 { 1 } else { 2 });
            out.extend_from_slice(&((chunk.len() - 1) as u16).to_be_bytes());
            out.extend_from_slice(chunk);
        }
        out.push(0);
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out.push(0);
        out
    }

    /// An add-on zip of `files` (paths from the add-on's root) for `arch` and `sdk`.
    pub fn addon(arch: &str, sdk: u32, files: &[(&str, &[u8])]) -> Vec<u8> {
        let prop = format!("id=Addon\nandroid_sdk={sdk}\nandroid_arch={arch}\n");
        let mut entries: Vec<(&str, &[u8])> = vec![("module.prop", prop.as_bytes())];
        entries.extend_from_slice(files);
        archive(&entries, b"")
    }

    /// A build whose module.prop says `variant`, `arch` and `sdk`, whose tar holds
    /// `tar_files`, and which carries the add-ons `addons` (name, zip).
    pub fn build_of(
        variant: &str,
        arch: &str,
        sdk: u32,
        tar_files: &[(&str, &[u8])],
        addons: &[(&str, Vec<u8>)],
    ) -> Vec<u8> {
        let prop = format!(
            "id=litegapps\nlitegapps_variant={variant}\nlitegapps_arch={arch}\n\
             litegapps_sdk={sdk}\n"
        );
        let packed = xz(&tar::archive(tar_files));
        let names: Vec<String> = addons
            .iter()
            .map(|(name, _)| format!("modules/core/{name}/{name}-LiteGapps-Addon-{arch}.zip"))
            .collect();
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("module.prop", prop.as_bytes()),
            (
                "META-INF/com/google/android/update-binary",
                &b"#!/sbin/sh\n"[..],
            ),
            ("customize.sh", &b"# installs on a phone\n"[..]),
            ("files/files.tar.xz", packed.as_slice()),
        ];
        for (name, (_, data)) in names.iter().zip(addons) {
            entries.push((name, data));
        }
        archive(&entries, b"")
    }

    /// A Core build for `arch` and `sdk`, as [`build_of`].
    pub fn build(
        arch: &str,
        sdk: u32,
        tar_files: &[(&str, &[u8])],
        addons: &[(&str, Vec<u8>)],
    ) -> Vec<u8> {
        build_of("core", arch, sdk, tar_files, addons)
    }

    /// The files a Core build's tar holds for `arch` and `sdk`, small.
    pub fn core_files(arch: &str, sdk: u32) -> Vec<(String, Vec<u8>)> {
        let system = format!("{arch}/{sdk}/system");
        let gms = apk("com.google.android.gms", &GMS_PERMISSIONS);
        let gsf = apk("com.google.android.gsf", &GSF_PERMISSIONS);
        let google_list = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<permissions>\n    \
             <privapp-permissions package=\"com.google.android.gms\">\n        \
             <permission name=\"{}\"/>\n    </privapp-permissions>\n</permissions>\n",
            GMS_PERMISSIONS[0]
        );
        [
            ("product/priv-app/GmsCore/GmsCore.apk", gms),
            (
                "product/priv-app/Phonesky/Phonesky.apk",
                b"vending".to_vec(),
            ),
            (
                "product/app/GoogleCalendarSyncAdapter/GoogleCalendarSyncAdapter.apk",
                b"calendar".to_vec(),
            ),
            (
                "product/etc/permissions/privapp-permissions-google.xml",
                google_list.into_bytes(),
            ),
            (
                "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk",
                gsf,
            ),
            (
                "system_ext/etc/permissions/privapp-permissions-google-se.xml",
                b"<permissions/>".to_vec(),
            ),
            (
                "product/framework/com.google.android.dialer.support.jar",
                b"jar".to_vec(),
            ),
            ("vendor/etc/left-out.xml", b"x".to_vec()),
            ("product/lib64/left-out.so", b"x".to_vec()),
        ]
        .into_iter()
        .map(|(path, data)| (format!("{system}/{path}"), data))
        .collect()
    }

    /// A Core build for `arch` and `sdk` with the apps a real one has, small, and two
    /// add-ons: one that is taken and a setup wizard that is not.
    pub fn sample_for(arch: &str, sdk: u32) -> Vec<u8> {
        let files = core_files(arch, sdk);
        let dir = format!("{arch}/");
        let mut tar_files: Vec<(&str, &[u8])> = Vec::new();
        tar_files.push((&dir, b""));
        for (path, data) in &files {
            tar_files.push((path, data));
        }
        let common = addon(
            arch,
            sdk,
            &[("system/product/etc/sysconfig/google.xml", b"<config/>")],
        );
        let wizard = addon(
            arch,
            sdk,
            &[(
                "system/product/priv-app/SetupWizardPrebuilt/SetupWizardPrebuilt.apk",
                b"wizard",
            )],
        );
        build(
            arch,
            sdk,
            &tar_files,
            &[("Common", common), ("SetupWizard", wizard)],
        )
    }

    /// [`sample_for`] x86_64 Android 16.
    pub fn sample() -> Vec<u8> {
        sample_for("x86_64", 36)
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{addon, build, build_of, sample};
    use super::*;
    use crate::apk_info::fixture::archive;
    use crate::erofs::read;

    const X86_36: Target = Target {
        arch: "x86_64",
        sdk: 36,
    };
    const GMS: &str = "x86_64/36/system/product/priv-app/GmsCore/GmsCore.apk";
    const VENDING: &str = "x86_64/36/system/product/priv-app/Phonesky/Phonesky.apk";

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dsa-gapps-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make(zip: &[u8], target: &Target, name: &str) -> Result<(Built, Vec<u8>), String> {
        let dir = scratch(name);
        std::fs::write(dir.join("build.zip"), zip).unwrap();
        let image = dir.join("system.img");
        let result = build_image(&dir.join("build.zip"), target, &image)
            .map(|built| (built, std::fs::read(&image).unwrap()));
        if result.is_err() {
            assert!(!image.exists(), "a refused build leaves no image");
        }
        // No copy made to read a privileged app's manifest is left beside the image.
        assert!(!image.with_extension("scan.apk").exists(), "{name}");
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn fr32_the_image_holds_only_the_files_at_the_set_paths() {
        let (built, image) = make(&sample(), &X86_36, "sample").unwrap();
        let files = read::files(&image);
        let paths: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(
            paths,
            [
                "product/app/GoogleCalendarSyncAdapter/GoogleCalendarSyncAdapter.apk",
                "product/etc/permissions/dsa-gapps-privapp-permissions.xml",
                "product/etc/permissions/privapp-permissions-google.xml",
                "product/etc/sysconfig/google.xml",
                "product/framework/com.google.android.dialer.support.jar",
                "product/priv-app/GmsCore/GmsCore.apk",
                "product/priv-app/Phonesky/Phonesky.apk",
                "system_ext/etc/permissions/dsa-gapps-privapp-permissions.xml",
                "system_ext/etc/permissions/privapp-permissions-google-se.xml",
                "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk",
            ]
        );
        assert_eq!(files["product/priv-app/Phonesky/Phonesky.apk"], b"vending");
        assert_eq!(built.files, 10);
        // vendor/, lib64/, the setup wizard, and the installer's own files.
        assert!(built.left_out >= 3, "{built:?}");
    }

    /// The grants of the allowlist DSA adds to `partition`, by package.
    fn allowlist(
        files: &std::collections::BTreeMap<String, Vec<u8>>,
        partition: &str,
    ) -> BTreeMap<String, BTreeSet<String>> {
        let path = format!("{partition}/etc/permissions/{}", privapp::FILE_NAME);
        let mut read = BTreeMap::new();
        privapp::read_list(&String::from_utf8_lossy(&files[&path]), &mut read);
        read.into_iter()
            .map(|(package, decisions)| (package, decisions.granted))
            .collect()
    }

    #[test]
    fn fr32_the_image_allowlists_every_permission_its_privileged_apps_ask_for() {
        use super::fixture::{GMS_PERMISSIONS, GSF_PERMISSIONS};
        let (built, image) = make(&sample(), &X86_36, "allowlist").unwrap();
        let files = read::files(&image);
        // Google Play services asks for two; LiteGapps' list grants the first, so DSA's grants
        // the other, the one an Android 17 system_server stopped at.
        assert_eq!(
            allowlist(&files, "product"),
            BTreeMap::from([(
                "com.google.android.gms".to_string(),
                BTreeSet::from([GMS_PERMISSIONS[1].to_string()])
            )])
        );
        // The services framework is in system_ext, whose lists alone Android reads for it.
        assert_eq!(
            allowlist(&files, "system_ext"),
            BTreeMap::from([(
                "com.google.android.gsf".to_string(),
                BTreeSet::from([GSF_PERMISSIONS[0].to_string()])
            )])
        );
        assert_eq!(built.allowlisted, 2);
        // The sample's Play Store is no APK, so nothing can be said for it.
        assert_eq!(built.unread, ["product/priv-app/Phonesky/Phonesky.apk"]);
    }

    #[test]
    fn fr32_an_add_on_app_replacing_a_build_app_is_allowlisted_for_what_it_asks() {
        use super::fixture::apk;
        let old = apk("com.google.android.gms", &["android.permission.OLD"]);
        let new = apk("com.google.android.gms", &["android.permission.NEW"]);
        let vending = apk("com.android.vending", &[]);
        let zip = build(
            "x86_64",
            36,
            &[(GMS, &old[..]), (VENDING, &vending[..])],
            &[(
                "Common",
                addon(
                    "x86_64",
                    36,
                    &[
                        ("system/product/priv-app/GmsCore/GmsCore.apk", &new[..]),
                        (
                            "system/product/etc/permissions/deny.xml",
                            b"<permissions><privapp-permissions package=\"com.android.vending\">\
                              <deny-permission name=\"android.permission.X\"/>\
                              </privapp-permissions></permissions>",
                        ),
                    ],
                ),
            )],
        );
        let (built, image) = make(&zip, &X86_36, "replaced").unwrap();
        let files = read::files(&image);
        assert_eq!(
            allowlist(&files, "product"),
            BTreeMap::from([(
                "com.google.android.gms".to_string(),
                BTreeSet::from(["android.permission.NEW".to_string()])
            )])
        );
        assert_eq!(built.allowlisted, 1);
        assert!(built.unread.is_empty(), "{built:?}");
        assert_eq!(allowlist(&files, "system_ext"), BTreeMap::new());
    }

    #[test]
    fn fr32_a_build_with_a_path_outside_it_is_refused() {
        for bad in [
            "../evil.sh",
            "/system/bin/evil",
            "files/../../evil",
            "C:/Windows/evil",
            "a\\..\\evil",
        ] {
            let zip = archive(&[(bad, b"x")], b"");
            let why = make(&zip, &X86_36, "outside-zip").unwrap_err();
            assert!(why.contains("outside the build"), "{bad}: {why}");
        }
        let tar_traversal = build(
            "x86_64",
            36,
            &[
                (GMS, b"gms"),
                ("x86_64/36/system/product/../../../../etc/passwd", b"x"),
            ],
            &[],
        );
        let why = make(&tar_traversal, &X86_36, "outside-tar").unwrap_err();
        assert!(why.contains("outside the build"), "{why}");
        let addon_traversal = build(
            "x86_64",
            36,
            &[(GMS, b"gms"), (VENDING, b"v")],
            &[(
                "Common",
                addon("x86_64", 36, &[("system/../../evil.xml", b"x")]),
            )],
        );
        let why = make(&addon_traversal, &X86_36, "outside-addon").unwrap_err();
        assert!(why.contains("outside the build"), "{why}");
    }

    #[test]
    fn fr32_a_build_for_another_architecture_or_android_version_is_refused() {
        let arm = Target {
            arch: "arm64",
            sdk: 36,
        };
        let why = make(&sample(), &arm, "arch").unwrap_err();
        assert!(why.contains("x86_64"), "{why}");
        let newer = Target {
            arch: "x86_64",
            sdk: 37,
        };
        let why = make(&sample(), &newer, "sdk").unwrap_err();
        assert!(why.contains("API level"), "{why}");
        // A module.prop that says x86_64 at API level 36 over files for another Android.
        let mislabelled = build(
            "x86_64",
            36,
            &[
                (
                    "arm64/36/system/product/priv-app/GmsCore/GmsCore.apk",
                    b"gms",
                ),
                (
                    "arm64/36/system/product/priv-app/Phonesky/Phonesky.apk",
                    b"v",
                ),
            ],
            &[],
        );
        let why = make(&mislabelled, &X86_36, "mislabelled").unwrap_err();
        assert!(why.contains("GmsCore"), "{why}");
        let wrong_addon = build(
            "x86_64",
            36,
            &[(GMS, b"gms"), (VENDING, b"v")],
            &[(
                "Common",
                addon("arm64", 36, &[("system/product/etc/a.xml", b"x")]),
            )],
        );
        let why = make(&wrong_addon, &X86_36, "wrong-addon").unwrap_err();
        assert!(why.contains("another architecture"), "{why}");
    }

    #[test]
    fn fr32_a_file_that_is_not_a_core_build_is_refused() {
        assert!(make(b"not a zip at all, but long enough", &X86_36, "not-zip").is_err());
        let no_prop = archive(&[("files/files.tar.xz", b"x")], b"");
        let why = make(&no_prop, &X86_36, "no-prop").unwrap_err();
        assert!(why.contains("module.prop"), "{why}");
        let lite = build_of("lite", "x86_64", 36, &[(GMS, b"gms"), (VENDING, b"v")], &[]);
        let why = make(&lite, &X86_36, "lite").unwrap_err();
        assert!(why.contains("lite"), "{why}");
    }

    #[test]
    fn fr32_paths_are_judged_by_their_parts() {
        assert_eq!(inside("./a/b/").unwrap(), "a/b");
        assert!(inside("a/../b").is_err());
        assert!(inside("..").is_err());
        assert!(inside("/a").is_err());
        assert_eq!(
            image_path("product/priv-app/GmsCore/GmsCore.apk").as_deref(),
            Some("product/priv-app/GmsCore/GmsCore.apk")
        );
        assert_eq!(image_path("product/lib64/libx.so"), None);
        assert_eq!(image_path("vendor/etc/a.xml"), None);
        assert_eq!(image_path("product/etc"), None);
        // In etc, only the folders the guest lays over.
        assert_eq!(
            image_path("product/etc/default-permissions/google.xml").as_deref(),
            Some("product/etc/default-permissions/google.xml")
        );
        assert_eq!(image_path("product/etc/google.xml"), None);
        assert_eq!(image_path("product/etc/security/fsverity/x.der"), None);
        assert_eq!(image_path("system_ext/etc/permissions"), None);
    }

    #[test]
    fn fr32_the_image_holds_every_folder_the_guest_lays_over() {
        // The guest lays each of these over the same folder of its system with an overlay,
        // which fails when the image lacks it, so each is there even when the build has
        // nothing for it.
        let (_, image) = make(&sample(), &X86_36, "folders").unwrap();
        let folders: Vec<String> = read::items(&image)
            .into_iter()
            .filter(|item| item.mode & 0o170_000 == 0o040_000)
            .map(|item| item.path)
            .collect();
        let overlaid = overlaid_folders();
        assert_eq!(overlaid.len(), 14);
        for folder in &overlaid {
            assert!(folders.contains(folder), "{folder} is not in {folders:?}");
        }
        assert!(overlaid.contains(&"system_ext/etc/preferred-apps".to_string()));
        assert!(!overlaid.iter().any(|folder| folder == "product/etc"));
    }
}
