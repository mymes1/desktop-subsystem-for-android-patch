//! What a VM boots from an image folder, and each user's Android disk.
//!
//! An image folder holds what the image build publishes for every host: `image.json`,
//! `initrd.img`, the kernel (`bzImage`, or `Image` for arm64), `system.vhdx` and `data.vhdx`,
//! the template of a user's disk (an older image has one `disk.vhdx` for both). QEMU reads VHDX
//! itself, so the published image boots as it is, and its checksums in `image.json` stay
//! true. The Linux package may also carry raw copies made with
//! `qemu-img convert -f vhdx -O raw system.vhdx system.img` (sparse, faster to read); a raw or
//! qcow2 file beside the VHDX wins over it.
//!
//! Each user's disk is a qcow2 file (`data.qcow2`), made from the template with
//! `qemu-img convert`: sparse, and the guest's discards shrink it. The image version it was
//! made from or brought to is recorded beside it (`data.version`).

use std::path::{Path, PathBuf};

/// What to boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFiles {
    pub kernel: PathBuf,
    pub initrd: Option<PathBuf>,
    pub kernel_cmdline: String,
    /// The read-only system disk; `None` for an image with one disk.
    pub system_image: Option<PathBuf>,
    /// The template of a new user's disk.
    pub disk_template: Option<PathBuf>,
}

/// The disk file named `stem`, preferring a raw copy, then qcow2, then the published VHDX.
pub fn disk_file(dir: &Path, stem: &str) -> Option<PathBuf> {
    ["img", "raw", "qcow2", "vhdx"]
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|path| path.exists())
}

