//! Reading the image's VHDX disks into raw disk images.
//!
//! The image ships its disks as dynamic VHDX files, which Hyper-V attaches. Apple's
//! Virtualization framework attaches raw images, so the macOS host converts each disk once.
//! A dynamic VHDX holds only the blocks that were written; the raw file gets those blocks and
//! nothing else, and the rest stays a hole, so on APFS the raw disk takes about the space the
//! VHDX did.
//!
//! The reader follows the VHDX format: the file identifier, the two headers (the one with
//! the higher sequence number wins), the region table that locates the block allocation
//! table and the metadata, the metadata items that give the block size, the logical sector
//! size and the virtual size, and the allocation table's payload entries. A file with a log
//! to replay, or one that is a differencing disk, is refused: the image makes neither.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

const FILE_SIGNATURE: &[u8; 8] = b"vhdxfile";
const HEADER_OFFSETS: [u64; 2] = [64 * KIB, 128 * KIB];
const REGION_TABLE_OFFSETS: [u64; 2] = [192 * KIB, 256 * KIB];

/// The region table's entries for the block allocation table and the metadata.
pub const BAT_REGION: &str = "2DC27766-F623-4200-9D64-115E9BFD4A08";
pub const METADATA_REGION: &str = "8B7CA206-4790-4B9A-B8FE-575F050F886E";
/// The metadata items the reader needs.
pub const FILE_PARAMETERS: &str = "CAA16737-FA36-4D43-B3B6-33F0AA44E76B";
pub const VIRTUAL_DISK_SIZE: &str = "2FA54224-CD1B-4876-B211-5DBED83BF4B8";
pub const LOGICAL_SECTOR_SIZE: &str = "8141BF1D-A96F-4709-BA47-F233A8FAAB5F";

/// A payload block whose data is in the file.
const PAYLOAD_FULLY_PRESENT: u64 = 6;
/// A payload block of a differencing disk, partly in its parent.
const PAYLOAD_PARTIALLY_PRESENT: u64 = 7;

/// How much of a block is written at a time; a run of zeroes this long stays a hole.
const CHUNK: usize = MIB as usize;

