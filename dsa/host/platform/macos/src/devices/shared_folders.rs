//! The user's Mac folders shared into the guest's shared storage, under `Host`.

use std::path::{Path, PathBuf};

use dsa_host_core::file_share::ShareHost;

/// The folder in Android's shared storage the Mac's folders appear under.
pub const LOCATION: &str = dsa_host_core::file_share::LOCATION;

/// The Mac folder, under the home folder, shared under the guest's name `name`. Videos are
/// `Movies` on a Mac.
pub fn mac_folder(name: &str) -> Option<&'static str> {
    Some(match name {
        "Documents" => "Documents",
        "Downloads" => "Downloads",
        "Pictures" => "Pictures",
        "Music" => "Music",
        "Videos" => "Movies",
        "Desktop" => "Desktop",
        _ => return None,
    })
}

/// The user's folders, for the file server. The service runs as the user (a launch agent),
/// so the file system's own rights already apply and nothing is impersonated.
#[derive(Debug, Clone)]
pub struct MacFolders {
    pub home: PathBuf,
}

impl ShareHost for MacFolders {
    fn folder(&self, name: &str) -> Option<PathBuf> {
        let path = self.home.join(mac_folder(name)?);
        path.is_dir().then_some(path)
    }

    fn as_user(&self, serve: &mut dyn FnMut()) -> Result<(), String> {
        serve();
        Ok(())
    }

    fn space(&self, path: &Path) -> Option<(u64, u64)> {
        volume_space(path)
    }

    fn location(&self) -> String {
        LOCATION.into()
    }
}

#[cfg(target_os = "macos")]
fn volume_space(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let block = stat.f_frsize as u64;
    Some((
        (stat.f_blocks as u64).saturating_mul(block),
        (stat.f_bavail as u64).saturating_mul(block),
    ))
}

#[cfg(not(target_os = "macos"))]
fn volume_space(_path: &Path) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsa_host_core::file_share::FOLDERS;

    #[test]
    fn fr29_every_folder_the_guest_names_has_its_mac_folder() {
        for name in FOLDERS {
            assert!(mac_folder(name).is_some(), "{name} has no Mac folder");
        }
        assert_eq!(mac_folder("Videos"), Some("Movies"));
        assert_eq!(mac_folder("../etc"), None);
    }

    #[test]
    fn fr29_a_folder_the_user_does_not_have_is_not_shared() {
        let folders = MacFolders {
            home: PathBuf::from("/nonexistent-home-for-a-test"),
        };
        assert_eq!(folders.folder("Documents"), None);
        assert_eq!(folders.folder("Secrets"), None);
        assert_eq!(folders.location(), "Host");
    }
}
