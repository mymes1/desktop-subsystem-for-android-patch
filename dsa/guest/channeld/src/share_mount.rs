//! The host's shared folders in Android's shared storage.
//!
//! The host says which folders it shares (`FileSharing`); the daemon writes that to the
//! property [`PROPERTY`] ([`property_value`]), and init runs `dsa-channeld --share-mount` as
//! root, which mounts each folder with the kernel's 9p file system at
//! `/data/media/0/<location>/<folder>` (the location is [`LOCATION`]) and takes away the ones no longer shared ([`plan`]).
//! Android's shared storage (`/storage/emulated/0`) is served from `/data/media/0` by the
//! media provider, so a mounted folder shows there for every app, under the storage
//! permissions Android gives each app. Mounts made under `/data/media` reach the media
//! provider's view of it because the mount is shared with its namespace.
//!
//! Each mount is one connection to the host's file server: the daemon connects, proves it
//! knows the boot's secret, and hands the socket to the kernel (`trans=fd`), which speaks
//! 9P2000.L on it from then on. The folder names and the location come from the host, and
//! are checked here as well: each is one plain name, so a mount never lands outside the
//! storage.

use dsa_protocol::control::FileSharing;

/// The property that carries what the host shares: [`property_value`].
pub const PROPERTY: &str = "vendor.dsa.shares";

/// The storage the folders are mounted in.
pub const STORAGE: &str = "/data/media/0";

/// The owner and group of the mount points: Android's `media_rw`, which owns the storage.
pub const MEDIA_RW: u32 = 1023;

/// The folder the host's folders go under when the host names none: `Host`, on every host.
pub const LOCATION: &str = "Host";

/// Where earlier versions put the host's folders, one folder per kind of host. Each is
/// emptied of the mount points left in it and taken away once empty ([`leftovers`]).
pub const OLD_LOCATIONS: [&str; 3] = ["Windows", "Linux", "Mac"];

/// The names a host's shared folder has, which are the only mount points [`leftovers`] takes.
const FOLDER_NAMES: [&str; 6] = [
    "Documents",
    "Downloads",
    "Pictures",
    "Music",
    "Videos",
    "Desktop",
];

/// What the host shares, as the property carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub port: u32,
    pub location: String,
    pub folders: Vec<String>,
}

/// Whether `name` is one plain name, which a mount point may be.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

/// The property's value for `sharing`: `off`, or `<port> <location> <folder>,<folder>...`.
/// Names that are not plain are left out.
pub fn property_value(sharing: &FileSharing) -> String {
    let folders: Vec<&str> = sharing
        .folders
        .iter()
        .map(String::as_str)
        .filter(|name| plain_name(name) && !name.contains(' '))
        .collect();
    if !sharing.on
        || folders.is_empty()
        || !plain_name(&sharing.location)
        || sharing.location.contains(' ')
    {
        return "off".into();
    }
    format!(
        "{} {} {}",
        sharing.port,
        sharing.location,
        folders.join(",")
    )
}

/// What a property value asks for; `None` for `off` or a value that is not well formed.
pub fn parse(value: &str) -> Option<Wanted> {
    let mut parts = value.split_whitespace();
    let port = parts.next()?.parse::<u32>().ok()?;
    let location = parts.next()?.to_string();
    let folders: Vec<String> = parts
        .next()?
        .split(',')
        .filter(|name| plain_name(name))
        .map(str::to_string)
        .collect();
    (plain_name(&location) && !folders.is_empty() && parts.next().is_none()).then_some(Wanted {
        port,
        location,
        folders,
    })
}

/// The 9p mount points under `storage` in `mounts` (as `/proc/mounts` lists them).
pub fn mounted(mounts: &str, storage: &str) -> Vec<String> {
    let prefix = format!("{}/", storage.trim_end_matches('/'));
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (_source, target, kind) = (fields.next()?, fields.next()?, fields.next()?);
            // /proc/mounts writes a space in a path as \040.
            let target = target.replace("\\040", " ");
            (kind == "9p" && target.starts_with(&prefix)).then_some(target)
        })
        .collect()
}

/// What to do: the mount points to take away, and the folders to mount, with where.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub unmount: Vec<String>,
    pub mount: Vec<(String, String)>,
}

/// What brings `mounted` (the 9p mount points under `storage`) to `wanted`.
pub fn plan(wanted: Option<&Wanted>, mounted: &[String], storage: &str) -> Plan {
    let target = |location: &str, folder: &str| {
        format!("{}/{location}/{folder}", storage.trim_end_matches('/'))
    };
    let targets: Vec<(String, String)> = wanted
        .map(|wanted| {
            wanted
                .folders
                .iter()
                .map(|folder| (folder.clone(), target(&wanted.location, folder)))
                .collect()
        })
        .unwrap_or_default();
    Plan {
        unmount: mounted
            .iter()
            .filter(|path| !targets.iter().any(|(_, target)| target == *path))
            .cloned()
            .collect(),
        mount: targets
            .into_iter()
            .filter(|(_, target)| !mounted.contains(target))
            .collect(),
    }
}

