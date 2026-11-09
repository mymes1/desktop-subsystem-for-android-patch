//! Google Play for a user who asks for it: LiteGapps' Core build of Google's apps, which the
//! service downloads, turns into a system image and hands to the user's Android.
//!
//! DSA ships no Google app. On the user's request the service reads the list of builds
//! LiteGapps publishes on its file host for the Android version and architecture the running
//! Android reports ([`listing`]), downloads the newest and checks it against the MD5 digest the
//! host publishes, then makes an erofs image of the build's apps, permission lists, system
//! configuration and framework files ([`package`]). Every path in the build is checked first:
//! one that leaves the build (`..`, an absolute path) refuses the whole build, and so does a
//! build for another architecture or Android version. The image goes on the user's add-on disk
//! ([`crate::addon_disk`], `gapps.img`), which the host attaches to their VM read only, and the
//! build's description goes with the user's settings. While one is recorded, each boot's
//! bootconfig carries [`BOOTCONFIG_ON`] and says where the image is, and the guest then lays
//! each folder of the image over the same folder of its `/product` and `/system_ext` before its
//! package manager starts, never a partition's root, whose services would then fail to start.
//! Every change applies from the next start of Android, because the package manager reads the
//! system apps and their permissions once.
//!
//! A DSA from before the add-on disk sent the image to the user's Android disk
//! (`/data/vendor/dsa/gapps/system.img`). The first start after the update mounts it from there
//! once more, and the host moves it onto the add-on disk and has the guest delete it
//! ([`Upkeep::MoveImage`]), so nothing of it stays in `/data`.
//!
//! ```text
//! image root
//!   product/{app,priv-app,framework}/...             over the same folders of /product
//!   product/etc/{permissions,sysconfig,default-permissions,preferred-apps}/...
//!   system_ext/...                                   the same folders, over /system_ext
//! ```

pub mod listing;
pub mod md5;
pub mod package;
pub mod privapp;
pub mod tar;
pub mod xz;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::settings::{user_file_stem, SettingsStore};
use crate::update::store::Fetch;

/// The bootconfig line that lays the Google apps over the system for one boot.
pub const BOOTCONFIG_ON: &str = "androidboot.dsa.gapps = \"on\"\n";
/// The bootconfig key that says where the image is for one boot
/// ([`crate::addon_disk::image_bootconfig`]).
pub const IMAGE_KEY: &str = "gapps_image";
/// The format of the Google apps image this host makes, recorded with the package
/// ([`GappsPackage::image_format`]). A record without one (0) is of an image an older host
/// made: 0.1.9's, which its guest never laid over, and 0.2.0's, which lacks the folders the
/// guest now lays over (an overlay of a folder the image lacks fails) and the privileged
/// permission allowlist ([`privapp`]), without which Android stops at every boot when Google's
/// apps ask for a privileged permission LiteGapps' lists leave out. Such an image is never laid
/// over the system: it is made again from the kept package, or the user installs it again.
///
/// 2: every folder the guest lays over, and an allowlist in each partition.
pub const IMAGE_FORMAT: u32 = 2;
/// The name of the kept package, in the user's folder of the package folder.
const KEPT_PACKAGE: &str = "gapps-package.zip";
/// LiteGapps' name for the build DSA installs: Google Play services, the Play Store, the
/// services framework, and the account sync adapters.
pub const VARIANT: &str = "core";
/// The API levels of the Android versions DSA runs, which LiteGapps builds for.
pub const SDKS: [u32; 2] = [36, 37];
/// Google's terms, which Google's apps follow.
pub const TERMS_URL: &str = "https://policies.google.com/terms";
/// Google's page where a user registers a device Google has not certified, as DSA's Android
/// is, so that Google's apps let them sign in.
pub const REGISTRATION_URL: &str = "https://www.google.com/android/uncertified/";