/// What to boot from the image folder `dir`, with `extra_cmdline` after the image's own
/// kernel command line, and `kernel` when the image carries none of its own.
pub fn image_files(
    dir: &Path,
    kernel: Option<&Path>,
    extra_cmdline: &str,
) -> Result<ImageFiles, String> {
    let kernel = ["bzImage", "Image"]
        .iter()
        .map(|name| dir.join(name))
        .find(|own| own.exists())
        .or_else(|| kernel.map(Path::to_path_buf))
        .ok_or_else(|| format!("the image in {} has no kernel", dir.display()))?;
    let manifest = std::fs::read_to_string(dir.join("image.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let image_cmdline = manifest
        .as_ref()
        .and_then(|m| m["kernel_cmdline"].as_str())
        .unwrap_or_default();
    let kernel_cmdline = format!("{image_cmdline} {extra_cmdline}")
        .trim()
        .to_string();
    let initrd = Some(dir.join("initrd.img")).filter(|p| p.exists());
    let (system_image, disk_template) = match disk_file(dir, "system") {
        Some(system) => (Some(system), disk_file(dir, "data")),
        None => (None, disk_file(dir, "disk")),
    };
    if system_image.is_none() && disk_template.is_none() {
        return Err(format!(
            "the image in {} has neither system.vhdx nor disk.vhdx",
            dir.display()
        ));
    }
    Ok(ImageFiles {
        kernel,
        initrd,
        kernel_cmdline,
        system_image,
        disk_template,
    })
}

/// The file beside a user's disk that records the image version it holds.
pub fn version_file(disk: &Path) -> PathBuf {
    disk.with_extension("version")
}

/// The image version the user's disk holds, if recorded.
pub fn disk_version(disk: &Path) -> Option<String> {
    std::fs::read_to_string(version_file(disk))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

#[cfg(target_os = "linux")]
pub use making::{bring_disk_to, make_disk};

#[cfg(target_os = "linux")]
mod making {
    use super::*;
    use crate::qemu::DiskFormat;
    use crate::users::Account;
    use std::process::{Command, Stdio};

    fn qemu_img(account: &Account, args: &[&str]) -> Result<(), String> {
        let program = crate::network::find_program("qemu-img")
            .ok_or("qemu-img is not installed; install QEMU's tools (the qemu-utils package)")?;
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        account.run_as(&mut command, None);
        let out = command
            .output()
            .map_err(|e| format!("running qemu-img: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "qemu-img {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// Makes a new disk at `disk` for `account`: a copy of `template`, or without one an empty
    /// disk that may grow to `size_gb`.
    pub fn make_disk(
        account: &Account,
        disk: &Path,
        template: Option<&Path>,
        size_gb: u32,
    ) -> Result<(), String> {
        if let Some(folder) = disk.parent() {
            account.make_dir(folder)?;
        }
        let target = disk.to_string_lossy();
        match template {
            Some(template) => {
                let from = template.to_string_lossy();
                qemu_img(
                    account,
                    &[
                        "convert",
                        "-q",
                        "-f",
                        DiskFormat::of(template).name(),
                        "-O",
                        "qcow2",
                        &from,
                        &target,
                    ],
                )
            }
            None => qemu_img(
                account,
                &[
                    "create",
                    "-q",
                    "-f",
                    "qcow2",
                    &target,
                    &format!("{size_gb}G"),
                ],
            ),
        }
    }

    /// Makes sure `account`'s disk holds the image `version`: a missing disk is made from
    /// `template`, and the version is recorded.
    ///
    /// The images made by the image build keep Android on the separate system disk, so a
    /// user's disk holds only their data and needs nothing copied when the image changes. A
    /// one-disk image (`disk.vhdx`) keeps Android on the user's disk; such a disk made at an
    /// older version is refused rather than booted with mismatched partitions.
    pub fn bring_disk_to(
        account: &Account,
        disk: &Path,
        template: &Path,
        two_disks: bool,
        version: &str,
    ) -> Result<(), String> {
        if !disk.exists() {
            make_disk(account, disk, Some(template), 0)?;
        } else if !two_disks {
            if let Some(held) = disk_version(disk).filter(|held| held != version) {
                return Err(format!(
                    "{} holds Android {held}, and the image {version} keeps Android on the \
                     same disk; reset Android's data (dsa reset) to start from {version}",
                    disk.display()
                ));
            }
        }
        account.write_private(&version_file(disk), version.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsa-image-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for file in files {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        dir
    }

    #[test]
    fn fr14_a_published_image_boots_its_own_kernel_and_disks() {
        let dir = folder(
            "two",
            &["bzImage", "initrd.img", "system.vhdx", "data.vhdx"],
        );
        std::fs::write(
            dir.join("image.json"),
            r#"{"kernel_cmdline": "console=ttyS0 androidboot.boot_devices=vmbus"}"#,
        )
        .unwrap();
        let files = image_files(&dir, None, "androidboot.dsa.x=1").unwrap();
        assert_eq!(files.kernel, dir.join("bzImage"));
        assert_eq!(files.initrd, Some(dir.join("initrd.img")));
        assert_eq!(files.system_image, Some(dir.join("system.vhdx")));
        assert_eq!(files.disk_template, Some(dir.join("data.vhdx")));
        assert_eq!(
            files.kernel_cmdline,
            "console=ttyS0 androidboot.boot_devices=vmbus androidboot.dsa.x=1"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fr14_a_raw_copy_the_package_made_wins_over_the_vhdx() {
        let dir = folder("raw", &["Image", "system.vhdx", "system.img", "data.vhdx"]);
        let files = image_files(&dir, None, "").unwrap();
        assert_eq!(files.kernel, dir.join("Image"));
        assert_eq!(files.system_image, Some(dir.join("system.img")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fr14_a_one_disk_image_is_the_template_of_the_users_disk() {
        let dir = folder("one", &["disk.vhdx"]);
        let files = image_files(&dir, Some(Path::new("/opt/dsa/bzImage")), "").unwrap();
        assert_eq!(files.kernel, PathBuf::from("/opt/dsa/bzImage"));
        assert_eq!(files.system_image, None);
        assert_eq!(files.disk_template, Some(dir.join("disk.vhdx")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fr14_an_image_without_disks_is_refused() {
        let dir = folder("empty", &["bzImage"]);
        assert!(image_files(&dir, None, "").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fr19_the_disks_version_is_kept_beside_it() {
        assert_eq!(
            version_file(Path::new("/h/.local/share/dsa/data.qcow2")),
            PathBuf::from("/h/.local/share/dsa/data.version")
        );
    }
}
