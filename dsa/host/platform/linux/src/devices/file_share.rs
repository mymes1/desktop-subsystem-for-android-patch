//! The user's folders, shared with their Android, as the Linux desktop names them.
//!
//! Each shared folder is one of the user's XDG folders (Documents, Downloads, Pictures,
//! Music, Videos, Desktop), read from their `user-dirs.dirs`, so a folder the user moved or
//! that has another name in their language is shared where it is. A folder the user's
//! settings turn off (it points at their home) is not shared.
//!
//! The service may run as root, which could read anything; each connection is then served
//! with the user's own file system identity (`setfsuid`, `setfsgid` and the user's groups,
//! for the serving thread alone), so the kernel checks every open against that user's
//! rights, exactly as for a program the user runs. Root's file capabilities fall away with
//! the identity, so nothing reaches past what the user could.

use std::ffi::{CStr, CString};
use std::io;
use std::path::{Path, PathBuf};

use dsa_host_core::file_share::ShareHost;

/// The folder in Android's shared storage the user's folders go under.
pub const LOCATION: &str = "Linux";

/// The `user-dirs.dirs` key for each shared folder name.
fn xdg_key(name: &str) -> Option<&'static str> {
    Some(match name {
        "Documents" => "XDG_DOCUMENTS_DIR",
        "Downloads" => "XDG_DOWNLOAD_DIR",
        "Pictures" => "XDG_PICTURES_DIR",
        "Music" => "XDG_MUSIC_DIR",
        "Videos" => "XDG_VIDEOS_DIR",
        "Desktop" => "XDG_DESKTOP_DIR",
        _ => return None,
    })
}

/// The folder `user-dirs.dirs` (its text) gives for `key`, below `home`. `None` when the file
/// does not name it; `Some(None)` when it turns the folder off by naming the home itself.
pub fn user_dir(text: &str, key: &str, home: &Path) -> Option<Option<PathBuf>> {
    let value = text.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
        Some(rest.trim().trim_matches('"').to_string())
    })?;
    let path = if let Some(below) = value.strip_prefix("$HOME") {
        home.join(below.trim_start_matches('/'))
    } else if value.starts_with('/') {
        PathBuf::from(value)
    } else {
        // Only "$HOME/..." and absolute paths are allowed in the file.
        return Some(None);
    };
    let same_as_home = path.components().eq(home.components());
    Some((!same_as_home).then_some(path))
}

/// A user's identity, from the password database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub uid: u32,
    pub gid: u32,
    pub name: String,
    pub home: PathBuf,
}

impl User {
    /// The user with `uid`.
    pub fn from_uid(uid: u32) -> io::Result<User> {
        let mut buffer = vec![0u8; 16 * 1024];
        // SAFETY: passwd is plain data that getpwuid_r fills in.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: the entry, a buffer of the length passed, and an out pointer.
        let code = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code));
        }
        if found.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no user has the ID {uid}"),
            ));
        }
        // SAFETY: getpwuid_r pointed these into the buffer, which is still alive.
        let (name, home) = unsafe {
            (
                CStr::from_ptr(entry.pw_name).to_string_lossy().into_owned(),
                CStr::from_ptr(entry.pw_dir).to_string_lossy().into_owned(),
            )
        };
        Ok(User {
            uid,
            gid: entry.pw_gid,
            name,
            home: PathBuf::from(home),
        })
    }

    /// The groups the user is in.
    fn groups(&self) -> io::Result<Vec<libc::gid_t>> {
        let name = CString::new(self.name.clone())
            .map_err(|_| io::Error::other("the user's name has a NUL"))?;
        let mut count: libc::c_int = 64;
        loop {
            let mut groups = vec![0 as libc::gid_t; count as usize];
            let asked = count;
            // SAFETY: a NUL-terminated name, a buffer of `count` groups, and the count.
            let result = unsafe {
                libc::getgrouplist(name.as_ptr(), self.gid, groups.as_mut_ptr(), &mut count)
            };
            if result >= 0 {
                groups.truncate(count as usize);
                return Ok(groups);
            }
            if count <= asked {
                count = asked * 2;
            }
            if count > 65_536 {
                return Err(io::Error::other("the user is in too many groups"));
            }
        }
    }
}

/// The folders of `user`, shared with their Android.
pub struct UserFolders {
    user: User,
}

impl UserFolders {
    pub fn new(user: User) -> UserFolders {
        UserFolders { user }
    }

    /// The folders of the user with `uid`.
    pub fn for_uid(uid: u32) -> io::Result<UserFolders> {
        Ok(UserFolders::new(User::from_uid(uid)?))
    }

    fn user_dirs_file(&self) -> PathBuf {
        // XDG_CONFIG_HOME is the user's own variable; the service sees it only when it runs
        // as that user.
        let config = (current_uid() == self.user.uid)
            .then(|| std::env::var_os("XDG_CONFIG_HOME"))
            .flatten()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| self.user.home.join(".config"));
        config.join("user-dirs.dirs")
    }
}