/// The build of Google's apps a user installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GappsPackage {
    /// LiteGapps' name for the build: [`VARIANT`].
    pub variant: String,
    /// The build's date, yyyymmdd.
    pub version: String,
    /// The Android version the build is for, as LiteGapps names it ("16.0").
    pub android_version: String,
    /// The Android API level the build is for.
    pub sdk: u32,
    /// The architecture the build is for, as LiteGapps names it ([`Target::arch`]).
    pub arch: String,
    /// The file downloaded.
    pub file: String,
    /// Where it came from.
    pub source: String,
    /// The MD5 digest the file host published for it.
    pub md5: String,
    /// The format of the image made from it: [`IMAGE_FORMAT`] for an image this host made, 0
    /// for one a host from before formats were recorded made.
    #[serde(default)]
    pub image_format: u32,
}

impl GappsPackage {
    /// Whether the image made from it is of an older format than this host makes, so that no
    /// boot lays it over the system.
    pub fn outdated(&self) -> bool {
        self.image_format < IMAGE_FORMAT
    }
}

/// What a build must be for: the Android the user runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// LiteGapps' name for the architecture: `x86_64` or `arm64`.
    pub arch: &'static str,
    pub sdk: u32,
}

impl Target {
    /// The build a guest with API level `sdk` and primary ABI `abi` needs, or why there is none.
    pub fn for_guest(sdk: u32, abi: &str) -> Result<Target, String> {
        let arch = match abi {
            "x86_64" => "x86_64",
            "arm64-v8a" => "arm64",
            "" => return Err("Android did not say which processor it runs on".into()),
            other => {
                return Err(format!(
                    "Android runs on {other}, which DSA installs Google Play for only on \
                     x86_64 and arm64-v8a"
                ))
            }
        };
        if !SDKS.contains(&sdk) {
            return Err(format!(
                "Android here has API level {sdk}; DSA installs Google Play for API levels {} \
                 and {}",
                SDKS[0], SDKS[1]
            ));
        }
        Ok(Target { arch, sdk })
    }
}

/// What a boot's bootconfig carries for the Google apps: [`BOOTCONFIG_ON`] while a build is
/// installed whose image is of the format this host makes, with where the image is: on the
/// add-on disk the boot attaches when `on_disk`, else where an older DSA kept it in `/data`.
/// Nothing otherwise.
pub fn bootconfig(installed: Option<&GappsPackage>, on_disk: bool) -> String {
    if installed.is_some_and(|package| !package.outdated()) {
        format!(
            "{BOOTCONFIG_ON}{}",
            crate::addon_disk::image_bootconfig(IMAGE_KEY, on_disk)
        )
    } else {
        String::new()
    }
}

/// The bootconfig for `user`'s next boot, from what their settings record and whether the
/// add-on disk the boot attaches holds the image (`attached`). A record that cannot be read is
/// logged and counts as none, so Android still starts; so does the record of an image an older
/// host made, which is made again once Android runs, or installed again by the user.
pub fn bootconfig_for(
    store: &dyn SettingsStore,
    user: &str,
    attached: &[crate::addon_disk::Addon],
) -> String {
    let package = store.gapps(user).unwrap_or_else(|why| {
        eprintln!("gapps: {why}; starting {user}'s Android without Google's apps");
        None
    });
    if let Some(package) = package.as_ref().filter(|package| package.outdated()) {
        eprintln!(
            "gapps: {user}'s Google apps image (LiteGapps {} {}) was made by an older DSA, in \
             format {} where this one makes {IMAGE_FORMAT}; starting Android without Google's \
             apps",
            package.variant, package.version, package.image_format
        );
    }
    let on_disk = attached.contains(&crate::addon_disk::Addon::Gapps);
    if package.as_ref().is_some_and(|package| !package.outdated()) && !on_disk {
        eprintln!(
            "gapps: {user}'s Google apps image is not on their add-on disk; Android mounts the \
             copy an older DSA kept in /data once more, and the service moves it to the disk"
        );
    }
    bootconfig(package.as_ref(), on_disk)
}

