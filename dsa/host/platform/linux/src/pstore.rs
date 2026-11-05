//! The pstore disk: a small raw disk where the guest kernel keeps its last log.
//!
//! The VM ends when the guest panics or resets, and the next start is a new VM with empty
//! memory, so a log kept in RAM is gone by then. The guest kernel's pstore/blk writes its
//! console, an oops's report and Android's last log lines to this disk instead, and shows
//! them in `/sys/fs/pstore` at the next boot.
//!
//! It is the same disk as the Windows host's, as a raw file: one GPT partition with a fixed
//! partition UUID, which the kernel command line names (`qemu::PSTORE_CMDLINE`), so finding
//! it does not depend on the order the guest names its disks in. A file that is missing or
//! not this disk is written again from scratch.

use std::io::{self, Read};
use std::path::Path;

const SECTOR: u64 = 512;
/// The size of the disk: 4 MiB.
pub const DISK_BYTES: u64 = 4 << 20;
const SECTORS: u64 = DISK_BYTES / SECTOR;

/// The UUID of the disk's one partition, as the kernel looks it up.
pub const PARTITION_UUID: &str = "5f8b3c2e-9a41-4d6e-b07c-1e2d3f4a5b6c";
/// The partition's name: DSA's own, never one of Android's.
pub const PARTITION_NAME: &str = "dsa_pstore";
const DISK_GUID: &str = "2c7d9e1f-6b3a-4f58-8e0d-a9b8c7d6e5f4";
/// GPT's type for Linux file system data.
const LINUX_DATA_TYPE: &str = "0fc63daf-8483-4772-8e79-3d69d8477de4";

const ENTRIES: u64 = 128;
const ENTRY_BYTES: u64 = 128;
const ENTRY_SECTORS: u64 = ENTRIES * ENTRY_BYTES / SECTOR;
const FIRST_USABLE: u64 = 2 + ENTRY_SECTORS;
const LAST_USABLE: u64 = SECTORS - 2 - ENTRY_SECTORS;
/// The partition starts at 1 MiB, as partitioning tools align them.
const PARTITION_FIRST_SECTOR: u64 = 2048;

/// CRC-32 (IEEE), as GPT uses it.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A GUID's 16 bytes as GPT stores them: the first three fields little endian.
fn guid_bytes(text: &str) -> [u8; 16] {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    let mut raw = [0u8; 16];
    for (i, byte) in raw.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("a constant GUID");
    }
    let mut out = raw;
    out[0..4].copy_from_slice(&[raw[3], raw[2], raw[1], raw[0]]);
    out[4..6].copy_from_slice(&[raw[5], raw[4]]);
    out[6..8].copy_from_slice(&[raw[7], raw[6]]);
    out
}

fn gpt_header(current: u64, backup: u64, entries_lba: u64, entries_crc: u32) -> [u8; 512] {
    let mut h = [0u8; 512];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    h[12..16].copy_from_slice(&92u32.to_le_bytes());
    h[24..32].copy_from_slice(&current.to_le_bytes());
    h[32..40].copy_from_slice(&backup.to_le_bytes());
    h[40..48].copy_from_slice(&FIRST_USABLE.to_le_bytes());
    h[48..56].copy_from_slice(&LAST_USABLE.to_le_bytes());
    h[56..72].copy_from_slice(&guid_bytes(DISK_GUID));
    h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
    h[80..84].copy_from_slice(&(ENTRIES as u32).to_le_bytes());
    h[84..88].copy_from_slice(&(ENTRY_BYTES as u32).to_le_bytes());
    h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&h[..92]);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

/// The whole file of a new, empty pstore disk.
pub fn image() -> Vec<u8> {
    let mut disk = vec![0u8; DISK_BYTES as usize];
    let at = |lba: u64| (lba * SECTOR) as usize;

    // The protective MBR: one partition of type 0xEE over the whole disk.
    let mbr = &mut disk[0..512];
    mbr[446 + 1..446 + 4].copy_from_slice(&[0x00, 0x02, 0x00]);
    mbr[446 + 4] = 0xEE;
    mbr[446 + 5..446 + 8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    mbr[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
    mbr[446 + 12..446 + 16].copy_from_slice(&((SECTORS - 1) as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    let mut entries = vec![0u8; (ENTRIES * ENTRY_BYTES) as usize];
    entries[0..16].copy_from_slice(&guid_bytes(LINUX_DATA_TYPE));
    entries[16..32].copy_from_slice(&guid_bytes(PARTITION_UUID));
    entries[32..40].copy_from_slice(&PARTITION_FIRST_SECTOR.to_le_bytes());
    entries[40..48].copy_from_slice(&LAST_USABLE.to_le_bytes());
    for (i, unit) in PARTITION_NAME.encode_utf16().enumerate() {
        entries[56 + i * 2..58 + i * 2].copy_from_slice(&unit.to_le_bytes());
    }
    let entries_crc = crc32(&entries);
    let backup_entries = LAST_USABLE + 1;

    disk[at(1)..at(2)].copy_from_slice(&gpt_header(1, SECTORS - 1, 2, entries_crc));
    disk[at(2)..at(2) + entries.len()].copy_from_slice(&entries);
    disk[at(backup_entries)..at(backup_entries) + entries.len()].copy_from_slice(&entries);
    disk[at(SECTORS - 1)..at(SECTORS)].copy_from_slice(&gpt_header(
        SECTORS - 1,
        1,
        backup_entries,
        entries_crc,
    ));
    disk
}

/// Whether the file at `path` is a pstore disk this module made, whatever its partition holds.
fn is_pstore_disk(path: &Path) -> io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() != DISK_BYTES {
        return Ok(false);
    }
    let mut start = vec![0u8; (FIRST_USABLE * SECTOR) as usize];
    file.read_exact(&mut start)?;
    Ok(start[..] == image()[..start.len()])
}

/// Makes sure the pstore disk at `path` exists. One this module made is kept, with the records
/// in it; anything else at `path` is replaced by an empty disk.
pub fn ensure(path: &Path) -> Result<(), String> {
    match is_pstore_disk(path) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "the pstore disk {} could not be read: {e}",
                path.display()
            ))
        }
    }
    std::fs::write(path, image()).map_err(|e| {
        format!(
            "the pstore disk {} could not be written: {e}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr21_the_pstore_disk_has_one_partition_the_kernel_finds_by_its_uuid() {
        let disk = image();
        assert_eq!(disk.len() as u64, DISK_BYTES);
        assert_eq!(&disk[512..520], b"EFI PART");
        assert_eq!(&disk[(DISK_BYTES - 512) as usize..][..8], b"EFI PART");
        let entry = &disk[1024..1024 + 128];
        assert_eq!(&entry[16..32], &guid_bytes(PARTITION_UUID));
        assert!(crate::qemu::PSTORE_CMDLINE.contains(PARTITION_UUID));
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn fr21_an_existing_pstore_disk_keeps_its_records() {
        let dir = std::env::temp_dir().join(format!("dsa-pstore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pstore.img");
        std::fs::write(&path, b"not a disk").unwrap();
        ensure(&path).unwrap();
        let mut disk = std::fs::read(&path).unwrap();
        let record = (PARTITION_FIRST_SECTOR * SECTOR) as usize;
        disk[record] = 0x42;
        std::fs::write(&path, &disk).unwrap();
        ensure(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap()[record], 0x42);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
