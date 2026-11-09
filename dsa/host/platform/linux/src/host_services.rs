//! The system services each user's session serves their Android on Linux: app data backups in
//! `~/DSA Backup`, the passkey check, and the root key of the guest's KeyMint.
//!
//! - The backup and KeyMint keys are kept in the user's Secret Service
//!   ([`crate::secret_service`]).
//! - The service runs as root, so it uses the backup folder only with the user's own file
//!   identity, as the shared folders are served: every file the backup makes is the user's,
//!   nothing reaches past what the user could, and a link the user puts in their folder can
//!   send nothing elsewhere. The folder itself is kept to its owner alone (mode `0700`).
//! - Linux has no passkey store a program can use, so no desktop session watches for passkey
//!   ceremonies. A request is still checked (the app's signature against the site's Digital
//!   Asset Links) and then answered as unavailable, with that reason.
//! - The KeyMint root key is given once per boot on its own vsock port.

use std::sync::Arc;

use dsa_host_core::backup::{folder_in, BackupService, FileAccess};
use dsa_host_core::file_share::ShareHost;
use dsa_host_core::host_keys::HostKeyStore;
use dsa_host_core::keymint_root::KEYMINT_PORT;
use dsa_host_core::passkeys::PasskeyService;
use dsa_host_core::session::Session;
use dsa_host_unix::backup_folder::keep_private;
use dsa_host_unix::https_get::CurlHttpsGet;

use crate::devices::file_share::UserFolders;
use crate::secret_service::SecretServiceKeys;
use crate::users::Account;
use crate::vsock::VsockHub;

/// What an app that asks for a passkey is told on Linux.
pub const NO_PASSKEYS: &str = "Linux has no passkey store DSA can use, so apps cannot make or \
     use passkeys on this computer; sign in another way, such as with a password";

/// Uses `folder` with the file identity of `folders`' user, and keeps it theirs alone.
fn file_access(folders: UserFolders, folder: std::path::PathBuf) -> FileAccess {
    Arc::new(move |run: &mut dyn FnMut()| {
        folders.as_user(&mut || {
            run();
            keep_private(&folder);
        })
    })
}

/// Gives `session` the user's backup, passkey check and KeyMint root key. `cid` is their
/// guest's vsock address; `build` names this host in the KeyMint port's handshake.
pub fn serve(
    session: &Session,
    account: &Account,
    cid: u32,
    build: &str,
    user: &str,
) -> Result<(), String> {
    let keys: Arc<dyn HostKeyStore> = Arc::new(SecretServiceKeys::new(account.clone()));

    let folder = folder_in(&account.home);
    let folders =
        UserFolders::for_uid(account.uid).map_err(|e| format!("the account of {user}: {e}"))?;
    let computer = crate::computer_identity::read().computer_name;
    let backup = BackupService::new(folder.clone(), Arc::clone(&keys), &computer)
        .with_file_access(file_access(folders, folder));
    session.serve_backup(Arc::new(backup));

    session.set_keymint_keys(keys);
    let listener = VsockHub::shared(KEYMINT_PORT)
        .map_err(|e| format!("vsock port {KEYMINT_PORT}: {e}"))?
        .listener(cid, false);
    session.serve_keymint_port(Box::new(listener), build);

    // The asset links are read with the user's rights, not root's.
    let curl = crate::network::find_program("curl")
        .unwrap_or_else(|| std::path::PathBuf::from("/usr/bin/curl"));
    let reader = account.clone();
    let fetch = CurlHttpsGet::new(curl).with_prepare(Box::new(move |command| {
        reader.run_as(command, None);
    }));
    session.serve_passkeys(Arc::new(
        PasskeyService::new(Arc::new(fetch)).with_no_authenticator_reason(NO_PASSKEYS),
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr36_a_passkey_request_on_linux_says_why_it_is_unavailable() {
        assert!(NO_PASSKEYS.contains("Linux has no passkey store"));
        assert!(!NO_PASSKEYS.contains("Windows"));
    }
}