/// Where the Google apps stand in the running guest, as it says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuestGapps {
    /// The image is on the user's add-on disk, for the next start of Android. The host fills
    /// this in from the disk; the guest does not know it.
    pub installed: bool,
    /// An image a DSA from before the add-on disk sent is still on the user's Android disk.
    pub left_on_data: bool,
    /// This boot runs with the image laid over the system.
    pub active: bool,
    /// The guest's API level.
    pub sdk: u32,
    /// The guest's primary ABI.
    pub abi: String,
    /// The ABIs the guest installs apps for, in the order it prefers them.
    pub abilist: String,
    /// The Google Services Framework Android ID, in decimal; empty until there is one.
    pub device_id: String,
}

/// Whether what the user has installed differs from what the running guest booted with, so
/// that it applies only once Android starts again. An image of an older format is never laid
/// over, so a restart does nothing for it.
pub fn restart_needed(installed: Option<&GappsPackage>, guest: Option<&GuestGapps>) -> bool {
    let usable = installed.is_some_and(|package| !package.outdated());
    guest.is_some_and(|guest| guest.active != usable)
}

/// Whether the user must install Google Play again: their package's image is of an older format,
/// or is neither on the add-on disk (`on_disk`) nor, as far as the running guest says, left in
/// its `/data`, and its download was not kept (`kept`), so it cannot be made again.
pub fn reinstall_needed(
    installed: Option<&GappsPackage>,
    on_disk: bool,
    guest: Option<&GuestGapps>,
    kept: bool,
) -> bool {
    let Some(package) = installed else {
        return false;
    };
    let lost = !on_disk && guest.is_some_and(|guest| !guest.left_on_data);
    (package.outdated() || lost) && !kept
}

/// What a boot of a user's Android that reached its desktop needs done for the Google apps,
/// from what their settings record and what the guest says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upkeep {
    /// Nothing.
    Nothing,
    /// An image an older DSA sent is still on the Android disk, and is of no more use: the user
    /// removed Google Play, or the image is on the add-on disk already. It goes now.
    RemoveImage,
    /// An image an older DSA sent is on the Android disk and not yet on the add-on disk: it is
    /// moved there and then removed from the Android disk.
    MoveImage,
    /// The image is of an older format, or is lost, and the package is kept: it is made again
    /// and put on the add-on disk, and any copy on the Android disk goes.
    Rebuild,
    /// The image is of an older format, or is lost, and the package is not kept: the user
    /// installs Google Play again, as their status says. Any copy on the Android disk, of no
    /// use, goes.
    Reinstall,
}

/// What `user`'s running Android needs, from what their settings record (`installed`), what
/// the guest says, whether the add-on disk holds the image (`on_disk`) and whether the package
/// is kept: see [`Upkeep`].
pub fn upkeep(
    installed: Option<&GappsPackage>,
    guest: &GuestGapps,
    on_disk: bool,
    kept: bool,
) -> Upkeep {
    let Some(package) = installed else {
        return if guest.left_on_data {
            Upkeep::RemoveImage
        } else {
            Upkeep::Nothing
        };
    };
    if !package.outdated() && on_disk {
        return if guest.left_on_data {
            Upkeep::RemoveImage
        } else {
            Upkeep::Nothing
        };
    }
    if !package.outdated() && guest.left_on_data {
        return Upkeep::MoveImage;
    }
    if kept {
        Upkeep::Rebuild
    } else {
        Upkeep::Reinstall
    }
}

/// What an install is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Reading LiteGapps' list of builds.
    Listing,
    /// Downloading the build.
    Downloading,
    /// Checking the build and making its image.
    Building,
    /// Putting the image on the add-on disk.
    Sending,
}

/// News of an install: what it is doing, and how far it has got (`done` of `total` bytes;
/// both 0 when there is nothing to count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    pub done: u64,
    pub total: u64,
}

/// How often news of bytes is given: every this many bytes, and at the end.
const NEWS_STEP: u64 = 4 << 20;

/// Where the service gets Google Play from, and where it makes it ready.
pub struct Source {
    fetch: Arc<dyn Fetch + Send + Sync>,
    /// LiteGapps' project on its file host ([`listing::PROJECT`]).
    project: String,
    /// The folder downloads and images are made in, a folder of each user's in it; the service
    /// owns it, and nothing stays there once an install ends.
    work_dir: PathBuf,
    /// The folder each user's installed package is kept in, a folder of each user's in it, so
    /// that a later DSA can make its image again in a new format without a download. It must
    /// outlast the service, unlike a temporary folder.
    package_dir: PathBuf,
    /// Held for the length of one install, so two do not share the folder or the bandwidth.
    turn: Mutex<()>,
}

