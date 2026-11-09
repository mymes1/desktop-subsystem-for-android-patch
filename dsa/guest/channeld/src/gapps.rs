//! The guest's half of the Google apps a user installs: where they stand in this boot, and the
//! image a DSA from before the add-on disk left on the user's disk.
//!
//! The host downloads the build of Google's apps the user asked for, makes a system image of
//! it, and puts the image on the user's add-on disk, a read-only disk only the host writes,
//! which the guest mounts at `/mnt/dsa-addons` early in each boot. An older host sent the image
//! to `/data/vendor/dsa/gapps/` instead. The host moves such an image onto the add-on disk: it
//! reads it here (`AddonImageRead`, answered with `AddonImagePart` by [`Gapps::read`]) and then
//! has it removed (`GappsRemove`). `GappsQuery` asks how things stand. Both are answered with
//! `GappsState`, which also says which Android the guest is (its API level and ABIs), so the
//! host fetches the build for this Android and no other.
//!
//! The image takes effect at the next boot: init lays it over `/product` and `/system_ext`
//! then (`dsa-gapps.rc`), when the host says one is installed. Whether the running boot has it
//! comes from the mount table. The platform supplies those facts, so this runs and is tested
//! off Android.

use std::path::PathBuf;

use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::GappsState;

pub use crate::image_store::IMAGE;
use crate::image_store::{ImageStore, Part};

/// Where an older host put the image on the user's disk.
pub const DIR: &str = "/data/vendor/dsa/gapps";
/// What the image is called in what the host shows the user.
const IMAGE_NAME: &str = "the Google apps image";
/// Where init mounts the image, as the mount table shows it once it is laid over the system.
pub const MOUNT_POINT: &str = "/mnt/dsa-gapps";

/// What the running boot is, which the platform reads from the system.
pub struct GuestFacts {
    /// Whether this boot has the image laid over the system.
    pub active: Box<dyn Fn() -> bool + Send + Sync>,
    /// `ro.build.version.sdk`.
    pub sdk: Box<dyn Fn() -> u32 + Send + Sync>,
    /// `ro.product.cpu.abi`.
    pub abi: Box<dyn Fn() -> String + Send + Sync>,
    /// `ro.product.cpu.abilist`.
    pub abilist: Box<dyn Fn() -> String + Send + Sync>,
}

/// Whether the mount table `mounts` (as `/proc/self/mounts` reads) has something mounted at
/// [`MOUNT_POINT`].
pub fn mounted(mounts: &str) -> bool {
    mounts
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(MOUNT_POINT))
}

impl GuestFacts {
    /// The facts of the running system: the mount table, and the properties `property` reads.
    pub fn of_system(property: impl Fn(&str) -> String + Send + Sync + 'static) -> GuestFacts {
        let property = std::sync::Arc::new(property);
        let (sdk, abi, abilist) = (property.clone(), property.clone(), property);
        GuestFacts {
            active: Box::new(|| {
                std::fs::read_to_string("/proc/self/mounts")
                    .map(|mounts| mounted(&mounts))
                    .unwrap_or(false)
            }),
            sdk: Box::new(move || (*sdk)("ro.build.version.sdk").trim().parse().unwrap_or(0)),
            abi: Box::new(move || (*abi)("ro.product.cpu.abi")),
            abilist: Box::new(move || (*abilist)("ro.product.cpu.abilist")),
        }
    }
}

/// The Google apps in this boot, and the image an older host left on the user's disk.
pub struct Gapps {
    store: ImageStore,
    facts: GuestFacts,
}

impl Gapps {
    /// Looks for an older host's image in `dir`.
    pub fn new(dir: impl Into<PathBuf>, facts: GuestFacts) -> Gapps {
        Gapps {
            store: ImageStore::new(dir, IMAGE_NAME),
            facts,
        }
    }

    /// Handles a message from the host meant for the Google apps, and returns the answer for
    /// the host. Any other message is not this module's: `None`.
    pub fn handle(&self, body: &Body) -> Option<GappsState> {
        let progress = match body {
            Body::GappsRemove(_) => self.store.remove(),
            Body::GappsQuery(_) => Default::default(),
            _ => return None,
        };
        Some(GappsState {
            done: progress.done,
            error: progress.error,
            installed: self.store.installed(),
            active: (self.facts.active)(),
            sdk: (self.facts.sdk)(),
            abi: (self.facts.abi)(),
            abilist: (self.facts.abilist)(),
        })
    }

    /// Up to `length` bytes of the image an older host left, from `offset`.
    pub fn read(&self, offset: u64, length: u32) -> Part {
        self.store.read(offset, length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr32_the_image_is_active_once_it_is_mounted() {
        let mounts = "/dev/block/dm-0 / erofs ro 0 0\n\
                      /mnt/dsa-addons/gapps.img /mnt/dsa-gapps erofs ro,nodev,nosuid 0 0\n\
                      overlay /product overlay ro,lowerdir=/mnt/dsa-gapps/product:/product 0 0\n";
        assert!(mounted(mounts));
        assert!(!mounted("/dev/block/dm-0 / erofs ro 0 0\n"));
        assert!(!mounted("/dev/loop1 /mnt/dsa-gapps-other erofs ro 0 0\n"));
    }
}
