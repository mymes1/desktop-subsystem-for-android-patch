//! Each user's secrets in Windows' own credential store: the key their backup folder is
//! encrypted with, and the root key of their Android's KeyMint.
//!
//! Each secret is sealed in a file of the user's DSA folder (`%LOCALAPPDATA%\DSA\keys`),
//! never on the guest's disk:
//!
//! - `dpapi`: DPAPI under the user's own account (`CryptProtectData`, with what the secret is
//!   for as its entropy). Only that user, on this computer or wherever their account's
//!   credentials roam, opens it. The service runs as SYSTEM in session 0, so it seals and
//!   opens while impersonating the user, with the token of a session they are signed in to;
//!   when they are signed in nowhere there is no token, and the secret is unavailable.
//! - `tpm+dpapi`: first encrypted (RSA-OAEP with SHA-256) to a key the computer's TPM keeps
//!   (the Microsoft Platform Crypto Provider, a persisted 2048-bit RSA key named for DSA and
//!   the user's SID, which never leaves the TPM), then sealed by DPAPI as above. It opens only
//!   for that user on this computer: a copy of the file and of the user's credentials is not
//!   enough without this TPM.
//!
//! The KeyMint root key is sealed `tpm+dpapi` when the computer has a TPM that works, else
//! `dpapi`. The backup key is always `dpapi`: a backup is restored on another computer with
//! the recovery key, and nothing about it is bound to this computer's TPM.
//!
//! Each file names the sealer that made it ([`SealedFileStore`] writes the name), and a secret
//! is opened with that sealer, whichever one a new secret would get now. So a computer whose
//! TPM appears later goes on opening its `dpapi` key, and one whose TPM stops working (turned
//! off in the firmware, cleared) says so, rather than making a new key: a new KeyMint root
//! key would leave every key Android wrapped under the old one unreadable for good. A kept
//! KeyMint root key is never replaced.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dsa_host_core::host_keys::{
    HostKeyStore, KeyError, KeyPurpose, SealedFileStore, Sealer, Secret,
};

/// The name of the DPAPI sealer, as sealed files record it.
pub const DPAPI: &str = "dpapi";
/// The name of the TPM and DPAPI sealer, as sealed files record it.
pub const TPM_DPAPI: &str = "tpm+dpapi";

/// What a sealed file starts with, before the sealer's name: the format the platform-neutral
/// [`SealedFileStore`] writes.
const SEALED_MAGIC: &[u8] = b"DSA sealed key 1\n";

/// The name of the sealer a sealed file records, if `data` is one.
pub fn sealer_of(data: &[u8]) -> Option<&str> {
    let rest = data.strip_prefix(SEALED_MAGIC)?;
    let end = rest.iter().position(|&b| b == b'\n')?;
    std::str::from_utf8(&rest[..end]).ok()
}

/// The sealer a new secret for `purpose` gets, given whether the computer's TPM works
/// (`Ok`) or why not.
pub fn preferred_sealer(purpose: KeyPurpose, tpm: &Result<(), String>) -> &'static str {
    match (purpose, tpm) {
        (KeyPurpose::KeyMintRoot, Ok(())) => TPM_DPAPI,
        _ => DPAPI,
    }
}

/// The sealers a computer has.
pub trait SealerSource: Send + Sync {
    /// The sealer a new secret for `purpose` is sealed with.
    fn preferred(&self, purpose: KeyPurpose) -> Result<Arc<dyn Sealer>, KeyError>;
    /// The sealer named `name`, to open a secret it sealed; an error says why this computer
    /// cannot open it.
    fn named(&self, name: &str, purpose: KeyPurpose) -> Result<Arc<dyn Sealer>, KeyError>;
}

/// A user's secrets, each in a sealed file of `dir`, opened with the sealer its file names
/// and sealed anew with the one [`SealerSource::preferred`] gives.
pub struct UserKeyStore {
    dir: PathBuf,
    sealers: Arc<dyn SealerSource>,
}