/// A build downloaded, checked and made into an image for a user's Android. The image is
/// removed when this is dropped, and so is the download unless it was kept ([`Source::keep`]).
pub struct Prepared {
    pub package: GappsPackage,
    pub image: PathBuf,
    pub size: u64,
    pub sha256: Vec<u8>,
    pub built: package::Built,
    /// The downloaded package, until it is kept; `None` for an image made from the kept one.
    download: Option<PathBuf>,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.image);
        if let Some(download) = &self.download {
            let _ = std::fs::remove_file(download);
        }
    }
}

impl Source {
    /// Downloads through `fetch` from LiteGapps' project, and makes builds ready in `work_dir`,
    /// where the installed packages are kept too unless [`with_package_dir`](Self::with_package_dir)
    /// names another folder.
    pub fn new(fetch: Arc<dyn Fetch + Send + Sync>, work_dir: impl Into<PathBuf>) -> Source {
        let work_dir = work_dir.into();
        Source {
            fetch,
            project: listing::PROJECT.to_string(),
            package_dir: work_dir.clone(),
            work_dir,
            turn: Mutex::new(()),
        }
    }

    /// Reads the builds from `project` instead, a mirror with the same layout.
    pub fn with_project(mut self, project: &str) -> Source {
        self.project = project.trim_end_matches('/').to_string();
        self
    }

    /// Keeps each user's installed package in `dir`, for a work folder that does not last.
    pub fn with_package_dir(mut self, dir: impl Into<PathBuf>) -> Source {
        self.package_dir = dir.into();
        self
    }

    /// Where `user`'s installed package is kept.
    pub fn kept_package(&self, user: &str) -> Result<PathBuf, String> {
        Ok(self
            .package_dir
            .join(user_file_stem(user)?)
            .join(KEPT_PACKAGE))
    }

    /// Whether `user`'s installed package is kept, so its image can be made again.
    pub fn is_kept(&self, user: &str) -> bool {
        self.kept_package(user).is_ok_and(|path| path.is_file())
    }