fn current_uid() -> u32 {
    // SAFETY: geteuid cannot fail.
    unsafe { libc::geteuid() }
}

/// Changes this thread's file system identity (and only this thread's), and puts the old one
/// back when dropped.
struct ActingAs {
    groups: Vec<libc::gid_t>,
}

impl ActingAs {
    fn begin(user: &User) -> io::Result<ActingAs> {
        // SAFETY: getgroups with a zero size returns the count.
        let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
        let mut saved = vec![0 as libc::gid_t; count.max(0) as usize];
        // SAFETY: a buffer of `count` groups.
        let got = unsafe { libc::getgroups(count, saved.as_mut_ptr()) };
        saved.truncate(got.max(0) as usize);
        let groups = user.groups()?;
        // The raw system call changes this thread's groups alone; glibc's setgroups would
        // change every thread's.
        // SAFETY: a buffer of `groups.len()` group IDs.
        let set = unsafe { libc::syscall(libc::SYS_setgroups, groups.len(), groups.as_ptr()) };
        if set != 0 {
            return Err(io::Error::last_os_error());
        }
        let acting = ActingAs { groups: saved };
        // setfsgid and setfsuid change the calling thread only. They return the previous
        // value, and success is checked by asking again.
        // SAFETY: plain identity changes for this thread.
        unsafe {
            libc::setfsgid(user.gid);
            libc::setfsuid(user.uid);
            if libc::setfsuid(u32::MAX) as u32 != user.uid
                || libc::setfsgid(u32::MAX) as u32 != user.gid
            {
                return Err(io::Error::other("the user's file system identity was refused"));
            }
        }
        Ok(acting)
    }
}

impl Drop for ActingAs {
    fn drop(&mut self) {
        // SAFETY: back to this process's own identity, for this thread.
        unsafe {
            libc::setfsuid(libc::geteuid());
            libc::setfsgid(libc::getegid());
            libc::syscall(libc::SYS_setgroups, self.groups.len(), self.groups.as_ptr());
        }
    }
}

impl ShareHost for UserFolders {
    fn folder(&self, name: &str) -> Option<PathBuf> {
        let key = xdg_key(name)?;
        let text = std::fs::read_to_string(self.user_dirs_file()).unwrap_or_default();
        match user_dir(&text, key, &self.user.home) {
            Some(folder) => folder,
            // Without a user-dirs file the folders have their English names.
            None => Some(self.user.home.join(name)),
        }
    }

    fn as_user(&self, serve: &mut dyn FnMut()) -> Result<(), String> {
        if current_uid() == self.user.uid {
            serve();
            return Ok(());
        }
        let acting = ActingAs::begin(&self.user)
            .map_err(|e| format!("cannot act as {}: {e}", self.user.name))?;
        serve();
        drop(acting);
        Ok(())
    }

    fn space(&self, path: &Path) -> Option<(u64, u64)> {
        let path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
        // SAFETY: statvfs is plain data that the call fills in.
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated path and the structure.
        if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
            return None;
        }
        let block = stats.f_frsize as u64;
        Some((
            (stats.f_blocks as u64).saturating_mul(block),
            (stats.f_bavail as u64).saturating_mul(block),
        ))
    }

    fn location(&self) -> String {
        LOCATION.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIRS: &str = r#"
# This file is written by xdg-user-dirs-update
XDG_DESKTOP_DIR="$HOME/바탕화면"
XDG_DOWNLOAD_DIR="$HOME/Downloads"
XDG_MUSIC_DIR="$HOME"
XDG_PICTURES_DIR="/data/photos"
XDG_VIDEOS_DIR="videos"
"#;

    #[test]
    fn fr29_the_shared_folders_are_the_users_xdg_folders() {
        let home = Path::new("/home/ada");
        let get = |name| user_dir(DIRS, xdg_key(name).unwrap(), home);
        assert_eq!(get("Desktop"), Some(Some(home.join("바탕화면"))));
        assert_eq!(get("Downloads"), Some(Some(home.join("Downloads"))));
        assert_eq!(get("Pictures"), Some(Some(PathBuf::from("/data/photos"))));
        // A folder that points at the home is turned off, and a relative one is refused.
        assert_eq!(get("Music"), Some(None));
        assert_eq!(get("Videos"), Some(None));
        assert_eq!(get("Documents"), None);
    }

    #[test]
    fn fr29_only_the_named_folders_are_shared() {
        assert_eq!(xdg_key("Documents"), Some("XDG_DOCUMENTS_DIR"));
        assert_eq!(xdg_key("etc"), None);
        assert_eq!(xdg_key("../Documents"), None);
    }

    #[test]
    fn fr29_the_service_runs_as_the_user_it_serves_without_changing_identity() {
        let me = User::from_uid(current_uid()).unwrap();
        let folders = UserFolders::new(me);
        let mut ran = false;
        folders.as_user(&mut || ran = true).unwrap();
        assert!(ran);
        assert_eq!(folders.location(), "Linux");
        assert!(folders.space(Path::new("/")).is_some());
    }
}
