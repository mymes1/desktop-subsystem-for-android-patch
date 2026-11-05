//! The user's raw Android disk across image updates.

use std::path::{Path, PathBuf};

use dsa_host_macos::user_disk::{
    bring_disk_to, disk_image_version, ensure_pstore_disk, prepare_empty_disk, raw_disk,
};

const SECTOR: usize = 512;
const MIB: usize = 1 << 20;

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("user-disk")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A raw disk with a primary GPT naming `parts` (name, MiB), each filled with `fill`, one
/// MiB apart from the start.
fn gpt_disk(parts: &[(&str, usize, u8)]) -> Vec<u8> {
    let total: usize = 1 + parts.iter().map(|(_, mib, _)| mib).sum::<usize>();
    let mut disk = vec![0u8; total * MIB];
    let header = SECTOR;
    disk[header..header + 8].copy_from_slice(b"EFI PART");
    disk[header + 72..header + 80].copy_from_slice(&2u64.to_le_bytes());
    disk[header + 80..header + 84].copy_from_slice(&128u32.to_le_bytes());
    disk[header + 84..header + 88].copy_from_slice(&128u32.to_le_bytes());
    let mut at = MIB;
    for (i, (name, mib, fill)) in parts.iter().enumerate() {
        let entry = 2 * SECTOR + i * 128;
        disk[entry] = 1 + i as u8;
        let first = (at / SECTOR) as u64;
        let last = ((at + mib * MIB) / SECTOR - 1) as u64;
        disk[entry + 32..entry + 40].copy_from_slice(&first.to_le_bytes());
        disk[entry + 40..entry + 48].copy_from_slice(&last.to_le_bytes());
        for (j, unit) in name.encode_utf16().enumerate() {
            disk[entry + 56 + j * 2..entry + 58 + j * 2].copy_from_slice(&unit.to_le_bytes());
        }
        disk[at..at + mib * MIB].fill(*fill);
        at += mib * MIB;
    }
    disk
}

fn byte_of(disk: &Path, mib: usize) -> u8 {
    std::fs::read(disk).unwrap()[mib * MIB + 7]
}

#[test]
fn fr14_a_new_user_gets_a_copy_of_the_template_at_its_version() {
    let dir = folder("new");
    let template = dir.join("disk.img");
    std::fs::write(&template, gpt_disk(&[("super", 1, 0x11), ("userdata", 1, 0)])).unwrap();
    let disk = dir.join("user").join("android.img");
    bring_disk_to(&disk, &template, "16.0.0-1").unwrap();
    assert_eq!(std::fs::read(&disk).unwrap(), std::fs::read(&template).unwrap());
    assert_eq!(disk_image_version(&disk).as_deref(), Some("16.0.0-1"));
}

#[test]
fn fr14_a_newer_image_replaces_android_and_keeps_the_users_data() {
    let dir = folder("update");
    let old = dir.join("old").join("disk.img");
    let new = dir.join("new").join("disk.img");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::create_dir_all(new.parent().unwrap()).unwrap();
    std::fs::write(&old, gpt_disk(&[("super", 1, 0x11), ("userdata", 1, 0)])).unwrap();
    std::fs::write(&new, gpt_disk(&[("super", 1, 0x22), ("userdata", 1, 0)])).unwrap();
    let disk = dir.join("android.img");
    bring_disk_to(&disk, &old, "1").unwrap();
    // Android writes the user's data.
    let mut bytes = std::fs::read(&disk).unwrap();
    bytes[2 * MIB..3 * MIB].fill(0x99);
    std::fs::write(&disk, bytes).unwrap();
    bring_disk_to(&disk, &new, "2").unwrap();
    assert_eq!(byte_of(&disk, 1), 0x22, "Android comes from the new image");
    assert_eq!(byte_of(&disk, 2), 0x99, "the user's data stays");
    assert_eq!(disk_image_version(&disk).as_deref(), Some("2"));
}

#[test]
fn fr14_a_two_disk_image_takes_androids_old_partitions_off_the_users_disk() {
    let dir = folder("two");
    let single = dir.join("disk.img");
    let data = dir.join("data.img");
    std::fs::write(&single, gpt_disk(&[("super", 1, 0x11), ("userdata", 1, 0)])).unwrap();
    std::fs::write(&data, gpt_disk(&[("userdata", 1, 0)])).unwrap();
    let disk = dir.join("android.img");
    bring_disk_to(&disk, &single, "1").unwrap();
    bring_disk_to(&disk, &data, "2").unwrap();
    let names = gpt_names(&disk);
    assert_eq!(names, vec!["userdata".to_string()]);
}

fn gpt_names(disk: &Path) -> Vec<String> {
    let bytes = std::fs::read(disk).unwrap();
    (0..128)
        .map(|i| &bytes[2 * SECTOR + i * 128..2 * SECTOR + (i + 1) * 128])
        .filter(|entry| entry[0..16].iter().any(|&b| b != 0))
        .map(|entry| {
            let units: Vec<u16> = entry[56..128]
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&u| u != 0)
                .collect();
            String::from_utf16_lossy(&units)
        })
        .collect()
}

#[test]
fn fr14_an_empty_disk_is_replaced_by_the_template() {
    let dir = folder("empty");
    let disk = dir.join("android.img");
    prepare_empty_disk(&disk, 1).unwrap();
    assert_eq!(std::fs::metadata(&disk).unwrap().len(), 1 << 30);
    let template = dir.join("disk.img");
    std::fs::write(&template, gpt_disk(&[("super", 1, 0x11), ("userdata", 1, 0)])).unwrap();
    bring_disk_to(&disk, &template, "1").unwrap();
    assert_eq!(byte_of(&disk, 1), 0x11);
}

#[test]
fn fr2_a_raw_disk_is_used_as_it_is() {
    let dir = folder("raw");
    let raw = dir.join("system.img");
    std::fs::write(&raw, [0u8; 4096]).unwrap();
    assert_eq!(raw_disk(&raw, &dir.join("cache")).unwrap(), raw);
}

#[test]
fn fr21_the_pstore_disk_is_made_once_and_kept() {
    let dir = folder("pstore");
    let disk = dir.join("pstore.img");
    ensure_pstore_disk(&disk).unwrap();
    let bytes = std::fs::read(&disk).unwrap();
    assert_eq!(bytes.len(), 4 << 20);
    assert_eq!(&bytes[512..520], b"EFI PART");
    // A record the kernel wrote stays.
    let mut kept = bytes.clone();
    kept[2 << 20] = 7;
    std::fs::write(&disk, &kept).unwrap();
    ensure_pstore_disk(&disk).unwrap();
    assert_eq!(std::fs::read(&disk).unwrap()[2 << 20], 7);
}