    /// Keeps the package `prepared` was made from as `user`'s, in place of any kept before,
    /// once its image is on their Android.
    pub fn keep(&self, user: &str, prepared: &mut Prepared) -> Result<(), String> {
        let Some(download) = prepared.download.take() else {
            return Ok(());
        };
        let kept = self.kept_package(user)?;
        let result = (|| {
            if let Some(dir) = kept.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("{} cannot be made: {e}", dir.display()))?;
            }
            if std::fs::rename(&download, &kept).is_err() {
                std::fs::copy(&download, &kept)
                    .map_err(|e| format!("{} cannot be written: {e}", kept.display()))?;
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(&download);
        if result.is_err() {
            let _ = std::fs::remove_file(&kept);
        }
        result
    }

    /// Removes `user`'s kept package, as removing Google Play does.
    pub fn forget(&self, user: &str) {
        if let Ok(kept) = self.kept_package(user) {
            match std::fs::remove_file(&kept) {
                Ok(()) => eprintln!("gapps: removed the package kept for {user}"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => eprintln!("gapps: {} cannot be removed: {e}", kept.display()),
            }
        }
    }

    /// Makes the image of `user`'s installed `package` again, from the package kept for them,
    /// in the format this host makes. The kept file must still be the one downloaded: its MD5
    /// digest must be the one recorded with it.
    pub fn rebuild(&self, user: &str, package: &GappsPackage) -> Result<Prepared, String> {
        let _turn = self.turn.lock().unwrap_or_else(|e| e.into_inner());
        let kept = self.kept_package(user)?;
        if !kept.is_file() {
            return Err(format!(
                "the package {} is no longer kept, so install Google Play again",
                package.file
            ));
        }
        let digest = file_md5(&kept)?;
        if !digest.eq_ignore_ascii_case(&package.md5) {
            return Err(format!(
                "the kept package {} has MD5 {digest}, not the {} it was downloaded with",
                kept.display(),
                package.md5
            ));
        }
        let target = Target::for_guest(
            package.sdk,
            match package.arch.as_str() {
                "arm64" => "arm64-v8a",
                other => other,
            },
        )?;
        let dir = self.work_dir.join(user_file_stem(user)?);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("{} cannot be made: {e}", dir.display()))?;
        let image = dir.join("gapps-system.img");
        let built = package::build_image(&kept, &target, &image)?;
        let mut package = package.clone();
        package.image_format = IMAGE_FORMAT;
        measure(package, image, built, None)
    }

    /// Finds the newest build for `target`, downloads and checks it, and makes its image for
    /// `user`, telling `progress` how it goes. The download is removed once the image is made.
    pub fn prepare(
        &self,
        user: &str,
        target: &Target,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Prepared, String> {
        let _turn = self.turn.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.work_dir.join(user_file_stem(user)?);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("{} cannot be made: {e}", dir.display()))?;
        progress(Progress {
            stage: Stage::Listing,
            done: 0,
            total: 0,
        });
        let build = listing::newest(&*self.fetch, &self.project, target)?;
        let total = build.size;
        let zip = dir.join("gapps-download.zip");
        let mut told = 0u64;
        progress(Progress {
            stage: Stage::Downloading,
            done: 0,
            total,
        });
        let package = listing::download(
            &*self.fetch,
            &self.project,
            target,
            &build,
            &zip,
            &mut |done| {
                if done >= told + NEWS_STEP || done == total {
                    told = done;
                    progress(Progress {
                        stage: Stage::Downloading,
                        done,
                        total,
                    });
                }
            },
        )?;
        progress(Progress {
            stage: Stage::Building,
            done: 0,
            total: 0,
        });
        let image = dir.join("gapps-system.img");
        let built = match package::build_image(&zip, target, &image) {
            Ok(built) => built,
            Err(why) => {
                let _ = std::fs::remove_file(&zip);
                return Err(why);
            }
        };
        let mut package = package;
        package.image_format = IMAGE_FORMAT;
        if let Some(unread) = built.unread.first() {
            eprintln!(
                "gapps: {unread} and {} more privileged apps of LiteGapps {} {} have no manifest \
                 DSA could read",
                built.unread.len() - 1,
                package.variant,
                package.version
            );
        }
        measure(package, image, built, Some(zip))
    }
}

/// The size and SHA-256 digest of the image just made, which `Prepared` then holds. On a failure
/// the image and the download are removed.
fn measure(
    package: GappsPackage,
    image: PathBuf,
    built: package::Built,
    download: Option<PathBuf>,
) -> Result<Prepared, String> {
    let measured = std::fs::metadata(&image)
        .map_err(|e| format!("{} cannot be read: {e}", image.display()))
        .and_then(|metadata| {
            crate::arm_translation::file_sha256(&image).map(|sha| (metadata.len(), sha))
        });
    match measured {
        Ok((size, sha256)) => Ok(Prepared {
            package,
            image,
            size,
            sha256,
            built,
            download,
        }),
        Err(why) => {
            let _ = std::fs::remove_file(&image);
            if let Some(download) = download {
                let _ = std::fs::remove_file(download);
            }
            Err(why)
        }
    }
}

/// The MD5 digest of the file at `path`, in lower-case hexadecimal.
fn file_md5(path: &Path) -> Result<String, String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
    let mut digest = md5::Md5::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(md5::hex(&digest.finish()))
}

/// A reader that says how many bytes have been read, every [`NEWS_STEP`] bytes and at the end.
pub struct Reporting<'a> {
    inner: &'a mut dyn Read,
    done: u64,
    told: u64,
    report: &'a mut dyn FnMut(u64),
}

impl<'a> Reporting<'a> {
    pub fn new(inner: &'a mut dyn Read, report: &'a mut dyn FnMut(u64)) -> Reporting<'a> {
        Reporting {
            inner,
            done: 0,
            told: 0,
            report,
        }
    }
}

