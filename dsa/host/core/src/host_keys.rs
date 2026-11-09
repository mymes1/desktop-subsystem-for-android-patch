//! The secrets the host keeps for each user in the host's own credential store: the key their
//! backup folder is encrypted with, and the root key of their Android's KeyMint.
//!
//! Each secret is 32 random bytes, made the first time it is needed and kept by a
//! [`HostKeyStore`] the platform supplies: on Windows sealed by DPAPI under the user's own
//! account (and, for the KeyMint root key, by the computer's TPM first), on macOS in the user's
//! keychain, on Linux in the user's Secret Service collection. None of them is ever written to
//! the guest's disk, so a copy of that disk alone holds no key.
//!
//! [`SealedFileStore`] is the part the platforms that seal (rather than store) share: the
//! sealed bytes live in a file of the user's DSA folder, and only the [`Sealer`] can open them.
//! A sealed file copied to another user or another computer does not open there.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How long each secret is, in bytes.
pub const SECRET_LEN: usize = 32;

/// What a secret is for. Each purpose has a secret of its own, so a leak of one says nothing of
/// another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyPurpose {
    /// The key the user's backup folder is encrypted with.
    Backup,
    /// The root key the guest's KeyMint wraps every key it makes with.
    KeyMintRoot,
}

impl KeyPurpose {
    /// The secret's name in the credential store and in file names: stable, ASCII.
    pub fn name(self) -> &'static str {
        match self {
            KeyPurpose::Backup => "backup-key",
            KeyPurpose::KeyMintRoot => "keymint-root-key",
        }
    }

    /// What a person reads when the credential store asks about the secret.
    pub fn description(self) -> &'static str {
        match self {
            KeyPurpose::Backup => "Desktop Subsystem for Android backup key",
            KeyPurpose::KeyMintRoot => "Desktop Subsystem for Android KeyMint root key",
        }
    }
}

/// 32 secret bytes, wiped from memory when dropped. `Debug` never shows them.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret([u8; SECRET_LEN]);

impl Secret {
    pub fn from_bytes(bytes: [u8; SECRET_LEN]) -> Secret {
        Secret(bytes)
    }

    /// A secret from a slice of exactly [`SECRET_LEN`] bytes.
    pub fn from_slice(bytes: &[u8]) -> Option<Secret> {
        <[u8; SECRET_LEN]>::try_from(bytes).ok().map(Secret)
    }

    /// A fresh secret from the operating system's random source.
    pub fn generate() -> Result<Secret, String> {
        let mut bytes = [0u8; SECRET_LEN];
        getrandom::getrandom(&mut bytes)
            .map_err(|e| format!("the system's random source failed: {e}"))?;
        Ok(Secret(bytes))
    }

    pub fn bytes(&self) -> &[u8; SECRET_LEN] {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        for byte in self.0.iter_mut() {
            // A volatile write, so the wipe is not optimised away.
            // SAFETY: a valid, aligned pointer to a byte this value owns.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// Why a secret could not be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The host has no credential store to keep it in, or the user turned it off. The text
    /// says which, and what to do.
    Unavailable(String),
    /// The store has the secret but cannot open it here: it was sealed for another user or
    /// another computer, or the store refused. The text says which.
    Unreadable(String),
    /// The store failed; the text says how.
    Failed(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::Unavailable(why) | KeyError::Unreadable(why) | KeyError::Failed(why) => {
                f.write_str(why)
            }
        }
    }
}

/// Keeps the secrets of one user in the host's credential store.
pub trait HostKeyStore: Send + Sync {
    /// The secret kept for `purpose`; `Ok(None)` when none is kept yet.
    fn load(&self, purpose: KeyPurpose) -> Result<Option<Secret>, KeyError>;
    /// Keeps `secret` for `purpose`, in place of what was kept.
    fn store(&self, purpose: KeyPurpose, secret: &Secret) -> Result<(), KeyError>;
}

/// The secret kept for `purpose`, made and kept first when there is none yet. A secret that is
/// kept but cannot be opened is an error and is never replaced: a new one would make
/// everything the old one protects unreadable for good.
pub fn load_or_create(store: &dyn HostKeyStore, purpose: KeyPurpose) -> Result<Secret, KeyError> {
    if let Some(secret) = store.load(purpose)? {
        return Ok(secret);
    }
    let secret = Secret::generate().map_err(KeyError::Failed)?;
    store.store(purpose, &secret)?;
    // Read back, so a store that keeps what it cannot give back fails now, not later.
    match store.load(purpose)? {
        Some(kept) if kept == secret => Ok(secret),
        _ => Err(KeyError::Failed(format!(
            "the credential store did not keep the {}",
            purpose.description()
        ))),
    }
}

/// Seals bytes so that only this host's user can open them again: DPAPI under the user's
/// account, or the computer's TPM. `label` names what is sealed and is bound into the seal
/// where the sealer can, so a sealed secret of one purpose does not open as another.
pub trait Sealer: Send + Sync {
    /// What seals, for the file and the log: `dpapi`, `tpm+dpapi`.
    fn name(&self) -> &str;
    fn seal(&self, plain: &[u8], label: &str) -> Result<Vec<u8>, KeyError>;
    fn unseal(&self, sealed: &[u8], label: &str) -> Result<Vec<u8>, KeyError>;
}

/// What a sealed file starts with, before the sealer's name.
const SEALED_MAGIC: &[u8] = b"DSA sealed key 1\n";

/// A [`HostKeyStore`] that keeps each secret sealed by a [`Sealer`] in a file of `dir`,
/// `<purpose>.sealed`. The file names the sealer, so a file sealed by another one (a TPM that
/// was cleared, a computer without one) is refused with that reason rather than read wrongly.
pub struct SealedFileStore {
    dir: PathBuf,
    sealer: Arc<dyn Sealer>,
}

impl SealedFileStore {
    pub fn new(dir: impl Into<PathBuf>, sealer: Arc<dyn Sealer>) -> SealedFileStore {
        SealedFileStore {
            dir: dir.into(),
            sealer,
        }
    }

