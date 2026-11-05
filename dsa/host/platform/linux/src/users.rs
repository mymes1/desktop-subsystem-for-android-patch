//! The users the service acts for: their accounts, and running programs and making files as
//! them.
//!
//! The service runs as root, as the one system service every user's `dsa` talks to, so it can
//! serve the vsock ports every guest shares and make each user's network. Everything that
//! belongs to a user runs as that user: their VM's QEMU and passt, with only the rights the
//! user has, and the files in their home are theirs.

use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The user ID in a local API identity, `uid:<n>`.
pub fn uid_of(user: &str) -> Result<u32, String> {
    user.strip_prefix("uid:")
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("{user} is not a Unix user"))
}

/// A user's account, as the system's user database has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub uid: u32,
    pub gid: u32,
    pub name: String,
    pub home: PathBuf,
    /// Every group the user is in, their own included.
    pub groups: Vec<u32>,
}

/// A step a child runs before it becomes the user, such as placing a file descriptor.
pub type BeforeExec = Box<dyn Fn() -> io::Result<()> + Send + Sync>;

/// Whether this process runs as root.
pub fn is_root() -> bool {
    // SAFETY: no preconditions.
    unsafe { libc::geteuid() == 0 }
}

impl Account {
    /// The account of user `uid`.
    pub fn of_uid(uid: u32) -> Result<Account, String> {
        let mut buffer = vec![0u8; 16 * 1024];
        // SAFETY: passwd is plain data that getpwuid_r fills in.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: the buffer outlives the call and the strings it points into are copied out
        // before it is dropped.
        let result = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if result != 0 || found.is_null() {
            return Err(format!("user {uid} has no account on this computer"));
        }
        // SAFETY: getpwuid_r set these to NUL-terminated strings in the buffer.
        let (name, home) = unsafe {
            (
                CStr::from_ptr(entry.pw_name).to_string_lossy().into_owned(),
                PathBuf::from(CStr::from_ptr(entry.pw_dir).to_string_lossy().into_owned()),
            )
        };
        let gid = entry.pw_gid;
        let groups = groups_of(&name, gid);
        Ok(Account {
            uid,
            gid,
            name,
            home,
            groups,
        })
    }

    /// The account this process runs as.
    pub fn current() -> Result<Account, String> {
        // SAFETY: no preconditions.
        Account::of_uid(unsafe { libc::geteuid() })
    }

    /// Makes `command` run as this user, when this process is root, after `before`. Its
    /// environment says who it is, and nothing else of the service's carries over.
    pub fn run_as(&self, command: &mut Command, before: Option<BeforeExec>) {
        command
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.home)
            .env("USER", &self.name)
            .env("LOGNAME", &self.name)
            .env("XDG_RUNTIME_DIR", format!("/run/user/{}", self.uid));
        let drop_to =
            (is_root() && self.uid != 0).then(|| (self.uid, self.gid, self.groups.clone()));
        let step = move || -> io::Result<()> {
            if let Some(before) = &before {
                before()?;
            }
            if let Some((uid, gid, groups)) = &drop_to {
                // SAFETY: setgroups, setgid and setuid are async-signal-safe, and the group
                // list was made before the fork.
                unsafe {
                    if libc::setgroups(groups.len(), groups.as_ptr()) != 0
                        || libc::setgid(*gid) != 0
                        || libc::setuid(*uid) != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
            }
            Ok(())
        };
        // SAFETY: the step only makes async-signal-safe calls.
        unsafe {
            command.pre_exec(step);
        }
    }

    /// Gives `path` to this user, when this process is root.
    pub fn own(&self, path: &Path) -> Result<(), String> {
        if !is_root() {
            return Ok(());
        }
        std::os::unix::fs::lchown(path, Some(self.uid), Some(self.gid))
            .map_err(|e| format!("giving {} to {}: {e}", path.display(), self.name))
    }

    /// Makes the folder `dir` and every missing one above it inside the user's home, each
    /// owned by the user.
    pub fn make_dir(&self, dir: &Path) -> Result<(), String> {
        let mut missing = Vec::new();
        let mut at = Some(dir);
        while let Some(path) = at {
            if path.exists() {
                break;
            }
            missing.push(path.to_path_buf());
            at = path.parent();
        }
        for path in missing.iter().rev() {
            std::fs::create_dir(path)
                .or_else(|e| {
                    if e.kind() == io::ErrorKind::AlreadyExists {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })
                .map_err(|e| format!("creating {}: {e}", path.display()))?;
            if path.starts_with(&self.home) {
                self.own(path)?;
            }
        }
        Ok(())
    }

    /// Writes `contents` to `path` as a file only this user may read.
    pub fn write_private(&self, path: &Path, contents: &[u8]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(path);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("creating {}: {e}", path.display()))?;
        file.write_all(contents)
            .map_err(|e| format!("writing {}: {e}", path.display()))?;
        self.own(path)
    }
}

/// Every group `name` is in, `gid` first.
fn groups_of(name: &str, gid: u32) -> Vec<u32> {
    let Ok(cname) = CString::new(name) else {
        return vec![gid];
    };
    let mut count: libc::c_int = 64;
    loop {
        let mut groups: Vec<libc::gid_t> = vec![0; count as usize];
        let asked = count;
        // SAFETY: the buffer holds `count` groups; getgrouplist says how many it needed.
        let result =
            unsafe { libc::getgrouplist(cname.as_ptr(), gid, groups.as_mut_ptr(), &mut count) };
        if result >= 0 {
            groups.truncate(count as usize);
            return groups;
        }
        if count <= asked {
            return vec![gid];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr19_a_local_api_identity_names_the_users_id() {
        assert_eq!(uid_of("uid:1000"), Ok(1000));
        assert!(uid_of("S-1-5-21").is_err());
        assert!(uid_of("uid:me").is_err());
    }

    #[test]
    fn fr19_the_current_users_account_is_found() {
        let me = Account::current().unwrap();
        // SAFETY: no preconditions.
        assert_eq!(me.uid, unsafe { libc::geteuid() });
        assert!(me.groups.contains(&me.gid));
        assert!(me.home.is_absolute());
    }
}