impl Read for Reporting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.done += read as u64;
        if self.done >= self.told + NEWS_STEP || (read == 0 && self.done > self.told) {
            self.told = self.done;
            (self.report)(self.done);
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package() -> GappsPackage {
        GappsPackage {
            variant: "core".into(),
            version: "20261001".into(),
            android_version: "16.0".into(),
            sdk: 36,
            arch: "x86_64".into(),
            file: "LiteGapps-core-x86_64-16.0-20261001-official.zip".into(),
            source: "https://example.com/LiteGapps-core-x86_64-16.0-20261001-official.zip".into(),
            md5: "ea9945a6d63849fa9e51782edfe613a4".into(),
            image_format: IMAGE_FORMAT,
        }
    }

    #[test]
    fn fr32_the_bootconfig_lays_the_google_apps_over_only_while_they_are_installed() {
        assert_eq!(bootconfig(None, true), "");
        assert_eq!(
            bootconfig(Some(&package()), true),
            "androidboot.dsa.gapps = \"on\"\nandroidboot.dsa.gapps_image = \"disk\"\n"
        );
        // Until the host has moved it, the image is where an older DSA kept it.
        assert_eq!(
            bootconfig(Some(&package()), false),
            "androidboot.dsa.gapps = \"on\"\nandroidboot.dsa.gapps_image = \"data\"\n"
        );
    }

    #[test]
    fn fr32_a_build_is_chosen_for_the_guests_architecture_and_android_version() {
        assert_eq!(
            Target::for_guest(36, "x86_64"),
            Ok(Target {
                arch: "x86_64",
                sdk: 36
            })
        );
        assert_eq!(
            Target::for_guest(37, "arm64-v8a"),
            Ok(Target {
                arch: "arm64",
                sdk: 37
            })
        );
        assert!(Target::for_guest(35, "x86_64").is_err());
        assert!(Target::for_guest(36, "armeabi-v7a").is_err());
        assert!(Target::for_guest(36, "").is_err());
    }

    #[test]
    fn fr32_a_restart_is_needed_when_the_guest_booted_otherwise() {
        let installed = package();
        let active = GuestGapps {
            installed: true,
            active: true,
            ..GuestGapps::default()
        };
        let inactive = GuestGapps::default();
        assert!(!restart_needed(Some(&installed), None), "nothing known");
        assert!(
            restart_needed(Some(&installed), Some(&inactive)),
            "installed"
        );
        assert!(!restart_needed(Some(&installed), Some(&active)));
        assert!(restart_needed(None, Some(&active)), "removed");
        assert!(!restart_needed(None, Some(&inactive)));
        // An image an older host made is never laid over, so a restart would change nothing.
        let old = GappsPackage {
            image_format: 0,
            ..package()
        };
        assert!(!restart_needed(Some(&old), Some(&inactive)));
    }

    #[test]
    fn fr32_an_image_made_by_an_older_host_is_never_laid_over() {
        // What 0.2.0 recorded: no format, since its image had none.
        let recorded = r#"{"variant":"core","version":"20261001","android_version":"17.0",
            "sdk":37,"arch":"x86_64","file":"LiteGapps-core-x86_64-17.0-20261001-official.zip",
            "source":"https://example.com/x.zip","md5":"ea9945a6d63849fa9e51782edfe613a4"}"#;
        let old: GappsPackage = serde_json::from_str(recorded).unwrap();
        assert_eq!(old.image_format, 0);
        assert!(old.outdated());
        assert_eq!(bootconfig(Some(&old), true), "");
        assert!(!package().outdated());
        let store = crate::settings::MemorySettings::default();
        let disk = [crate::addon_disk::Addon::Gapps];
        store.set_gapps("uid:1000", Some(&old)).unwrap();
        assert_eq!(bootconfig_for(&store, "uid:1000", &disk), "");
        store.set_gapps("uid:1000", Some(&package())).unwrap();
        assert!(bootconfig_for(&store, "uid:1000", &disk).starts_with(BOOTCONFIG_ON));
        // The user is told to install it again only when it cannot be made again.
        assert!(reinstall_needed(Some(&old), true, None, false));
        assert!(!reinstall_needed(Some(&old), true, None, true));
        assert!(!reinstall_needed(Some(&package()), true, None, false));
        assert!(!reinstall_needed(None, false, None, false));
    }

    #[test]
    fn fr32_a_boot_that_reaches_the_desktop_mends_the_google_apps() {
        let old = GappsPackage {
            image_format: 0,
            ..package()
        };
        let left = GuestGapps {
            left_on_data: true,
            ..GuestGapps::default()
        };
        let none = GuestGapps::default();
        assert_eq!(upkeep(None, &left, false, false), Upkeep::RemoveImage);
        assert_eq!(upkeep(None, &none, false, true), Upkeep::Nothing);
        assert_eq!(upkeep(Some(&old), &left, false, true), Upkeep::Rebuild);
        assert_eq!(upkeep(Some(&old), &left, false, false), Upkeep::Reinstall);
        // The add-on disk has the image: nothing stays in /data.
        assert_eq!(
            upkeep(Some(&package()), &left, true, false),
            Upkeep::RemoveImage
        );
        assert_eq!(
            upkeep(Some(&package()), &none, true, false),
            Upkeep::Nothing
        );
    }

    #[test]
    fn fr32_an_image_an_older_dsa_kept_in_data_moves_to_the_addon_disk() {
        let left = GuestGapps {
            left_on_data: true,
            ..GuestGapps::default()
        };
        let none = GuestGapps::default();
        assert_eq!(
            upkeep(Some(&package()), &left, false, false),
            Upkeep::MoveImage
        );
        assert_eq!(
            upkeep(Some(&package()), &left, false, true),
            Upkeep::MoveImage
        );
        // Neither on the disk nor in /data: made again from the kept package, or installed
        // again by the user.
        assert_eq!(
            upkeep(Some(&package()), &none, false, true),
            Upkeep::Rebuild
        );
        assert_eq!(
            upkeep(Some(&package()), &none, false, false),
            Upkeep::Reinstall
        );
        assert!(reinstall_needed(
            Some(&package()),
            false,
            Some(&none),
            false
        ));
        assert!(!reinstall_needed(
            Some(&package()),
            false,
            Some(&left),
            false
        ));
        assert!(
            !reinstall_needed(Some(&package()), false, None, false),
            "Android not asked"
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsa-gapps-mod-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fr32_an_old_image_is_made_again_from_the_kept_package() {
        let dir = scratch("rebuild");
        let source = Source::new(Arc::new(crate::update::store::FileFetch), dir.join("work"))
            .with_package_dir(dir.join("kept"));
        let user = "uid:1000";
        let zip = package::fixture::sample();
        let mut digest = md5::Md5::new();
        digest.update(&zip);
        let old = GappsPackage {
            image_format: 0,
            md5: md5::hex(&digest.finish()),
            ..package()
        };
        assert!(!source.is_kept(user));
        let why = source.rebuild(user, &old).err().unwrap();
        assert!(why.contains("install Google Play again"), "{why}");

        let kept = source.kept_package(user).unwrap();
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, &zip).unwrap();
        assert!(source.is_kept(user));
        let rebuilt = source.rebuild(user, &old).unwrap();
        assert_eq!(rebuilt.package.image_format, IMAGE_FORMAT);
        assert_eq!(rebuilt.package.md5, old.md5);
        let image = std::fs::read(&rebuilt.image).unwrap();
        let files = crate::erofs::read::files(&image);
        assert!(files.contains_key("product/etc/permissions/dsa-gapps-privapp-permissions.xml"));
        let made = rebuilt.image.clone();
        drop(rebuilt);
        assert!(!made.exists(), "the image goes once it is sent");
        assert!(kept.is_file(), "the kept package stays");

        // A kept file that is not the one downloaded is not used.
        std::fs::write(&kept, b"another file").unwrap();
        let why = source.rebuild(user, &old).err().unwrap();
        assert!(why.contains("MD5"), "{why}");

        source.forget(user);
        assert!(!source.is_kept(user));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
