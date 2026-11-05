//! The user's Android disk on macOS: a raw disk image in their own folder.
//!
//! It follows the Windows host's rules, on raw files instead of attached VHDX disks:
//! - a new disk is a copy of the image's data template (or of its single disk), which APFS
//!   clones without copying its blocks;
//! - an image of two disks keeps Android on its shared system disk, so the user's disk keeps
//!   only the template's partitions (misc, metadata, userdata);
//! - an image of one disk carries Android in it, so a newer image's Android partitions are
//!   copied into the user's disk and the user's data partitions stay;
//! - a disk that Android never wrote to holds nothing and is replaced by a fresh copy.
//!
//! The image version a disk holds Android from is recorded beside it.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::gpt;

/// The partitions of a single-disk image that hold Android itself. Every other partition
/// holds the user's data.
pub const ANDROID_PARTITIONS: [&str; 8] = [
    "super",
    "vbmeta_a",
    "vbmeta_system_a",
    "vbmeta_system_dlkm_a",
    "vbmeta_vendor_dlkm_a",
    "boot_a",
    "init_boot_a",
    "vendor_boot_a",
];

/// The record of the image version beside `disk`.
fn version_file(disk: &Path) -> PathBuf {
    disk.with_extension("image-version")
}

/// The image version the user's disk holds Android from, if known.
pub fn disk_image_version(disk: &Path) -> Option<String> {
    std::fs::read_to_string(version_file(disk))
        .ok()
        .map(|v| v.trim().to_string())
}

pub fn record_disk_image_version(disk: &Path, version: &str) -> Result<(), String> {
    std::fs::write(version_file(disk), version)
        .map_err(|e| format!("recording the image version of {}: {e}", disk.display()))
}

fn open(path: &Path, write: bool) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(write)
        .open(path)
        .map_err(|e| format!("opening {}: {e}", path.display()))
}

/// The folder a disk is in, created when missing.
fn folder_of(disk: &Path) -> Result<&Path, String> {
    let folder = disk
        .parent()
        .ok_or_else(|| format!("{} has no folder", disk.display()))?;
    std::fs::create_dir_all(folder).map_err(|e| format!("creating {}: {e}", folder.display()))?;
    Ok(folder)
}

/// Makes `disk` a copy of `template` through a temporary file, so a copy that is cut short
/// never looks like a disk.
fn copy_template(disk: &Path, template: &Path) -> Result<(), String> {
    folder_of(disk)?;
    let partial = disk.with_extension("img.new");
    std::fs::copy(template, &partial).map_err(|e| {
        format!(
            "copying {} to {}: {e}",
            template.display(),
            partial.display()
        )
    })?;
    std::fs::rename(&partial, disk).map_err(|e| format!("placing {}: {e}", disk.display()))
}

/// Makes sure `disk` exists: an existing one is kept, a new one is empty and may grow to
/// `size_gb`. A raw file of that size is a hole until it is written.
pub fn prepare_empty_disk(disk: &Path, size_gb: u32) -> Result<(), String> {
    if disk.exists() {
        return Ok(());
    }
    folder_of(disk)?;
    let file = File::create(disk).map_err(|e| format!("creating {}: {e}", disk.display()))?;
    file.set_len(u64::from(size_gb) << 30)
        .map_err(|e| format!("sizing {}: {e}", disk.display()))
}

/// Whether the disk's template is a data template, whose image keeps Android on a system
/// disk of its own.
fn is_data_template(template: &Path) -> bool {
    template
        .file_stem()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "data")
}

/// Brings the user's raw `disk` to image `version`, whose disk template (raw) is `template`.
pub fn bring_disk_to(disk: &Path, template: &Path, version: &str) -> Result<(), String> {
    let held = disk_image_version(disk);
    if disk.exists() && held.is_none() {
        let blank = gpt::is_blank(&mut open(disk, false)?)
            .map_err(|e| format!("reading {}: {e}", disk.display()))?;
        if blank {
            std::fs::remove_file(disk)
                .map_err(|e| format!("replacing the empty {}: {e}", disk.display()))?;
        }
    }
    if !disk.exists() {
        copy_template(disk, template)?;
        return record_disk_image_version(disk, version);
    }
    if is_data_template(template) {
        fit_to_data_template(disk, template, version)?;
        if held.as_deref() == Some(version) {
            return Ok(());
        }
        return record_disk_image_version(disk, version);
    }
    if held.as_deref() == Some(version) {
        return Ok(());
    }
    let mut source = open(template, false)?;
    let mut target = open(disk, true)?;
    let have =
        gpt::partitions(&mut source).map_err(|e| format!("reading {}: {e}", template.display()))?;
    let names: Vec<&str> = ANDROID_PARTITIONS
        .iter()
        .copied()
        .filter(|n| have.iter().any(|p| p.name == *n))
        .collect();
    let fits = gpt::fits(&mut source, &mut target, &names)
        .map_err(|e| format!("reading {}: {e}", disk.display()))?;
    if !fits {
        replace_if_unused(disk, template, version, &mut target)?;
        return record_disk_image_version(disk, version);
    }
    gpt::copy_partitions(&mut source, &mut target, &names)
        .map_err(|e| format!("writing Android {version} into {}: {e}", disk.display()))?;
    target
        .sync_all()
        .map_err(|e| format!("writing {}: {e}", disk.display()))?;
    record_disk_image_version(disk, version)
}

