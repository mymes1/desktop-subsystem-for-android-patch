//! An add-on image (an ARM translation package, the Google apps) that a DSA from before the
//! add-on disk sent to the user's disk, `/data`.
//!
//! The host now keeps each add-on's image on the add-on disk it attaches read only, and the
//! guest mounts it from there. An image an older host sent is still in `/data`, where it is
//! cached twice (by the encrypted file and by erofs) for as long as it is mounted. The host
//! reads it from here part by part ([`ImageStore::read`]), puts it on the add-on disk, and then
//! has it removed ([`ImageStore::remove`]), so nothing of it stays in `/data`.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The image's name in its folder.
pub const IMAGE: &str = "system.img";
/// The name an older host's transfer wrote under until it was complete and checked.
const PARTIAL: &str = "system.img.part";
/// The most one part holds.
pub const MAX_PART: u32 = 1 << 20;

/// How a removal went.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// The removal is done.
    pub done: bool,
    /// Why it failed, in words a person can act on; empty otherwise.
    pub error: String,
}

/// A part of the image, answering [`ImageStore::read`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Part {
    pub offset: u64,
    pub data: Vec<u8>,
    /// The whole image's size; 0 when there is none.
    pub size: u64,
    /// Why the image cannot be read; empty otherwise.
    pub error: String,
}

/// One image an older host left on the user's disk.
pub struct ImageStore {
    dir: PathBuf,
    /// What the image is called in what the host shows the user.
    name: &'static str,
}

impl ImageStore {
    /// The image called `name` in `dir`.
    pub fn new(dir: impl Into<PathBuf>, name: &'static str) -> ImageStore {
        ImageStore {
            dir: dir.into(),
            name,
        }
    }

    /// The folder the image is in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether an image is on the disk.
    pub fn installed(&self) -> bool {
        self.dir.join(IMAGE).is_file()
    }

    /// Up to `length` bytes of the image from `offset` (at most [`MAX_PART`]), with the
    /// image's size.
    pub fn read(&self, offset: u64, length: u32) -> Part {
        let name = self.name;
        let path = self.dir.join(IMAGE);
        let failed = |error: String| Part {
            offset,
            error,
            ..Part::default()
        };
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return failed(format!("{name} is not on the user's disk"))
            }
            Err(error) => return failed(format!("cannot read {name}: {error}")),
        };
        let size = match file.metadata() {
            Ok(metadata) => metadata.len(),
            Err(error) => return failed(format!("cannot read {name}: {error}")),
        };
        if offset > size {
            return Part {
                size,
                ..failed(format!("{name} has {size} bytes, none at {offset}"))
            };
        }
        let wanted = u64::from(length.min(MAX_PART)).min(size - offset);
        let mut data = vec![0u8; wanted as usize];
        let read = file
            .seek(SeekFrom::Start(offset))
            .and_then(|_| file.read_exact(&mut data));
        match read {
            Ok(()) => Part {
                offset,
                data,
                size,
                error: String::new(),
            },
            Err(error) => Part {
                size,
                ..failed(format!("cannot read {name}: {error}"))
            },
        }
    }

    /// Removes the image, and what an older host's broken-off transfer left beside it.
    pub fn remove(&self) -> Progress {
        let _ = fs::remove_file(self.dir.join(PARTIAL));
        match fs::remove_file(self.dir.join(IMAGE)) {
            Ok(()) => Progress {
                done: true,
                error: String::new(),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Progress {
                done: true,
                error: String::new(),
            },
            Err(error) => Progress {
                done: false,
                error: format!("cannot remove {}: {error}", self.name),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dsa-image-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fr32_an_image_left_on_data_is_read_in_parts_and_removed() {
        let dir = scratch("parts");
        let store = ImageStore::new(&dir, "the image");
        assert!(!store.installed());
        assert!(store.read(0, 16).error.contains("not on the user's disk"));
        let image: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
        fs::write(dir.join(IMAGE), &image).unwrap();
        fs::write(dir.join(PARTIAL), b"broken off").unwrap();
        assert!(store.installed());
        let mut got = Vec::new();
        while (got.len() as u64) < image.len() as u64 {
            let part = store.read(got.len() as u64, 4096);
            assert!(part.error.is_empty(), "{}", part.error);
            assert_eq!(part.size, image.len() as u64);
            assert_eq!(part.offset, got.len() as u64);
            assert!(!part.data.is_empty());
            got.extend(part.data);
        }
        assert_eq!(got, image);
        assert!(store.read(image.len() as u64, 4096).data.is_empty());
        assert!(!store.read(image.len() as u64 + 1, 4096).error.is_empty());
        assert_eq!(store.read(0, u32::MAX).data.len(), image.len());
        let removed = store.remove();
        assert!(removed.done, "{}", removed.error);
        assert!(!store.installed());
        assert!(!dir.join(PARTIAL).exists());
        assert!(store.remove().done, "removing nothing is done too");
        let _ = fs::remove_dir_all(&dir);
    }
}