/// A GUID's 16 bytes as VHDX stores them: the first three fields little endian.
pub fn guid_bytes(text: &str) -> [u8; 16] {
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

fn invalid(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.into())
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn read_at(file: &mut (impl Read + Seek), at: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    file.seek(SeekFrom::Start(at))?;
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// What the reader learned of a VHDX: its geometry and where each block's data is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub virtual_size: u64,
    pub block_size: u64,
    pub logical_sector_size: u64,
    /// The file offset of each payload block's data, or `None` for a block that reads as
    /// zeroes.
    pub blocks: Vec<Option<u64>>,
}

/// Whether the file at `path` is a VHDX: it starts with the file identifier.
pub fn is_vhdx(path: &Path) -> io::Result<bool> {
    let mut signature = [0u8; 8];
    let mut file = File::open(path)?;
    Ok(file.read_exact(&mut signature).is_ok() && &signature == FILE_SIGNATURE)
}

/// Reads the layout of the VHDX in `file`.
pub fn layout(file: &mut (impl Read + Seek)) -> io::Result<Layout> {
    if read_at(file, 0, 8)? != FILE_SIGNATURE {
        return Err(invalid("the file is not a VHDX (no vhdxfile identifier)"));
    }
    // The current header is the valid one with the higher sequence number.
    let mut current: Option<(u64, Vec<u8>)> = None;
    for at in HEADER_OFFSETS {
        let header = read_at(file, at, 4096)?;
        if &header[0..4] != b"head" {
            continue;
        }
        let sequence = u64_at(&header, 8);
        if current.as_ref().is_none_or(|(seen, _)| sequence > *seen) {
            current = Some((sequence, header));
        }
    }
    let (_, header) = current.ok_or_else(|| invalid("the VHDX has no valid header"))?;
    if header[48..64].iter().any(|&b| b != 0) {
        return Err(invalid(
            "the VHDX has a log to replay (it was not closed cleanly); open it once in \
             Hyper-V or qemu-img to finish it",
        ));
    }
    let table = REGION_TABLE_OFFSETS
        .iter()
        .map(|&at| read_at(file, at, 64 * KIB as usize))
        .find(|table| table.as_ref().is_ok_and(|t| &t[0..4] == b"regi"))
        .ok_or_else(|| invalid("the VHDX has no region table"))??;
    let count = u32_at(&table, 8) as usize;
    if count > 2047 {
        return Err(invalid("the VHDX region table is not plausible"));
    }
    let (mut bat, mut metadata) = (None, None);
    for i in 0..count {
        let entry = &table[16 + i * 32..16 + (i + 1) * 32];
        let region = (u64_at(entry, 16), u32_at(entry, 24) as u64);
        if entry[0..16] == guid_bytes(BAT_REGION) {
            bat = Some(region);
        } else if entry[0..16] == guid_bytes(METADATA_REGION) {
            metadata = Some(region);
        }
    }
    let (bat_at, bat_len) = bat.ok_or_else(|| invalid("the VHDX has no allocation table"))?;
    let (meta_at, meta_len) = metadata.ok_or_else(|| invalid("the VHDX has no metadata"))?;
    if meta_len > 64 * MIB || bat_len > 1024 * MIB {
        return Err(invalid("the VHDX regions are not plausible"));
    }
    let meta = read_at(file, meta_at, meta_len as usize)?;
    if &meta[0..8] != b"metadata" {
        return Err(invalid("the VHDX metadata table has no signature"));
    }
    let item = |id: &str| -> io::Result<&[u8]> {
        let entries = u16_at(&meta, 10) as usize;
        for i in 0..entries {
            let entry = &meta[32 + i * 32..32 + (i + 1) * 32];
            if entry[0..16] == guid_bytes(id) {
                let (at, len) = (u32_at(entry, 16) as usize, u32_at(entry, 20) as usize);
                return meta
                    .get(at..at + len)
                    .ok_or_else(|| invalid("a VHDX metadata item lies outside its region"));
            }
        }
        Err(invalid(format!("the VHDX lacks the metadata item {id}")))
    };
    let parameters = item(FILE_PARAMETERS)?;
    let block_size = u64::from(u32_at(parameters, 0));
    if u32_at(parameters, 4) & 2 != 0 {
        return Err(invalid(
            "the VHDX is a differencing disk, which needs its parent",
        ));
    }
    let virtual_size = u64_at(item(VIRTUAL_DISK_SIZE)?, 0);
    let logical_sector_size = u64::from(u32_at(item(LOGICAL_SECTOR_SIZE)?, 0));
    if !(MIB..=256 * MIB).contains(&block_size)
        || !block_size.is_power_of_two()
        || ![512, 4096].contains(&logical_sector_size)
    {
        return Err(invalid("the VHDX block or sector size is not plausible"));
    }
    // One sector bitmap entry follows each chunk of payload entries.
    let chunk_ratio = ((1u64 << 23) * logical_sector_size) / block_size;
    let block_count = virtual_size.div_ceil(block_size);
    let table = read_at(file, bat_at, bat_len as usize)?;
    let mut blocks = Vec::with_capacity(block_count as usize);
    for block in 0..block_count {
        let index = (block + block / chunk_ratio) as usize;
        let entry = table
            .get(index * 8..index * 8 + 8)
            .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
            .ok_or_else(|| invalid("the VHDX allocation table is too short"))?;
        let state = entry & 7;
        if state == PAYLOAD_PARTIALLY_PRESENT {
            return Err(invalid(
                "the VHDX has a block that is partly in a parent disk",
            ));
        }
        blocks.push((state == PAYLOAD_FULLY_PRESENT).then_some((entry >> 20) * MIB));
    }
    Ok(Layout {
        virtual_size,
        block_size,
        logical_sector_size,
        blocks,
    })
}

/// Writes the disk the VHDX at `from` holds to `to` as a raw image of its virtual size,
/// writing only the data that is not zero.
pub fn convert_to_raw(from: &Path, to: &Path) -> io::Result<()> {
    let mut source = File::open(from)?;
    let layout = layout(&mut source)?;
    let mut target = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(to)?;
    target.set_len(layout.virtual_size)?;
    let mut buf = vec![0u8; CHUNK];
    for (block, offset) in layout.blocks.iter().enumerate() {
        let Some(offset) = offset else { continue };
        let start = block as u64 * layout.block_size;
        let end = (start + layout.block_size).min(layout.virtual_size);
        let mut at = start;
        while at < end {
            let len = ((end - at) as usize).min(CHUNK);
            source.seek(SeekFrom::Start(offset + (at - start)))?;
            source.read_exact(&mut buf[..len])?;
            if buf[..len].iter().any(|&b| b != 0) {
                target.seek(SeekFrom::Start(at))?;
                target.write_all(&buf[..len])?;
            }
            at += len as u64;
        }
    }
    target.sync_all()
}
