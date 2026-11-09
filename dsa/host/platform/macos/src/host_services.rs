//! The system services the user's session serves their Android on macOS: app data backups in
//! `~/DSA Backup`, the passkey check, and the root key of the guest's KeyMint.
//!
//! - The backup and KeyMint keys are kept in the user's login keychain
//!   ([`crate::keychain`]).
//! - The agent runs as the user, so the backup folder is the user's own; it is kept to them
//!   alone (mode `0700`).
//! - A passkey request is checked (the app's signature against the site's Digital Asset
//!   Links, read with `/usr/bin/curl`, no redirect followed) and handed to the user's device
//!   lender (`dsa-devices`), which performs it with the Mac's passkeys
//!   (`crate::devices::passkeys`). With no device lender running, it is unavailable.
//! - The KeyMint root key is given once per boot on its own vsock port, which the VM listens
//!   on with the others ([`listen_ports`]).

use std::path::Path;
use std::sync::Arc;

use dsa_host_core::backup::{folder_in, BackupService};
use dsa_host_core::host_keys::HostKeyStore;
use dsa_host_core::keymint_root::KEYMINT_PORT;
use dsa_host_core::passkeys::PasskeyService;
use dsa_host_core::session::Session;
use dsa_host_unix::backup_folder::own_rights;
use dsa_host_unix::https_get::CurlHttpsGet;
use dsa_transport::Listener;

use crate::keychain::KeychainKeys;

/// The vsock ports of these services the VM listens on.
pub fn listen_ports() -> [u32; 1] {
    [KEYMINT_PORT]
}

/// What a checked passkey request is told when no device lender performs it.
pub const NO_DEVICE_LENDER: &str = "this Mac's passkeys are used through DSA's device lender \
     (dsa-devices), which is not running; sign in to the Mac's desktop, or start DSA again";

/// Gives `session` the user's backup, the passkey check and KeyMint's root key. `home` is the
/// user's home folder; `keymint` the listener of the KeyMint port; `build` names this host in
/// that port's handshake.
pub fn serve(session: &Session, home: &Path, keymint: Box<dyn Listener>, build: &str) {
    let keys: Arc<dyn HostKeyStore> = Arc::new(KeychainKeys);

    let folder = folder_in(home);
    let computer = crate::computer_identity::read().computer_name;
    let backup = BackupService::new(folder.clone(), Arc::clone(&keys), &computer)
        .with_file_access(own_rights(folder));
    session.serve_backup(Arc::new(backup));

    session.set_keymint_keys(keys);
    session.serve_keymint_port(keymint, build);

    let fetch = CurlHttpsGet::new("/usr/bin/curl");
    session.serve_passkeys(Arc::new(
        PasskeyService::new(Arc::new(fetch)).with_no_authenticator_reason(NO_DEVICE_LENDER),
    ));
}