/// Replaces a disk whose layout the new image cannot use, if Android never wrote data to it.
fn replace_if_unused(
    disk: &Path,
    template: &Path,
    version: &str,
    target: &mut File,
) -> Result<(), String> {
    let unused = gpt::partition_is_blank(target, "userdata")
        .map_err(|e| format!("reading {}: {e}", disk.display()))?;
    if !unused {
        return Err(format!(
            "Android {version} needs a disk layout that {} does not have, and the disk \
             holds Android data, so it is left as it is",
            disk.display()
        ));
    }
    std::fs::remove_file(disk)
        .map_err(|e| format!("replacing the unused {}: {e}", disk.display()))?;
    copy_template(disk, template)
}

/// Gives the user's disk the partitions of the image's data template, and only those, so
/// Android does not find old copies of its own partitions on the user's disk.
fn fit_to_data_template(disk: &Path, template: &Path, version: &str) -> Result<(), String> {
    let wanted: Vec<String> = gpt::partitions(&mut open(template, false)?)
        .map_err(|e| format!("reading {}: {e}", template.display()))?
        .into_iter()
        .map(|p| p.name)
        .collect();
    let mut target = open(disk, true)?;
    let have =
        gpt::partitions(&mut target).map_err(|e| format!("reading {}: {e}", disk.display()))?;
    if wanted
        .iter()
        .any(|name| !have.iter().any(|p| &p.name == name))
    {
        return replace_if_unused(disk, template, version, &mut target);
    }
    if have.iter().all(|p| wanted.contains(&p.name)) {
        return Ok(());
    }
    let keep: Vec<&str> = wanted.iter().map(String::as_str).collect();
    gpt::keep_only(&mut target, &keep).map_err(|e| {
        format!(
            "removing the old Android partitions from {}: {e}",
            disk.display()
        )
    })?;
    Ok(())
}

/// The raw image of the disk `image_disk` (a VHDX of an image, or already raw), converting
/// it into `cache` the first time. The conversion goes through a temporary file, so a cut
/// short one is never taken for the disk.
pub fn raw_disk(image_disk: &Path, cache: &Path) -> Result<PathBuf, String> {
    let is_vhdx = crate::vhdx::is_vhdx(image_disk)
        .map_err(|e| format!("reading {}: {e}", image_disk.display()))?;
    if !is_vhdx {
        return Ok(image_disk.to_path_buf());
    }
    let stem = image_disk
        .file_stem()
        .ok_or_else(|| format!("{} is not a file", image_disk.display()))?;
    let raw = cache.join(stem).with_extension("img");
    if raw.exists() {
        return Ok(raw);
    }
    std::fs::create_dir_all(cache).map_err(|e| format!("creating {}: {e}", cache.display()))?;
    let partial = raw.with_extension("img.new");
    crate::vhdx::convert_to_raw(image_disk, &partial).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("converting {} to a raw disk: {e}", image_disk.display())
    })?;
    std::fs::rename(&partial, &raw).map_err(|e| format!("placing {}: {e}", raw.display()))?;
    Ok(raw)
}

/// Makes sure the raw pstore disk at `path` exists: the pstore disk the Windows host makes,
/// without its VHD footer. One already there is kept with its records; anything else at
/// `path` (of another size) is replaced.
pub fn ensure_pstore_disk(path: &Path) -> Result<(), String> {
    let size = crate::pstore_disk::DISK_BYTES;
    if std::fs::metadata(path).is_ok_and(|m| m.len() == size) {
        return Ok(());
    }
    folder_of(path)?;
    let image = crate::pstore_disk::image();
    std::fs::write(path, &image[..size as usize])
        .map_err(|e| format!("writing the pstore disk {}: {e}", path.display()))
}