impl UserKeyStore {
    pub fn new(dir: impl Into<PathBuf>, sealers: Arc<dyn SealerSource>) -> UserKeyStore {
        UserKeyStore {
            dir: dir.into(),
            sealers,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file the secret for `purpose` is sealed in.
    pub fn path(&self, purpose: KeyPurpose) -> PathBuf {
        self.dir.join(format!("{}.sealed", purpose.name()))
    }
}

impl HostKeyStore for UserKeyStore {
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
        let Some(name) = sealer_of(&data) else {
            return Err(KeyError::Unreadable(format!(
                "{} is not a sealed key DSA made",
                path.display()
            )));
        };
        let sealer = self.sealers.named(name, purpose)?;
        SealedFileStore::new(&self.dir, sealer).load(purpose)
    }

    fn store(&self, purpose: KeyPurpose, secret: &Secret) -> Result<(), KeyError> {
        // The backup key is replaced when the user brings a recovery key; the KeyMint root key
        // never is, since Android's keys are wrapped under the one kept.
        if purpose == KeyPurpose::KeyMintRoot && self.path(purpose).exists() {
            return Err(KeyError::Failed(format!(
                "a {} is kept in {} already, and it is never replaced",
                purpose.description(),
                self.dir.display()
            )));
        }
        let sealer = self.sealers.preferred(purpose)?;
        SealedFileStore::new(&self.dir, sealer).store(purpose, secret)
    }
}

/// Whether an NCrypt or TPM error `hr` means the computer has no TPM that can be used (none,
/// turned off, not ready, too old for what is asked), rather than a failure of a TPM that works.
pub fn tpm_unusable(hr: i32) -> bool {
    const NTE_BAD_PROVIDER: u32 = 0x8009_0013;
    const NTE_PROVIDER_DLL_FAIL: u32 = 0x8009_001D;
    const NTE_PROV_DLL_NOT_FOUND: u32 = 0x8009_001E;
    const NTE_NOT_SUPPORTED: u32 = 0x8009_0029;
    const NTE_DEVICE_NOT_READY: u32 = 0x8009_0030;
    const NTE_DEVICE_NOT_FOUND: u32 = 0x8009_0035;
    const NTE_BAD_ALGID: u32 = 0x8009_0008;
    let code = hr as u32;
    matches!(
        code,
        NTE_BAD_PROVIDER
            | NTE_PROVIDER_DLL_FAIL
            | NTE_PROV_DLL_NOT_FOUND
            | NTE_NOT_SUPPORTED
            | NTE_DEVICE_NOT_READY
            | NTE_DEVICE_NOT_FOUND
            | NTE_BAD_ALGID
    )
        // TPM_E_* (the TPM's own errors) and TBS_E_* (TPM Base Services: no TPM, turned off).
        || (code & 0xFFFF_0000) == 0x8028_0000
        || (code & 0xFFFF_0000) == 0x8029_0000
}

/// The TPM key's name for the user with SID `sid`.
pub fn tpm_key_name(sid: &str) -> String {
    format!("Desktop Subsystem for Android key sealing {sid}")
}

/// The entropy DPAPI binds into a seal: the sealer and what is sealed, so a seal of one
/// purpose (or of one sealer's inner bytes) never opens as another.
pub fn dpapi_entropy(sealer: &str, label: &str) -> Vec<u8> {
    format!("Desktop Subsystem for Android {sealer} {label}").into_bytes()
}

/// A text for HRESULT `hr`: Windows' message and the code.
pub fn hresult_text(hr: i32) -> String {
    let message = std::io::Error::from_raw_os_error(hr).to_string();
    format!("{message} (0x{:08X})", hr as u32)
}

#[cfg(windows)]
pub use self::sealers::{DpapiSealer, TpmDpapiSealer, WindowsSealers};

#[cfg(windows)]
mod sealers {
    use std::io;
    use std::sync::Arc;

