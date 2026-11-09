//! The guest's half of ARM app translation: where it stands in this boot, and the translation
//! package's image a DSA from before the add-on disk left on the user's disk.
//!
//! The host puts a package the user installs on the user's add-on disk, a read-only disk only
//! the host writes, which the guest mounts at `/mnt/dsa-addons` early in each boot. An older
//! host sent the image to `/data/vendor/dsa/arm-translation/` instead. The host moves such an
//! image onto the add-on disk: it reads it here (`AddonImageRead`, answered with
//! `AddonImagePart` by [`ArmTranslation::read`]) and then has it removed
//! (`ArmTranslationRemove`). `ArmTranslationQuery` asks how things stand. Both are answered with
//! `ArmTranslationState`.
//!
//! The image takes effect at the next boot: init mounts it and binds its parts onto empty
//! places of the system then (`dsa-arm-translation.rc`), when the host says one is installed.
//! What the running boot has comes from the system: whether the bridge library is in place and
//! which ABIs the package manager installs apps for. The platform supplies those, so this runs
//! and is tested off Android.

use std::path::{Path, PathBuf};

use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::ArmTranslationState;

pub use crate::image_store::IMAGE;
use crate::image_store::{ImageStore, Part};

/// Where an older host put the package on the user's disk.
pub const DIR: &str = "/data/vendor/dsa/arm-translation";
/// What the image is called in what the host shows the user.
const IMAGE_NAME: &str = "the ARM translation image";

/// The native bridge library the image's properties name, once init has bound the package's
/// libraries onto `/system_ext/lib64`, where the zygote finds the bridge by name. The image's
/// system_ext partition has no libraries of its own there, so it exists only then.
pub const BRIDGE_LIBRARY: &str = "/system_ext/lib64/libdsa_native_bridge.so";

/// What the running boot has, which the platform reads from the system.
pub struct BootFacts {
    /// Whether the native bridge library the image's properties name is in place.
    pub active: Box<dyn Fn() -> bool + Send + Sync>,
    /// `ro.product.cpu.abilist`.
    pub abilist: Box<dyn Fn() -> String + Send + Sync>,
}

impl BootFacts {
    /// The bridge is in place when [`BRIDGE_LIBRARY`] exists; `abilist` reads the property.
    pub fn of_system(abilist: impl Fn() -> String + Send + Sync + 'static) -> BootFacts {
        BootFacts {
            active: Box::new(|| Path::new(BRIDGE_LIBRARY).exists()),
            abilist: Box::new(abilist),
        }
    }
}

/// ARM translation in this boot, and the image an older host left on the user's disk.
pub struct ArmTranslation {
    store: ImageStore,
    facts: BootFacts,
}

impl ArmTranslation {
    /// Looks for an older host's package in `dir`.
    pub fn new(dir: impl Into<PathBuf>, facts: BootFacts) -> ArmTranslation {
        ArmTranslation {
            store: ImageStore::new(dir, IMAGE_NAME),
            facts,
        }
    }

    /// Handles a message from the host meant for ARM translation, and returns the answer for
    /// the host. Any other message is not this module's: `None`.
    pub fn handle(&self, body: &Body) -> Option<ArmTranslationState> {
        let progress = match body {
            Body::ArmTranslationRemove(_) => self.store.remove(),
            Body::ArmTranslationQuery(_) => Default::default(),
            _ => return None,
        };
        Some(ArmTranslationState {
            done: progress.done,
            error: progress.error,
            installed: self.store.installed(),
            active: (self.facts.active)(),
            abilist: (self.facts.abilist)(),
        })
    }

    /// Up to `length` bytes of the image an older host left, from `offset`.
    pub fn read(&self, offset: u64, length: u32) -> Part {
        self.store.read(offset, length)
    }
}