/// The folders an earlier location left behind under `storage`, in the order they are taken
/// away: each old mount point, then the old location itself. `location` is the one in use,
/// which is never among them. Only empty folders are taken away, so a folder the user made
/// there, or put files in, stays.
pub fn leftovers(storage: &str, location: &str) -> Vec<String> {
    let storage = storage.trim_end_matches('/');
    OLD_LOCATIONS
        .iter()
        .filter(|old| **old != location)
        .flat_map(|old| {
            FOLDER_NAMES
                .iter()
                .map(move |folder| format!("{storage}/{old}/{folder}"))
                .chain(std::iter::once(format!("{storage}/{old}")))
        })
        .collect()
}

/// The mount options for a folder on the socket `fd`.
pub fn mount_options(fd: i32, folder: &str) -> String {
    format!(
        "trans=fd,rfdno={fd},wfdno={fd},version=9p2000.L,aname={folder},msize=524288,\
         access=client,cache=none,dfltuid={MEDIA_RW},dfltgid={MEDIA_RW}"
    )
}

/// Runs the mounts: `dsa-channeld --share-mount`, as init starts it. Returns the exit status.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn run(read_property: &dyn Fn(&str) -> String, say: &dyn Fn(&str)) -> i32 {
    // A value that changes while the mounts are made is acted on before this ends.
    let mut seen = None;
    loop {
        let value = read_property(PROPERTY);
        if seen.as_deref() == Some(value.as_str()) {
            return 0;
        }
        apply(parse(&value).as_ref(), say);
        seen = Some(value);
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn apply(wanted: Option<&Wanted>, say: &dyn Fn(&str)) {
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let now = mounted(&mounts, STORAGE);
    let plan = plan(wanted, &now, STORAGE);
    for target in &plan.unmount {
        match sys::unmount(target) {
            Ok(()) => {
                let _ = std::fs::remove_dir(target);
                say(&format!("took away the host's folder at {target}"));
            }
            Err(error) => say(&format!("could not take away {target}: {error}")),
        }
    }
    if wanted.is_none() {
        // Nothing is shared: the location folder goes too when it is empty.
        for target in &plan.unmount {
            if let Some(parent) = std::path::Path::new(target).parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
    // The folders an earlier version mounted under another name. Their mounts went with the
    // plan above, or with the last shutdown; the empty folders go now.
    let location = wanted.map_or(LOCATION, |wanted| wanted.location.as_str());
    for old in leftovers(STORAGE, location) {
        let path = std::path::Path::new(&old);
        if path.is_dir() && std::fs::remove_dir(path).is_ok() {
            say(&format!("took away the old folder {old}"));
        }
    }
    let Some(wanted) = wanted else {
        return;
    };
    let secret = match std::fs::read_to_string("/proc/bootconfig")
        .ok()
        .and_then(|text| dsa_protocol::auth::secret_from_bootconfig(&text))
    {
        Some(secret) => secret,
        None => {
            say("the boot has no secret, so no host folder can be mounted");
            return;
        }
    };
    for (folder, target) in &plan.mount {
        match sys::mount_share(wanted.port, folder, target, &secret) {
            Ok(()) => say(&format!("mounted the host's {folder} at {target}")),
            Err(error) => say(&format!("could not mount the host's {folder}: {error}")),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod sys {
    use std::ffi::CString;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::{chown, DirBuilderExt};

    use dsa_protocol::auth::BootSecret;

    /// The host, as a VM sees it.
    const HOST: u32 = 2;

    pub fn unmount(target: &str) -> io::Result<()> {
        let path = CString::new(target).map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: a NUL-terminated path.
        if unsafe { libc::umount2(path.as_ptr(), libc::MNT_DETACH) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn make_dir(path: &std::path::Path) -> io::Result<()> {
        if !path.is_dir() {
            std::fs::DirBuilder::new().mode(0o770).create(path)?;
        }
        chown(path, Some(super::MEDIA_RW), Some(super::MEDIA_RW))
    }

    fn connect(port: u32) -> io::Result<OwnedFd> {
        // SAFETY: plain socket creation; the result is checked.
        let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd was just made and nothing else owns it.
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: sockaddr_vm is plain data; zero is a valid start.
        let mut address: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
        address.svm_family = libc::AF_VSOCK as libc::sa_family_t;
        address.svm_cid = HOST;
        address.svm_port = port;
        // SAFETY: a valid address of the size passed.
        let connected = unsafe {
            libc::connect(
                socket.as_raw_fd(),
                (&address as *const libc::sockaddr_vm).cast(),
                std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        };
        if connected != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(socket)
    }

    pub fn mount_share(
        port: u32,
        folder: &str,
        target: &str,
        secret: &BootSecret,
    ) -> io::Result<()> {
        let target_path = std::path::Path::new(target);
        if let Some(location) = target_path.parent() {
            make_dir(location)?;
        }
        make_dir(target_path)?;
        let socket = connect(port)?;
        // The proof: the host's nonce, answered with the boot's secret.
        let mut stream = std::fs::File::from(socket.try_clone()?);
        let mut nonce = [0u8; 32];
        stream.read_exact(&mut nonce)?;
        stream.write_all(&dsa_host_proof(secret, &nonce))?;
        drop(stream);
        let source =
            CString::new(format!("dsa-{folder}")).map_err(|_| io::ErrorKind::InvalidInput)?;
        let target_c = CString::new(target).map_err(|_| io::ErrorKind::InvalidInput)?;
        let kind = CString::new("9p").expect("no NUL");
        let options = CString::new(super::mount_options(socket.as_raw_fd(), folder))
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: NUL-terminated strings; the kernel takes its own reference to the socket.
        let mounted = unsafe {
            libc::mount(
                source.as_ptr(),
                target_c.as_ptr(),
                kind.as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                options.as_ptr().cast(),
            )
        };
        if mounted != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The proof the host's file server checks, as the host computes it.
    fn dsa_host_proof(secret: &BootSecret, nonce: &[u8]) -> [u8; 32] {
        secret.proof(&[b"file-share:".as_slice(), nonce].concat())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sharing(on: bool, folders: &[&str]) -> FileSharing {
        FileSharing {
            on,
            port: 5590,
            location: "Host".into(),
            folders: folders.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn fr29_the_host_sharing_becomes_a_property_and_back() {
        let value = property_value(&sharing(true, &["Documents", "Pictures"]));
        assert_eq!(value, "5590 Host Documents,Pictures");
        assert_eq!(
            parse(&value),
            Some(Wanted {
                port: 5590,
                location: "Host".into(),
                folders: vec!["Documents".into(), "Pictures".into()],
            })
        );
        assert_eq!(property_value(&sharing(false, &["Documents"])), "off");
        assert_eq!(property_value(&sharing(true, &[])), "off");
        assert_eq!(parse("off"), None);
    }

    #[test]
    fn fr29_a_folder_name_that_leaves_the_storage_is_never_mounted() {
        assert_eq!(
            property_value(&sharing(true, &["..", "a/b", "Pictures"])),
            "5590 Host Pictures"
        );
        let mut outside = sharing(true, &["Pictures"]);
        outside.location = "../../system".into();
        assert_eq!(property_value(&outside), "off");
        assert_eq!(parse("5590 .. Pictures"), None);
        assert_eq!(
            parse("5590 Host ..,Pictures").map(|w| w.folders),
            Some(vec!["Pictures".to_string()])
        );
    }

    #[test]
    fn fr29_turning_sharing_off_takes_every_mount_away() {
        let mounts = "dsa-Documents /data/media/0/Host/Documents 9p rw 0 0\n\
                      dsa-Pictures /data/media/0/Host/Pictures 9p rw 0 0\n\
                      /dev/block/dm-5 /data ext4 rw 0 0\n";
        let now = mounted(mounts, STORAGE);
        assert_eq!(now.len(), 2);
        let off = plan(None, &now, STORAGE);
        assert_eq!(off.unmount, now);
        assert!(off.mount.is_empty());

        let wanted = parse("5590 Host Pictures,Music").unwrap();
        let changed = plan(Some(&wanted), &now, STORAGE);
        assert_eq!(
            changed.unmount,
            vec!["/data/media/0/Host/Documents".to_string()]
        );
        assert_eq!(
            changed.mount,
            vec![("Music".to_string(), "/data/media/0/Host/Music".to_string())]
        );
    }

    #[test]
    fn fr29_the_mount_hands_the_socket_to_the_kernel() {
        let options = mount_options(7, "Pictures");
        assert!(options.contains("trans=fd,rfdno=7,wfdno=7"), "{options}");
        assert!(options.contains("version=9p2000.L"), "{options}");
        assert!(options.contains("aname=Pictures"), "{options}");
    }

    #[test]
    fn fr29_the_old_windows_folder_is_moved_to_host() {
        // A guest that mounted the host's folders under Windows/ before: those mounts are taken
        // away, the new ones go under Host/, and the empty old folders go too.
        let mounts = "dsa-Documents /data/media/0/Windows/Documents 9p rw 0 0\n";
        let now = mounted(mounts, STORAGE);
        let wanted = parse("5590 Host Documents").unwrap();
        let moved = plan(Some(&wanted), &now, STORAGE);
        assert_eq!(
            moved.unmount,
            vec!["/data/media/0/Windows/Documents".to_string()]
        );
        assert_eq!(
            moved.mount,
            vec![(
                "Documents".to_string(),
                "/data/media/0/Host/Documents".to_string()
            )]
        );
        let old = leftovers(STORAGE, LOCATION);
        assert!(old.contains(&"/data/media/0/Windows/Documents".to_string()));
        assert!(old.contains(&"/data/media/0/Linux".to_string()));
        let windows = old
            .iter()
            .position(|p| p == "/data/media/0/Windows")
            .unwrap();
        let documents = old
            .iter()
            .position(|p| p == "/data/media/0/Windows/Documents")
            .unwrap();
        assert!(documents < windows, "a folder goes before the one it is in");
        assert!(old.iter().all(|p| !p.starts_with("/data/media/0/Host")));
    }
}