    use dsa_host_core::host_keys::{KeyError, KeyPurpose, Sealer};
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, NCryptCreatePersistedKey, NCryptDecrypt,
        NCryptEncrypt, NCryptFinalizeKey, NCryptFreeObject, NCryptOpenKey,
        NCryptOpenStorageProvider, NCryptSetProperty, BCRYPT_OAEP_PADDING_INFO,
        BCRYPT_SHA256_ALGORITHM, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        MS_PLATFORM_CRYPTO_PROVIDER, NCRYPT_ALLOW_DECRYPT_FLAG, NCRYPT_KEY_USAGE_PROPERTY,
        NCRYPT_LENGTH_PROPERTY, NCRYPT_MACHINE_KEY_FLAG, NCRYPT_PAD_OAEP_FLAG,
        NCRYPT_RSA_ALGORITHM, NCRYPT_SILENT_FLAG,
    };
    use windows_sys::Win32::Security::{ImpersonateLoggedOnUser, RevertToSelf};

    use super::{
        dpapi_entropy, hresult_text, preferred_sealer, tpm_key_name, tpm_unusable, SealerSource,
        DPAPI, TPM_DPAPI,
    };
    use crate::pipe::current_user_sid;

    const NTE_BAD_KEYSET: i32 = 0x8009_0016_u32 as i32;
    const NTE_EXISTS: i32 = 0x8009_000F_u32 as i32;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Runs `work` as the user with SID `sid`: on this thread, impersonating them with the
    /// token of a session they are signed in to, or as this process when it is the user's own.
    fn as_user<T>(sid: &str, work: impl FnOnce() -> T) -> Result<T, KeyError> {
        if current_user_sid().is_ok_and(|me| me == sid) {
            return Ok(work());
        }
        let token = crate::window_manager::user_token(sid).map_err(|e| {
            KeyError::Unavailable(format!(
                "Windows opens this user's credentials only while they are signed in, and they \
                 are not ({e})"
            ))
        })?;
        // SAFETY: the user's token; this thread acts as the user until it reverts below.
        if unsafe { ImpersonateLoggedOnUser(token.0) } == 0 {
            return Err(KeyError::Failed(format!(
                "cannot act as the user to open their credentials: {}",
                io::Error::last_os_error()
            )));
        }
        // Reverting must happen even if the work panics, or the thread would stay the user's.
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                // SAFETY: ends this thread's impersonation.
                unsafe { RevertToSelf() };
            }
        }
        let _revert = Revert;
        Ok(work())
    }

    /// Takes the bytes of a blob DPAPI allocated, and frees it.
    fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if blob.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: DPAPI returned `cbData` bytes at `pbData`, freed once below.
        let bytes =
            unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
        // SAFETY: a LocalAlloc'd buffer DPAPI handed over.
        unsafe { LocalFree(blob.pbData as HLOCAL) };
        bytes
    }

    fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        }
    }

    /// DPAPI under the user's account, which never shows a prompt.
    fn dpapi_protect(
        sid: &str,
        plain: &[u8],
        entropy: &[u8],
        what: &str,
    ) -> Result<Vec<u8>, KeyError> {
        let description = wide(what);
        as_user(sid, || {
            let (input, entropy) = (blob(plain), blob(entropy));
            let mut output = CRYPT_INTEGER_BLOB::default();
            // SAFETY: blobs over buffers that outlive the call, and an out blob DPAPI fills.
            let ok = unsafe {
                CryptProtectData(
                    &input,
                    description.as_ptr(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(KeyError::Failed(format!(
                    "Windows could not seal the {what} for this user: {}",
                    io::Error::last_os_error()
                )));
            }
            Ok(take_blob(output))
        })?
    }

    fn dpapi_unprotect(
        sid: &str,
        sealed: &[u8],
        entropy: &[u8],
        what: &str,
    ) -> Result<Vec<u8>, KeyError> {
        as_user(sid, || {
            let (input, entropy) = (blob(sealed), blob(entropy));
            let mut output = CRYPT_INTEGER_BLOB::default();
            // SAFETY: blobs over buffers that outlive the call, and an out blob DPAPI fills.
            let ok = unsafe {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(KeyError::Unreadable(format!(
                    "Windows cannot open the {what} for this user: it was sealed for another \
                     user or on another computer ({})",
                    io::Error::last_os_error()
                )));
            }
            Ok(take_blob(output))
        })?
    }

    /// Seals with DPAPI under the account of the user with SID `sid`.
    pub struct DpapiSealer {
        sid: String,
    }

    impl DpapiSealer {
        pub fn new(sid: &str) -> DpapiSealer {
            DpapiSealer {
                sid: sid.to_string(),
            }
        }
    }

    impl Sealer for DpapiSealer {
        fn name(&self) -> &str {
            DPAPI
        }
        fn seal(&self, plain: &[u8], label: &str) -> Result<Vec<u8>, KeyError> {
            dpapi_protect(&self.sid, plain, &dpapi_entropy(DPAPI, label), label)
        }
        fn unseal(&self, sealed: &[u8], label: &str) -> Result<Vec<u8>, KeyError> {
            dpapi_unprotect(&self.sid, sealed, &dpapi_entropy(DPAPI, label), label)
        }
    }

    /// An NCrypt handle, freed when dropped.
    struct NcryptObject(usize);

    impl Drop for NcryptObject {
        fn drop(&mut self) {
            // SAFETY: an NCrypt handle, freed once.
            unsafe { NCryptFreeObject(self.0) };
        }
    }

    fn hr_ok(hr: i32) -> Result<(), i32> {
        if hr >= 0 {
            Ok(())
        } else {
            Err(hr)
        }
    }

    /// The TPM key of one user, through the Microsoft Platform Crypto Provider.
    struct TpmKey {
        name: Vec<u16>,
        /// Whether the key is the machine's (named for the user) rather than in the user
        /// profile of this process's account: the service, as SYSTEM, keeps machine keys.
        machine: bool,
    }

    impl TpmKey {
        fn of(sid: &str) -> TpmKey {
            TpmKey {
                name: wide(&tpm_key_name(sid)),
                machine: !current_user_sid().is_ok_and(|me| me == sid),
            }
        }

        fn flags(&self) -> u32 {
            NCRYPT_SILENT_FLAG
                | if self.machine {
                    NCRYPT_MACHINE_KEY_FLAG
                } else {
                    0
                }
        }

        fn provider() -> Result<NcryptObject, i32> {
            let mut provider = 0usize;
            // SAFETY: an out handle and the provider's NUL-terminated name.
            hr_ok(unsafe {
                NCryptOpenStorageProvider(&mut provider, MS_PLATFORM_CRYPTO_PROVIDER, 0)
            })?;
            Ok(NcryptObject(provider))
        }

        /// The key, `None` when the TPM has none by that name.
        fn open(&self, provider: &NcryptObject) -> Result<Option<NcryptObject>, i32> {
            let mut key = 0usize;
            // SAFETY: a provider handle, a NUL-terminated name and an out handle.
            let hr =
                unsafe { NCryptOpenKey(provider.0, &mut key, self.name.as_ptr(), 0, self.flags()) };
            match hr {
                NTE_BAD_KEYSET => Ok(None),
                hr => hr_ok(hr).map(|()| Some(NcryptObject(key))),
            }
        }

        fn open_or_create(&self, provider: &NcryptObject) -> Result<NcryptObject, i32> {
            if let Some(key) = self.open(provider)? {
                return Ok(key);
            }
            let mut key = 0usize;
            // SAFETY: a provider handle, the algorithm's and key's NUL-terminated names and an
            // out handle.
            let hr = unsafe {
                NCryptCreatePersistedKey(
                    provider.0,
                    &mut key,
                    NCRYPT_RSA_ALGORITHM,
                    self.name.as_ptr(),
                    0,
                    if self.machine {
                        NCRYPT_MACHINE_KEY_FLAG
                    } else {
                        0
                    },
                )
            };
            if hr == NTE_EXISTS {
                // Made by another thread just now.
                return self.open(provider)?.ok_or(NTE_BAD_KEYSET);
            }
            hr_ok(hr)?;
            let key = NcryptObject(key);
            let length: u32 = 2048;
            let usage: u32 = NCRYPT_ALLOW_DECRYPT_FLAG;
            for (property, value) in [
                (NCRYPT_LENGTH_PROPERTY, length),
                (NCRYPT_KEY_USAGE_PROPERTY, usage),
            ] {
                // SAFETY: a key handle, a property name and a u32 value of the size given.
                hr_ok(unsafe {
                    NCryptSetProperty(
                        key.0,
                        property,
                        (&value as *const u32).cast(),
                        std::mem::size_of::<u32>() as u32,
                        0,
                    )
                })?;
            }
            // SAFETY: a key handle whose properties are set.
            hr_ok(unsafe { NCryptFinalizeKey(key.0, NCRYPT_SILENT_FLAG) })?;
            Ok(key)
        }
    }

    fn oaep() -> BCRYPT_OAEP_PADDING_INFO {
        BCRYPT_OAEP_PADDING_INFO {
            pszAlgId: BCRYPT_SHA256_ALGORITHM,
            pbLabel: std::ptr::null_mut(),
            cbLabel: 0,
        }
    }

    /// Encrypts (`encrypt`) or decrypts `input` with `key`, RSA-OAEP with SHA-256.
    fn rsa(key: &NcryptObject, input: &[u8], encrypt: bool) -> Result<Vec<u8>, i32> {
        let padding = oaep();
        let call = |output: *mut u8, capacity: u32, written: &mut u32| -> i32 {
            // SAFETY: a key handle, the input, the padding info and an output buffer of
            // `capacity` bytes (or none, to ask for the size).
            unsafe {
                if encrypt {
                    NCryptEncrypt(
                        key.0,
                        input.as_ptr(),
                        input.len() as u32,
                        (&padding as *const BCRYPT_OAEP_PADDING_INFO).cast(),
                        output,
                        capacity,
                        written,
                        NCRYPT_PAD_OAEP_FLAG | NCRYPT_SILENT_FLAG,
                    )
                } else {
                    NCryptDecrypt(
                        key.0,
                        input.as_ptr(),
                        input.len() as u32,
                        (&padding as *const BCRYPT_OAEP_PADDING_INFO).cast(),
                        output,
                        capacity,
                        written,
                        NCRYPT_PAD_OAEP_FLAG | NCRYPT_SILENT_FLAG,
                    )
                }
            }
        };
        let mut size = 0u32;
        hr_ok(call(std::ptr::null_mut(), 0, &mut size))?;
        let mut output = vec![0u8; size as usize];
        let mut written = 0u32;
        hr_ok(call(output.as_mut_ptr(), output.len() as u32, &mut written))?;
        output.truncate(written as usize);
        Ok(output)
    }

    /// Encrypts to the TPM key of the user with SID `sid`, then seals that with DPAPI under
    /// their account.
    pub struct TpmDpapiSealer {
        sid: String,
    }

    impl TpmDpapiSealer {
        pub fn new(sid: &str) -> TpmDpapiSealer {
            TpmDpapiSealer {
                sid: sid.to_string(),
            }
        }

        /// Whether the computer's TPM can seal and open for this user now: its provider opens,
        /// the user's key is there or can be made, and a test secret goes through it and back.
        /// `Err` says why not, and whether that means there is no usable TPM (`true`).
        pub fn check(&self) -> Result<(), (bool, String)> {
            let key = TpmKey::of(&self.sid);
            let fail =
                |hr: i32, step: &str| (tpm_unusable(hr), format!("{step}: {}", hresult_text(hr)));
            let provider =
                TpmKey::provider().map_err(|hr| fail(hr, "opening the TPM's key provider"))?;
            let key = key
                .open_or_create(&provider)
                .map_err(|hr| fail(hr, "making the TPM key"))?;
            let probe = [0x5Au8; 32];
            let sealed =
                rsa(&key, &probe, true).map_err(|hr| fail(hr, "encrypting to the TPM key"))?;
            let opened =
                rsa(&key, &sealed, false).map_err(|hr| fail(hr, "decrypting with the TPM key"))?;
            if opened != probe {
                return Err((
                    true,
                    "the TPM returned other bytes than it was given".into(),
                ));
            }
            Ok(())
        }
    }

    impl Sealer for TpmDpapiSealer {
        fn name(&self) -> &str {
            TPM_DPAPI
        }

        fn seal(&self, plain: &[u8], label: &str) -> Result<Vec<u8>, KeyError> {
            let tpm = TpmKey::of(&self.sid);
            let failed = |hr: i32| {
                KeyError::Failed(format!(
                    "the computer's TPM could not seal the {label}: {}",
                    hresult_text(hr)
                ))
            };
            let provider = TpmKey::provider().map_err(failed)?;
            let key = tpm.open_or_create(&provider).map_err(failed)?;
            let inner = rsa(&key, plain, true).map_err(failed)?;
            dpapi_protect(&self.sid, &inner, &dpapi_entropy(TPM_DPAPI, label), label)
        }

        fn unseal(&self, sealed: &[u8], label: &str) -> Result<Vec<u8>, KeyError> {
            let inner =
                dpapi_unprotect(&self.sid, sealed, &dpapi_entropy(TPM_DPAPI, label), label)?;
            let tpm = TpmKey::of(&self.sid);
            let provider = TpmKey::provider().map_err(|hr| {
                KeyError::Unreadable(format!(
                    "the {label} was sealed by this computer's TPM, which cannot be used now \
                     ({}); turn the TPM on again in the computer's firmware settings",
                    hresult_text(hr)
                ))
            })?;
            let key = match tpm.open(&provider) {
                Ok(Some(key)) => key,
                Ok(None) => {
                    return Err(KeyError::Unreadable(format!(
                        "the TPM key the {label} was sealed with is gone: the TPM was cleared, \
                         or this is not the computer it was sealed on"
                    )))
                }
                Err(hr) => {
                    return Err(KeyError::Unreadable(format!(
                        "the TPM key the {label} was sealed with cannot be opened: {}",
                        hresult_text(hr)
                    )))
                }
            };
            rsa(&key, &inner, false).map_err(|hr| {
                KeyError::Unreadable(format!(
                    "the computer's TPM cannot open the {label}: {}",
                    hresult_text(hr)
                ))
            })
        }
    }

    /// The sealers of the user with SID `sid` on this computer.
    pub struct WindowsSealers {
        sid: String,
    }

    impl WindowsSealers {
        pub fn new(sid: &str) -> WindowsSealers {
            WindowsSealers {
                sid: sid.to_string(),
            }
        }
    }

    impl SealerSource for WindowsSealers {
        fn preferred(&self, purpose: KeyPurpose) -> Result<Arc<dyn Sealer>, KeyError> {
            let tpm = match purpose {
                KeyPurpose::KeyMintRoot => match TpmDpapiSealer::new(&self.sid).check() {
                    Ok(()) => Ok(()),
                    Err((true, why)) => {
                        eprintln!(
                            "{}: keys: this computer has no TPM that works ({why}), so the {} is \
                             sealed by DPAPI alone",
                            self.sid,
                            purpose.description()
                        );
                        Err(why)
                    }
                    // A TPM that is there but failed: the key is not made weaker for it.
                    Err((false, why)) => {
                        return Err(KeyError::Failed(format!(
                            "the computer's TPM could not seal the {}: {why}",
                            purpose.description()
                        )))
                    }
                },
                KeyPurpose::Backup => Err("not used for the backup key".into()),
            };
            Ok(match preferred_sealer(purpose, &tpm) {
                TPM_DPAPI => Arc::new(TpmDpapiSealer::new(&self.sid)),
                _ => Arc::new(DpapiSealer::new(&self.sid)),
            })
        }

        fn named(&self, name: &str, purpose: KeyPurpose) -> Result<Arc<dyn Sealer>, KeyError> {
            match name {
                DPAPI => Ok(Arc::new(DpapiSealer::new(&self.sid))),
                TPM_DPAPI => Ok(Arc::new(TpmDpapiSealer::new(&self.sid))),
                other => Err(KeyError::Unreadable(format!(
                    "the {} was sealed by {other}, which this version of DSA does not know",
                    purpose.description()
                ))),
            }
        }
    }
}

/// The store of the user with SID `sid`, whose sealed files live in `dir`.
#[cfg(windows)]
pub fn for_user(sid: &str, dir: &Path) -> UserKeyStore {
    UserKeyStore::new(dir, Arc::new(WindowsSealers::new(sid)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A sealer that records its name in what it seals, and opens only its own.
    struct NamedSealer(&'static str);

    impl Sealer for NamedSealer {
        fn name(&self) -> &str {
            self.0
        }
        fn seal(&self, plain: &[u8], label: &str) -> Result<Vec<u8>, KeyError> {
            let mut out = format!("{}:{label}:", self.0).into_bytes();
            out.extend_from_slice(plain);
            Ok(out)
        }
        fn unseal(&self, sealed: &[u8], label: &str) -> Result<Vec<u8>, KeyError> {
            let head = format!("{}:{label}:", self.0).into_bytes();
            sealed
                .strip_prefix(head.as_slice())
                .map(<[u8]>::to_vec)
                .ok_or_else(|| KeyError::Unreadable("not sealed here".into()))
        }
    }

    /// A computer whose TPM can be turned on and off.
    struct FakeComputer {
        tpm: Mutex<bool>,
    }

    impl SealerSource for FakeComputer {
        fn preferred(&self, purpose: KeyPurpose) -> Result<Arc<dyn Sealer>, KeyError> {
            let tpm = if *self.tpm.lock().unwrap() {
                Ok(())
            } else {
                Err("no TPM".to_string())
            };
            Ok(match preferred_sealer(purpose, &tpm) {
                TPM_DPAPI => Arc::new(NamedSealer(TPM_DPAPI)),
                _ => Arc::new(NamedSealer(DPAPI)),
            })
        }
        fn named(&self, name: &str, _: KeyPurpose) -> Result<Arc<dyn Sealer>, KeyError> {
            match name {
                DPAPI => Ok(Arc::new(NamedSealer(DPAPI))),
                TPM_DPAPI if *self.tpm.lock().unwrap() => Ok(Arc::new(NamedSealer(TPM_DPAPI))),
                TPM_DPAPI => Err(KeyError::Unreadable("the TPM cannot be used now".into())),
                other => Err(KeyError::Unreadable(format!("unknown sealer {other}"))),
            }
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsa-keys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn store(dir: &Path, tpm: bool) -> (UserKeyStore, Arc<FakeComputer>) {
        let computer = Arc::new(FakeComputer {
            tpm: Mutex::new(tpm),
        });
        (UserKeyStore::new(dir, computer.clone()), computer)
    }

    #[test]
    fn fr36_the_file_format_is_the_one_the_sealed_file_store_writes() {
        let dir = scratch("format");
        let core = SealedFileStore::new(&dir, Arc::new(NamedSealer("tpm+dpapi")));
        let secret = Secret::from_bytes([7; 32]);
        core.store(KeyPurpose::KeyMintRoot, &secret).unwrap();
        let data = std::fs::read(core.path(KeyPurpose::KeyMintRoot)).unwrap();
        assert_eq!(sealer_of(&data), Some("tpm+dpapi"));
        assert_eq!(sealer_of(b"something else"), None);
        let (ours, _) = store(&dir, true);
        assert_eq!(
            ours.path(KeyPurpose::KeyMintRoot),
            core.path(KeyPurpose::KeyMintRoot)
        );
        assert_eq!(ours.load(KeyPurpose::KeyMintRoot).unwrap(), Some(secret));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fr36_the_keymint_root_key_is_sealed_by_the_tpm_when_there_is_one_and_the_backup_key_never() {
        assert_eq!(
            preferred_sealer(KeyPurpose::KeyMintRoot, &Ok(())),
            TPM_DPAPI
        );
        assert_eq!(
            preferred_sealer(KeyPurpose::KeyMintRoot, &Err("no TPM".into())),
            DPAPI
        );
        assert_eq!(preferred_sealer(KeyPurpose::Backup, &Ok(())), DPAPI);

        let dir = scratch("choice");
        let (keys, _) = store(&dir, true);
        dsa_host_core::host_keys::load_or_create(&keys, KeyPurpose::KeyMintRoot).unwrap();
        dsa_host_core::host_keys::load_or_create(&keys, KeyPurpose::Backup).unwrap();
        let sealer = |purpose| {
            let data = std::fs::read(keys.path(purpose)).unwrap();
            sealer_of(&data).unwrap().to_string()
        };
        assert_eq!(sealer(KeyPurpose::KeyMintRoot), TPM_DPAPI);
        assert_eq!(sealer(KeyPurpose::Backup), DPAPI);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fr36_a_key_sealed_without_a_tpm_still_opens_once_a_tpm_appears() {
        let dir = scratch("appears");
        let (keys, computer) = store(&dir, false);
        let first =
            dsa_host_core::host_keys::load_or_create(&keys, KeyPurpose::KeyMintRoot).unwrap();
        *computer.tpm.lock().unwrap() = true;
        let again =
            dsa_host_core::host_keys::load_or_create(&keys, KeyPurpose::KeyMintRoot).unwrap();
        assert_eq!(
            first, again,
            "the kept key is used, not a new one sealed by the TPM"
        );
        let data = std::fs::read(keys.path(KeyPurpose::KeyMintRoot)).unwrap();
        assert_eq!(sealer_of(&data), Some(DPAPI));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fr36_a_key_sealed_by_a_tpm_that_is_gone_is_refused_and_never_replaced() {
        let dir = scratch("gone");
        let (keys, computer) = store(&dir, true);
        dsa_host_core::host_keys::load_or_create(&keys, KeyPurpose::KeyMintRoot).unwrap();
        let before = std::fs::read(keys.path(KeyPurpose::KeyMintRoot)).unwrap();
        *computer.tpm.lock().unwrap() = false;
        let error =
            dsa_host_core::host_keys::load_or_create(&keys, KeyPurpose::KeyMintRoot).unwrap_err();
        assert!(matches!(error, KeyError::Unreadable(_)), "{error:?}");
        assert!(
            keys.store(KeyPurpose::KeyMintRoot, &Secret::from_bytes([1; 32]))
                .is_err(),
            "a kept KeyMint root key is never replaced"
        );
        assert_eq!(
            std::fs::read(keys.path(KeyPurpose::KeyMintRoot)).unwrap(),
            before
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fr36_the_backup_key_is_replaced_by_a_recovery_key() {
        let dir = scratch("recovery");
        let (keys, _) = store(&dir, true);
        dsa_host_core::host_keys::load_or_create(&keys, KeyPurpose::Backup).unwrap();
        let recovered = Secret::from_bytes([9; 32]);
        keys.store(KeyPurpose::Backup, &recovered).unwrap();
        assert_eq!(keys.load(KeyPurpose::Backup).unwrap(), Some(recovered));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fr36_a_file_that_is_not_a_sealed_key_is_unreadable() {
        let dir = scratch("garbage");
        std::fs::create_dir_all(&dir).unwrap();
        let (keys, _) = store(&dir, true);
        std::fs::write(keys.path(KeyPurpose::Backup), b"not a key").unwrap();
        assert!(matches!(
            keys.load(KeyPurpose::Backup),
            Err(KeyError::Unreadable(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fr36_no_usable_tpm_is_told_from_a_tpm_that_failed() {
        for code in [
            0x8009_0035u32,
            0x8009_0030,
            0x8009_001E,
            0x8028_400F,
            0x8029_0102,
        ] {
            assert!(tpm_unusable(code as i32), "{code:08X}");
        }
        for code in [0x8007_0005u32, 0x8009_0016, 0x8009_0036] {
            assert!(!tpm_unusable(code as i32), "{code:08X}");
        }
    }

    #[test]
    fn fr36_each_user_has_a_tpm_key_of_their_own_and_each_seal_its_own_entropy() {
        assert_ne!(
            tpm_key_name("S-1-5-21-1-2-3-1001"),
            tpm_key_name("S-1-5-21-1-2-3-1002")
        );
        assert!(tpm_key_name("S-1-5-21-1-2-3-1001").ends_with("S-1-5-21-1-2-3-1001"));
        assert_ne!(
            dpapi_entropy(DPAPI, "backup-key"),
            dpapi_entropy(TPM_DPAPI, "backup-key")
        );
        assert_ne!(
            dpapi_entropy(DPAPI, "backup-key"),
            dpapi_entropy(DPAPI, "keymint-root-key")
        );
    }
}
