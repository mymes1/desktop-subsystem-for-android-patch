//! The user's folders, shared with their Android, as Windows finds them.
//!
//! Each shared folder is one of the user's known folders (Documents, Downloads, Pictures,
//! Music, Videos, Desktop), found with the user's own token, so a folder the user moved (to
//! OneDrive, say) is shared where it is now. The service runs as SYSTEM, which could read
//! anything; the file server therefore runs each connection impersonating the signed-in user,
//! so Windows checks every open against that user's rights, exactly as for a program the user
//! runs. When the user is not signed in to any session there is no token, and nothing is
//! served.

use std::io;
use std::path::{Path, PathBuf};

use dsa_host_core::file_share::ShareHost;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::{ImpersonateLoggedOnUser, RevertToSelf};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Videos, SHGetKnownFolderPath,
};

use crate::pipe::{current_user_sid, OwnedHandle};

/// The folder in Android's shared storage the user's folders go under.
pub const LOCATION: &str = "Windows";

/// The known folder each shared folder name stands for.
fn known_folder(name: &str) -> Option<GUID> {
    Some(match name {
        "Documents" => FOLDERID_Documents,
        "Downloads" => FOLDERID_Downloads,
        "Pictures" => FOLDERID_Pictures,
        "Music" => FOLDERID_Music,
        "Videos" => FOLDERID_Videos,
        "Desktop" => FOLDERID_Desktop,
        _ => return None,
    })
}

/// The user with SID `sid`, whose folders are shared.
pub struct UserFolders {
    sid: String,
}

impl UserFolders {
    pub fn new(sid: &str) -> UserFolders {
        UserFolders {
            sid: sid.to_string(),
        }
    }

    /// The user's token, or `None` when this process is the user's own (`dsa-hostd serve`
    /// run by hand), which needs none.
    fn token(&self) -> io::Result<Option<OwnedHandle>> {
        if current_user_sid().is_ok_and(|me| me == self.sid) {
            return Ok(None);
        }
        crate::window_manager::user_token(&self.sid).map(Some)
    }
}

impl ShareHost for UserFolders {
    fn folder(&self, name: &str) -> Option<PathBuf> {
        let id = known_folder(name)?;
        let token = self.token().ok()?;
        let handle: HANDLE = token.as_ref().map_or(std::ptr::null_mut(), |token| token.0);
        let mut path = std::ptr::null_mut();
        // SAFETY: a known folder ID, the user's token (or none for this process's user), and
        // an out pointer the shell fills and the caller frees.
        let result = unsafe { SHGetKnownFolderPath(&id, 0, handle, &mut path) };
        if result < 0 || path.is_null() {
            if !path.is_null() {
                // SAFETY: the shell's allocation, freed once.
                unsafe { CoTaskMemFree(path.cast()) };
            }
            return None;
        }
        // SAFETY: a NUL-terminated string the shell allocated, read before it is freed.
        let text = unsafe {
            let mut len = 0;
            while *path.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(path, len))
        };
        // SAFETY: the shell's allocation, freed once.
        unsafe { CoTaskMemFree(path.cast()) };
        Some(PathBuf::from(text))
    }

    fn as_user(&self, serve: &mut dyn FnMut()) -> Result<(), String> {
        let token = self.token().map_err(|e| {
            format!("the user is not signed in, so their folders are not served ({e})")
        })?;
        let Some(token) = token else {
            serve();
            return Ok(());
        };
        // SAFETY: the user's token; this thread acts as the user until it reverts below.
        if unsafe { ImpersonateLoggedOnUser(token.0) } == 0 {
            return Err(format!(
                "cannot act as the user: {}",
                io::Error::last_os_error()
            ));
        }
        // Reverting must happen even if serving panics, or the thread would stay the user's.
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                // SAFETY: ends this thread's impersonation.
                unsafe { RevertToSelf() };
            }
        }
        let _revert = Revert;
        serve();
        Ok(())
    }

    fn space(&self, path: &Path) -> Option<(u64, u64)> {
        let wide: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
        // SAFETY: a NUL-terminated path and three out values.
        let ok =
            unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
        (ok != 0).then_some((total, available))
    }

    fn location(&self) -> String {
        LOCATION.into()
    }
}
