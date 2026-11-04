//! The guest's half of ARM app translation: the translation package's system image on the
//! user's disk.
//!
//! The host sends the image (`ArmTranslationInstall` and its `ArmTranslationChunk`s). The
//! daemon writes it beside the one already there, and puts it in that one's place only once
//! all of it has arrived and its length and SHA-256 match what the host announced, so a
//! transfer that breaks off leaves the guest as it was. `ArmTranslationRemove` deletes it, and
//! `ArmTranslationQuery` asks how things stand. Every one of them is answered with
//! `ArmTranslationState`.
//!
//! The image takes effect at the next boot: init mounts it over the system then
//! (`dsa-arm-translation.rc`), when the host says one is installed. What the running boot has
//! comes from the system: whether the bridge library is in place and which ABIs the package
//! manager installs apps for. The platform supplies those, so this runs and is tested off
//! Android.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::ArmTranslationState;
use sha2::{Digest, Sha256};

/// Where the package goes on the user's disk.
pub const DIR: &str = "/data/vendor/dsa/arm-translation";
/// The image's name in [`DIR`].
pub const IMAGE: &str = "system.img";
/// The name it is written under until it is complete and checked.
const PARTIAL: &str = "system.img.part";
/// What the image is called in what the host shows the user.
const IMAGE_NAME: &str = "the ARM translation image";
/// The largest image taken: a translator, its ARM64 libraries and their proxies take a few
/// hundred MiB at most.
pub const MAX_IMAGE_BYTES: u64 = 2 << 30;

/// The native bridge library the image's properties name, once a package is laid over the
/// system.
pub const BRIDGE_LIBRARY: &str = "/system/lib64/libdsa_native_bridge.so";

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

struct Transfer {
    id: u32,
    size: u64,
    sha256: Vec<u8>,
    file: File,
    hasher: Sha256,
    received: u64,
}

/// The package on the user's disk and the transfer under way.
pub struct ArmTranslation {
    dir: PathBuf,
    facts: BootFacts,
    transfer: Mutex<Option<Transfer>>,
}

impl ArmTranslation {
    /// Keeps the package in `dir`, which init makes at boot.
    pub fn new(dir: impl Into<PathBuf>, facts: BootFacts) -> ArmTranslation {
        ArmTranslation {
            dir: dir.into(),
            facts,
            transfer: Mutex::new(None),
        }
    }

    /// Handles a message from the host meant for ARM translation, and returns the answer for
    /// the host. Any other message is not this module's: `None`.
    pub fn handle(&self, body: &Body) -> Option<ArmTranslationState> {
        match body {
            Body::ArmTranslationInstall(install) => {
                Some(self.begin(install.transfer_id, install.size, &install.sha256))
            }
            Body::ArmTranslationChunk(chunk) => {
                Some(self.write(chunk.transfer_id, chunk.offset, &chunk.data))
            }
            Body::ArmTranslationRemove(_) => Some(self.remove()),
            Body::ArmTranslationQuery(_) => Some(self.state(0, 0, false, String::new())),
            _ => None,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Transfer>> {
        self.transfer.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn state(
        &self,
        transfer_id: u32,
        received: u64,
        done: bool,
        error: String,
    ) -> ArmTranslationState {
        ArmTranslationState {
            transfer_id,
            received,
            done,
            error,
            installed: self.dir.join(IMAGE).is_file(),
            active: (self.facts.active)(),
            abilist: (self.facts.abilist)(),
        }
    }

    fn failed(&self, transfer_id: u32, received: u64, why: String) -> ArmTranslationState {
        let _ = fs::remove_file(self.dir.join(PARTIAL));
        self.state(transfer_id, received, false, why)
    }

    fn begin(&self, id: u32, size: u64, sha256: &[u8]) -> ArmTranslationState {
        let mut transfer = self.lock();
        // A transfer still under way has been given up by the host.
        *transfer = None;
        if size == 0 || size > MAX_IMAGE_BYTES {
            let why = format!(
                "{IMAGE_NAME} is {size} bytes; it must have from 1 byte to {} MiB",
                MAX_IMAGE_BYTES >> 20
            );
            return self.failed(id, 0, why);
        }
        if sha256.len() != 32 {
            let why = format!("{IMAGE_NAME} came without its SHA-256");
            return self.failed(id, 0, why);
        }
        if let Err(error) = fs::create_dir_all(&self.dir) {
            let why = format!("cannot make {}: {error}", self.dir.display());
            return self.failed(id, 0, why);
        }
        match File::create(self.dir.join(PARTIAL)) {
            Ok(file) => {
                *transfer = Some(Transfer {
                    id,
                    size,
                    sha256: sha256.to_vec(),
                    file,
                    hasher: Sha256::new(),
                    received: 0,
                });
                self.state(id, 0, false, String::new())
            }
            Err(error) => {
                let why = format!("cannot write {IMAGE_NAME} in {}: {error}", self.dir.display());
                self.failed(id, 0, why)
            }
        }
    }

    fn write(&self, id: u32, offset: u64, data: &[u8]) -> ArmTranslationState {
        let mut slot = self.lock();
        let Some(transfer) = slot.as_mut().filter(|t| t.id == id) else {
            let why = format!("no transfer {id} of {IMAGE_NAME} is under way");
            return self.state(id, 0, false, why);
        };
        let (received, size) = (transfer.received, transfer.size);
        let length = data.len() as u64;
        if offset != received || received + length > size {
            *slot = None;
            let why = format!(
                "a piece of {IMAGE_NAME} arrived out of place (at {offset}, {length} bytes, \
                 after {received} of {size})"
            );
            return self.failed(id, received, why);
        }
        if let Err(error) = transfer.file.write_all(data) {
            *slot = None;
            let why = format!("cannot write {IMAGE_NAME}: {error}");
            return self.failed(id, received, why);
        }
        transfer.hasher.update(data);
        transfer.received += length;
        let received = transfer.received;
        if received < size {
            return self.state(id, received, false, String::new());
        }
        let Some(Transfer {
            file,
            hasher,
            sha256,
            ..
        }) = slot.take()
        else {
            return self.state(id, received, false, String::new());
        };
        if let Err(error) = file.sync_all() {
            let why = format!("cannot save {IMAGE_NAME}: {error}");
            return self.failed(id, received, why);
        }
        drop(file);
        if hasher.finalize().as_slice() != sha256.as_slice() {
            let why = format!(
                "{IMAGE_NAME} did not arrive intact (its SHA-256 differs); install it again"
            );
            return self.failed(id, received, why);
        }
        if let Err(error) = fs::rename(self.dir.join(PARTIAL), self.dir.join(IMAGE)) {
            let why = format!("cannot put {IMAGE_NAME} in place: {error}");
            return self.failed(id, received, why);
        }
        self.state(id, received, true, String::new())
    }

    fn remove(&self) -> ArmTranslationState {
        *self.lock() = None;
        let _ = fs::remove_file(self.dir.join(PARTIAL));
        match fs::remove_file(self.dir.join(IMAGE)) {
            Ok(()) => self.state(0, 0, true, String::new()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.state(0, 0, true, String::new())
            }
            Err(error) => {
                let why = format!("cannot remove {IMAGE_NAME}: {error}");
                self.state(0, 0, false, why)
            }
        }
    }
}
