//! ARM app translation: a translation package (a native bridge with its ARM64 libraries) the
//! user installs, and what the user's Android is told at boot.
//!
//! Most apps ship ARM native code only. The guest is x86_64, so such an app installs and runs
//! only with a translator, which plugs into Android as its native bridge. None that DSA could
//! ship is ready yet, so the user brings one as a package: a folder holding
//!
//! ```text
//! arm-translation.json   {"name": "...", "version": "...", "library": "libberberis_arm64.so",
//!                         "abis": ["arm64-v8a"], "license": "Apache-2.0"}
//! system.img             an erofs image whose system/lib64, system/bin and system/etc are
//!                        laid over the guest's, with system/lib64/libdsa_native_bridge.so
//!                        being (or linking to) the bridge library
//! ```
//!
//! The host checks the description and the image's erofs signature, sends the image to the
//! user's Android, which keeps it on the user's disk, and keeps the description with the
//! user's settings. While a package is installed and the user's `arm-translation` setting is
//! on, each boot's bootconfig carries [`BOOTCONFIG_ON`]: the guest then imports the bridge and
//! ABI properties and mounts the image before its zygote starts. Without it the guest is
//! exactly the image as made, so an x86_64 app sees no difference. The ABIs Android installs
//! apps for are fixed at boot, so every change applies from the next start of Android.
//!
//! [`google`] makes such a package, on the user's request, from Google's own ARM translator:
//! the user's computer downloads it from Google, and DSA never ships it.

pub mod google;

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The package's description, in its folder.
pub const MANIFEST: &str = "arm-translation.json";
/// The package's system image, in its folder.
pub const IMAGE: &str = "system.img";
/// The bootconfig line that turns translation on for one boot.
pub const BOOTCONFIG_ON: &str = "androidboot.dsa.arm_translation = \"on\"\n";
/// The only ABI a package may translate: the guest has no 32-bit zygote.
pub const ARM64: &str = "arm64-v8a";
/// The largest image taken.
pub const MAX_IMAGE_BYTES: u64 = 2 << 30;

/// Where erofs keeps its superblock, and the signature it starts with.
const EROFS_SUPERBLOCK: usize = 1024;
const EROFS_MAGIC: u32 = 0xE0F5_E1E2;

/// What a package says of itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// The translator's native bridge library, a file name in the image's `system/lib64`.
    pub library: String,
    /// The ABIs it translates.
    pub abis: Vec<String>,
    /// Its licence, as an SPDX expression.
    #[serde(default)]
    pub license: String,
}

/// A text field of the description: printable, without spaces at either end, not too long.
fn check_text(what: &str, value: &str, max: usize) -> Result<(), String> {
    if value.is_empty() || value.chars().count() > max {
        return Err(format!(
            "the package's {what} must have from 1 to {max} characters"
        ));
    }
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(format!(
            "the package's {what} {value:?} has spaces at an end or a control character"
        ));
    }
    Ok(())
}

impl Package {
    /// Checks the description, and says what is wrong with it.
    pub fn check(&self) -> Result<(), String> {
        check_text("name", &self.name, 64)?;
        check_text("version", &self.version, 64)?;
        if !self.license.is_empty() {
            check_text("license", &self.license, 128)?;
        }
        let library_ok = self.library.len() <= 128
            && self.library.ends_with(".so")
            && self.library.len() > 3
            && self
                .library
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !library_ok {
            return Err(format!(
                "the bridge library {:?} is not a library file name (letters, digits, '.', \
                 '_' and '-', ending in .so)",
                self.library
            ));
        }
        if self.abis.is_empty() {
            return Err(format!(
                "the package names no ABI; it must translate {ARM64}"
            ));
        }
        if let Some(other) = self.abis.iter().find(|abi| abi.as_str() != ARM64) {
            return Err(format!(
                "the package translates {other}, but Android here runs only 64-bit apps, so \
                 only {ARM64} is taken"
            ));
        }
        Ok(())
    }
}

/// Whether `header`, the start of an image, is an erofs file system.
pub fn is_erofs(header: &[u8]) -> bool {
    header
        .get(EROFS_SUPERBLOCK..EROFS_SUPERBLOCK + 4)
        .map(|magic| u32::from_le_bytes([magic[0], magic[1], magic[2], magic[3]]) == EROFS_MAGIC)
        .unwrap_or(false)
}

/// A package folder, checked.
#[derive(Debug, Clone)]
pub struct PackageDir {
    pub package: Package,
    pub image: PathBuf,
    pub size: u64,
}

/// Reads and checks the package in `dir`: its description, and that its image is erofs and
/// not too large.
pub fn open_package(dir: &Path) -> Result<PackageDir, String> {
    let manifest = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest).map_err(|e| {
        format!(
            "{} is not an ARM translation package: {} cannot be read ({e})",
            dir.display(),
            manifest.display()
        )
    })?;
    let package: Package = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a usable description: {e}", manifest.display()))?;
    package.check()?;
    let image = dir.join(IMAGE);
    let mut file = std::fs::File::open(&image)
        .map_err(|e| format!("the package's {} cannot be read: {e}", image.display()))?;
    let size = file
        .metadata()
        .map_err(|e| format!("the package's {} cannot be read: {e}", image.display()))?
        .len();
    if size == 0 || size > MAX_IMAGE_BYTES {
        return Err(format!(
            "the package's {IMAGE} is {size} bytes; it must have from 1 byte to {} MiB",
            MAX_IMAGE_BYTES >> 20
        ));
    }
    let mut header = Vec::with_capacity(EROFS_SUPERBLOCK + 4);
    let _ = file
        .by_ref()
        .take((EROFS_SUPERBLOCK + 4) as u64)
        .read_to_end(&mut header);
    if !is_erofs(&header) {
        return Err(format!(
            "the package's {IMAGE} is not an erofs image; make the package with \
             dsa/guest/arm-translation/make-package.py"
        ));
    }
    Ok(PackageDir {
        package,
        image,
        size,
    })
}