    /// The file the secret for `purpose` is sealed in.
    pub fn path(&self, purpose: KeyPurpose) -> PathBuf {
        self.dir.join(format!("{}.sealed", purpose.name()))
    }
}

/// `<sealer name>\n<sealed bytes>` after the magic.
fn parse_sealed(data: &[u8]) -> Option<(&str, &[u8])> {
    let rest = data.strip_prefix(SEALED_MAGIC)?;
    let end = rest.iter().position(|&b| b == b'\n')?;
    let name = std::str::from_utf8(&rest[..end]).ok()?;
    Some((name, &rest[end + 1..]))
}

impl HostKeyStore for SealedFileStore {
    fn load(&self, purpose: KeyPurpose) -> Result<Option<Secret>, KeyError> {
        let path = self.path(purpose);
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(KeyError::Failed(format!(
                    "the {} cannot be read from {}: {e}",
                    purpose.description(),
                    path.display()
                )))
            }
        };
        let Some((name, sealed)) = parse_sealed(&data) else {
            return Err(KeyError::Unreadable(format!(
                "{} is not a sealed key DSA made",
                path.display()
            )));
        };
        if name != self.sealer.name() {
            return Err(KeyError::Unreadable(format!(
                "the {} in {} was sealed by {name}, which this computer does not seal with \
                 ({}); it opens only where it was made",
                purpose.description(),
                path.display(),
                self.sealer.name()
            )));
        }
        let plain = self.sealer.unseal(sealed, purpose.name())?;
        Secret::from_slice(&plain).map(Some).ok_or_else(|| {
            KeyError::Unreadable(format!(
                "the {} in {} is {} bytes long, not {SECRET_LEN}",
                purpose.description(),
                path.display(),
                plain.len()
            ))
        })
    }

    fn store(&self, purpose: KeyPurpose, secret: &Secret) -> Result<(), KeyError> {
        let sealed = self.sealer.seal(secret.bytes(), purpose.name())?;
        let mut data = SEALED_MAGIC.to_vec();
        data.extend_from_slice(self.sealer.name().as_bytes());
        data.push(b'\n');
        data.extend_from_slice(&sealed);
        write_replacing(&self.path(purpose), &data).map_err(|e| {
            KeyError::Failed(format!(
                "the {} cannot be kept in {}: {e}",
                purpose.description(),
                self.dir.display()
            ))
        })
    }
}

/// Writes `data` to `path` through a file beside it, so a crash leaves the old file or the new
/// one, never half of either.
pub fn write_replacing(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".new");
    let temporary = path.with_file_name(name);
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)
}

/// A [`HostKeyStore`] for a host that has no credential store: every secret is unavailable,
/// with `why`.
pub struct NoKeyStore(pub String);

impl HostKeyStore for NoKeyStore {
    fn load(&self, _: KeyPurpose) -> Result<Option<Secret>, KeyError> {
        Err(KeyError::Unavailable(self.0.clone()))
    }
    fn store(&self, _: KeyPurpose, _: &Secret) -> Result<(), KeyError> {
        Err(KeyError::Unavailable(self.0.clone()))
    }
}
