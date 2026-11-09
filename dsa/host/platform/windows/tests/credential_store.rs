//! The backup and KeyMint root keys sealed by DPAPI and the computer's TPM, as the user this
//! test runs as (Windows only).
#![cfg(windows)]

use dsa_host_core::host_keys::{load_or_create, HostKeyStore, KeyError, KeyPurpose, Sealer};
use dsa_host_windows::credential_store::{
    for_user, sealer_of, DpapiSealer, TpmDpapiSealer, DPAPI, TPM_DPAPI,
};
use dsa_host_windows::pipe::current_user_sid;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("keys {name} {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn fr36_dpapi_opens_what_it_sealed_only_for_the_same_purpose() {
    let sealer = DpapiSealer::new(&current_user_sid().unwrap());
    let sealed = sealer.seal(&[3u8; 32], "backup-key").unwrap();
    assert!(
        !sealed.windows(32).any(|w| w == [3u8; 32]),
        "sealed in the clear"
    );
    assert_eq!(sealer.unseal(&sealed, "backup-key").unwrap(), vec![3u8; 32]);
    assert!(matches!(
        sealer.unseal(&sealed, "keymint-root-key"),
        Err(KeyError::Unreadable(_))
    ));
}

#[test]
fn fr36_keymint_root_key_is_sealed_by_the_host() {
    let sid = current_user_sid().unwrap();
    let dir = scratch("keymint");
    let keys = for_user(&sid, &dir);
    let key = load_or_create(&keys, KeyPurpose::KeyMintRoot).unwrap();
    assert_eq!(
        keys.load(KeyPurpose::KeyMintRoot).unwrap(),
        Some(key.clone())
    );
    let data = std::fs::read(keys.path(KeyPurpose::KeyMintRoot)).unwrap();
    let tpm = TpmDpapiSealer::new(&sid).check();
    let expected = if tpm.is_ok() { TPM_DPAPI } else { DPAPI };
    assert_eq!(sealer_of(&data), Some(expected), "TPM check: {tpm:?}");
    assert!(
        !data.windows(32).any(|w| w == key.bytes()),
        "the key is on disk in the clear"
    );
    // The same file, sealed for this user, opens for no other account: DPAPI under another
    // user's credentials is refused (here, a file whose seal was changed by a byte).
    let mut tampered = data.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    std::fs::write(keys.path(KeyPurpose::KeyMintRoot), &tampered).unwrap();
    assert!(keys.load(KeyPurpose::KeyMintRoot).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
