//! The image's VHDX disks read into raw disks the Virtualization framework attaches.

use std::path::PathBuf;

use dsa_host_macos::vhdx::{
    convert_to_raw, guid_bytes, is_vhdx, layout, BAT_REGION, FILE_PARAMETERS, LOGICAL_SECTOR_SIZE,
    METADATA_REGION, VIRTUAL_DISK_SIZE,
};

const MIB: usize = 1 << 20;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("vhdx");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}-{name}", std::process::id()))
}

fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

/// A dynamic VHDX of three 1 MiB blocks: the first and last hold data, the middle one was
/// never written.
fn small_vhdx(log: bool) -> Vec<u8> {
    let mut file = vec![0u8; 5 * MIB];
    put(&mut file, 0, b"vhdxfile");
    let header = 64 * 1024;
    put(&mut file, header, b"head");
    put(&mut file, header + 8, &1u64.to_le_bytes());
    if log {
        put(&mut file, header + 48, &[1u8; 16]);
    }
    let regions = 192 * 1024;
    put(&mut file, regions, b"regi");
    put(&mut file, regions + 8, &2u32.to_le_bytes());
    for (i, (id, at)) in [(BAT_REGION, MIB), (METADATA_REGION, 2 * MIB)]
        .into_iter()
        .enumerate()
    {
        let entry = regions + 16 + i * 32;
        put(&mut file, entry, &guid_bytes(id));
        put(&mut file, entry + 16, &(at as u64).to_le_bytes());
        put(&mut file, entry + 24, &(MIB as u32).to_le_bytes());
    }
    let meta = 2 * MIB;
    put(&mut file, meta, b"metadata");
    put(&mut file, meta + 10, &3u16.to_le_bytes());
    let items: [(&str, Vec<u8>); 3] = [
        (
            FILE_PARAMETERS,
            [(MIB as u32).to_le_bytes(), 0u32.to_le_bytes()].concat(),
        ),
        (VIRTUAL_DISK_SIZE, (3 * MIB as u64).to_le_bytes().to_vec()),
        (LOGICAL_SECTOR_SIZE, 512u32.to_le_bytes().to_vec()),
    ];
    for (i, (id, value)) in items.iter().enumerate() {
        let entry = meta + 32 + i * 32;
        let offset = 64 * 1024 + i * 64;
        put(&mut file, entry, &guid_bytes(id));
        put(&mut file, entry + 16, &(offset as u32).to_le_bytes());
        put(&mut file, entry + 20, &(value.len() as u32).to_le_bytes());
        put(&mut file, meta + offset, value);
    }
    let bat = MIB;
    put(&mut file, bat, &(6u64 | (3u64 << 20)).to_le_bytes());
    put(&mut file, bat + 16, &(6u64 | (4u64 << 20)).to_le_bytes());
    file[3 * MIB..4 * MIB].fill(0xa5);
    file[4 * MIB + 100] = 0x5a;
    file
}

#[test]
fn fr2_a_vhdx_disk_becomes_a_raw_disk_with_the_same_contents() {
    let vhdx = scratch("disk.vhdx");
    std::fs::write(&vhdx, small_vhdx(false)).unwrap();
    assert!(is_vhdx(&vhdx).unwrap());
    let raw = scratch("disk.img");
    convert_to_raw(&vhdx, &raw).unwrap();
    let disk = std::fs::read(&raw).unwrap();
    assert_eq!(disk.len(), 3 * MIB);
    assert!(disk[..MIB].iter().all(|&b| b == 0xa5));
    assert!(disk[MIB..2 * MIB].iter().all(|&b| b == 0));
    assert_eq!(disk[2 * MIB + 100], 0x5a);
    assert_eq!(disk[2 * MIB..].iter().filter(|&&b| b != 0).count(), 1);
    assert!(!is_vhdx(&raw).unwrap());
}

#[test]
fn fr2_the_layout_names_only_the_blocks_that_hold_data() {
    let file = small_vhdx(false);
    let found = layout(&mut std::io::Cursor::new(file)).unwrap();
    assert_eq!(found.virtual_size, 3 * MIB as u64);
    assert_eq!(found.block_size, MIB as u64);
    assert_eq!(found.logical_sector_size, 512);
    assert_eq!(
        found.blocks,
        vec![Some(3 * MIB as u64), None, Some(4 * MIB as u64)]
    );
}

#[test]
fn nfr5_a_vhdx_with_a_log_to_replay_or_no_signature_is_refused() {
    let with_log = small_vhdx(true);
    assert!(layout(&mut std::io::Cursor::new(with_log)).is_err());
    let mut unsigned = small_vhdx(false);
    unsigned[0] = b'X';
    assert!(layout(&mut std::io::Cursor::new(unsigned)).is_err());
    let truncated = small_vhdx(false)[..300 * 1024].to_vec();
    assert!(layout(&mut std::io::Cursor::new(truncated)).is_err());
}