/// The SHA-256 digest of the file at `path`, which the guest checks the image it receives
/// against.
pub fn file_sha256(path: &Path) -> Result<Vec<u8>, String> {
    use sha2::Digest;
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Ok(hasher.finalize().to_vec()),
            Ok(read) => hasher.update(&buffer[..read]),
            Err(e) => return Err(format!("{} cannot be read: {e}", path.display())),
        }
    }
}

/// What a boot's bootconfig carries for ARM translation: [`BOOTCONFIG_ON`] while a package is
/// installed and the user's setting is on, nothing otherwise.
pub fn bootconfig(installed: Option<&Package>, enabled: bool) -> &'static str {
    if installed.is_some() && enabled {
        BOOTCONFIG_ON
    } else {
        ""
    }
}

/// Where ARM translation stands in the running guest, as its control daemon says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuestArmTranslation {
    /// A package image is on the user's disk.
    pub installed: bool,
    /// This boot runs with the package laid over the system.
    pub active: bool,
    /// The ABIs the guest's package manager installs apps for.
    pub abilist: String,
}

/// Whether what the user has installed and switched on differs from what the running guest
/// booted with, so that it applies only once Android starts again.
pub fn restart_needed(
    installed: Option<&Package>,
    enabled: bool,
    guest: Option<&GuestArmTranslation>,
) -> bool {
    match guest {
        Some(guest) => guest.active != (installed.is_some() && enabled),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package() -> Package {
        Package {
            name: "Digitalis".into(),
            version: "16.0.1".into(),
            library: "libberberis_arm64.so".into(),
            abis: vec!["arm64-v8a".into()],
            license: "Apache-2.0".into(),
        }
    }

    fn erofs(size: usize) -> Vec<u8> {
        let mut image = vec![0u8; size];
        image[1024..1028].copy_from_slice(&0xE0F5_E1E2u32.to_le_bytes());
        image
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsa-arm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fr25_a_package_must_name_a_library_file_and_translate_arm64_only() {
        assert_eq!(package().check(), Ok(()));
        for library in [
            "",
            "libbridge",
            "../lib/libx.so",
            "lib x.so",
            "/system/lib64/l.so",
        ] {
            let bad = Package {
                library: library.into(),
                ..package()
            };
            assert!(bad.check().is_err(), "{library:?} is taken");
        }
        for abis in [
            vec![],
            vec!["armeabi-v7a".to_string()],
            vec!["x86_64".to_string()],
        ] {
            let bad = Package {
                abis: abis.clone(),
                ..package()
            };
            assert!(bad.check().is_err(), "{abis:?} is taken");
        }
        let blank = Package {
            name: " ".into(),
            ..package()
        };
        assert!(blank.check().is_err());
    }

    #[test]
    fn fr25_a_package_folder_needs_its_description_and_an_erofs_image() {
        let dir = scratch("folder");
        assert!(open_package(&dir).unwrap_err().contains(MANIFEST));
        std::fs::write(
            dir.join(MANIFEST),
            serde_json::to_string(&package()).unwrap(),
        )
        .unwrap();
        assert!(open_package(&dir).is_err(), "no image");
        std::fs::write(dir.join(IMAGE), vec![7u8; 4096]).unwrap();
        assert!(open_package(&dir).unwrap_err().contains("erofs"));
        std::fs::write(dir.join(IMAGE), erofs(4096)).unwrap();
        let opened = open_package(&dir).unwrap();
        assert_eq!(opened.package, package());
        assert_eq!(opened.size, 4096);
        assert_eq!(opened.image, dir.join(IMAGE));
        let wrong = Package {
            abis: vec!["armeabi-v7a".into()],
            ..package()
        };
        std::fs::write(dir.join(MANIFEST), serde_json::to_string(&wrong).unwrap()).unwrap();
        assert!(open_package(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fr25_the_bootconfig_turns_translation_on_only_with_a_package_and_the_setting() {
        let installed = package();
        assert_eq!(bootconfig(None, true), "");
        assert_eq!(bootconfig(Some(&installed), false), "");
        assert_eq!(
            bootconfig(Some(&installed), true),
            "androidboot.dsa.arm_translation = \"on\"\n"
        );
    }

    #[test]
    fn fr25_a_restart_is_needed_when_the_guest_booted_otherwise() {
        let installed = package();
        let active = GuestArmTranslation {
            installed: true,
            active: true,
            abilist: "x86_64,arm64-v8a".into(),
        };
        let inactive = GuestArmTranslation::default();
        assert!(
            !restart_needed(Some(&installed), true, None),
            "nothing known"
        );
        assert!(restart_needed(Some(&installed), true, Some(&inactive)));
        assert!(!restart_needed(Some(&installed), true, Some(&active)));
        assert!(restart_needed(None, true, Some(&active)), "removed");
        assert!(
            restart_needed(Some(&installed), false, Some(&active)),
            "turned off"
        );
        assert!(!restart_needed(None, true, Some(&inactive)));
    }
}
