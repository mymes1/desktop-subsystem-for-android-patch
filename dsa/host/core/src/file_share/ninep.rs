//! A 9P2000.L file server over host folders, as the Linux kernel's `9p` file system uses it.
//!
//! Each connection attaches to one folder (the attach's `aname`) and never leaves it. Every
//! name that comes from the guest is checked on its own: a name with a folder separator, `.`,
//! `..`, or a character the host's file systems treat specially is refused. Every path is
//! resolved on the host before it is used, so a symbolic link (or a junction) that points
//! outside the folder is refused as well. A folder the user stops sharing is closed to the
//! guest from the next request on.
//!
//! What the guest is told of ownership is fixed: every file belongs to [`GUEST_OWNER`] with
//! group read and write, which is how Android's own shared storage keeps its files, so the
//! guest's storage permissions decide which apps reach them. Ownership and mode changes are
//! taken and ignored. Links, device nodes and extended attributes are not offered.

use std::collections::HashMap;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The largest message either side sends: 512 KiB, the most the kernel asks for over a
/// stream.
pub const MAX_MESSAGE: u32 = 512 * 1024;

/// The owner and group the guest sees every shared file under: Android's `media_rw`, which
/// owns the files of its shared storage.
pub const GUEST_OWNER: u32 = 1023;

/// The space a reply takes besides the data of a read: size, type, tag and count.
const READ_HEADER: u32 = 11;

/// Message types.
mod kind {
    pub const RLERROR: u8 = 7;
    pub const TSTATFS: u8 = 8;
    pub const TLOPEN: u8 = 12;
    pub const TLCREATE: u8 = 14;
    pub const TSYMLINK: u8 = 16;
    pub const TMKNOD: u8 = 18;
    pub const TRENAME: u8 = 20;
    pub const TREADLINK: u8 = 22;
    pub const TGETATTR: u8 = 24;
    pub const TSETATTR: u8 = 26;
    pub const TXATTRWALK: u8 = 30;
    pub const TXATTRCREATE: u8 = 32;
    pub const TREADDIR: u8 = 40;
    pub const TFSYNC: u8 = 50;
    pub const TLOCK: u8 = 52;
    pub const TGETLOCK: u8 = 54;
    pub const TLINK: u8 = 70;
    pub const TMKDIR: u8 = 72;
    pub const TRENAMEAT: u8 = 74;
    pub const TUNLINKAT: u8 = 76;
    pub const TVERSION: u8 = 100;
    pub const TAUTH: u8 = 102;
    pub const TATTACH: u8 = 104;
    pub const TFLUSH: u8 = 108;
    pub const TWALK: u8 = 110;
    pub const TREAD: u8 = 116;
    pub const TWRITE: u8 = 118;
    pub const TCLUNK: u8 = 120;
    pub const TREMOVE: u8 = 122;
}

/// Linux error numbers, which 9P2000.L carries.
pub mod errno {
    pub const EPERM: u32 = 1;
    pub const ENOENT: u32 = 2;
    pub const EIO: u32 = 5;
    pub const EBADF: u32 = 9;
    pub const EACCES: u32 = 13;
    pub const EBUSY: u32 = 16;
    pub const EEXIST: u32 = 17;
    pub const ENOTDIR: u32 = 20;
    pub const EISDIR: u32 = 21;
    pub const EINVAL: u32 = 22;
    pub const ENOSPC: u32 = 28;
    pub const EROFS: u32 = 30;
    pub const ENAMETOOLONG: u32 = 36;
    pub const ENOTEMPTY: u32 = 39;
    pub const EPROTO: u32 = 71;
    pub const EOPNOTSUPP: u32 = 95;
}

/// Linux open flags, as Tlopen and Tlcreate carry them.
const O_ACCMODE: u32 = 0o3;
const O_WRONLY: u32 = 0o1;
const O_RDWR: u32 = 0o2;
const O_CREAT: u32 = 0o100;
const O_EXCL: u32 = 0o200;
const O_TRUNC: u32 = 0o1000;
const O_APPEND: u32 = 0o2000;

const AT_REMOVEDIR: u32 = 0x200;

const QID_DIR: u8 = 0x80;
const QID_FILE: u8 = 0;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;

/// Tsetattr's valid bits.
const SETATTR_SIZE: u32 = 0x8;
const SETATTR_MTIME: u32 = 0x20;
const SETATTR_MTIME_SET: u32 = 0x100;

/// Rgetattr's valid bits: the basic set.
const GETATTR_BASIC: u64 = 0x7ff;

/// The longest name a guest file system takes.
const MAX_NAME: usize = 255;

/// The magic number Linux reports for a 9p file system.
const V9FS_MAGIC: u32 = 0x0102_1997;

type Errno = u32;

/// A file the guest refers to by a number of its choosing.
struct Fid {
    /// The folder the connection attached to, as the host resolves it.
    root: PathBuf,
    /// The name it was attached by.
    share: String,
    /// The names from the folder to the file.
    names: Vec<String>,
    open: Option<Open>,
}

enum Open {
    File(File),
    /// A folder's entries as they were when it was opened, with `.` and `..` first.
    Dir(Vec<Entry>),
}

struct Entry {
    qid: Qid,
    kind: u8,
    name: String,
}

#[derive(Clone, Copy)]
struct Qid {
    kind: u8,
    version: u32,
    path: u64,
}

/// One connection's server.
pub struct Server<'a> {
    roots: &'a dyn Fn(&str) -> Option<PathBuf>,
    still_shared: &'a dyn Fn(&str) -> bool,
    space: &'a dyn Fn(&Path) -> Option<(u64, u64)>,
    msize: u32,
    fids: HashMap<u32, Fid>,
}

impl<'a> Server<'a> {
    /// A server whose folders `roots` finds (`None` for a folder that is not shared), that
    /// `still_shared` says are shared from one request to the next, and whose volumes' size
    /// `space` tells.
    pub fn new(
        roots: &'a dyn Fn(&str) -> Option<PathBuf>,
        still_shared: &'a dyn Fn(&str) -> bool,
        space: &'a dyn Fn(&Path) -> Option<(u64, u64)>,
    ) -> Server<'a> {
        Server {
            roots,
            still_shared,
            space,
            msize: MAX_MESSAGE,
            fids: HashMap::new(),
        }
    }

    /// Answers one whole request (its size field included) with one whole reply.
    pub fn handle(&mut self, request: &[u8]) -> Vec<u8> {
        let mut input = Input::new(request);
        let (Ok(_size), Ok(kind), Ok(tag)) = (input.u32(), input.u8(), input.u16()) else {
            return error_reply(0xffff, errno::EPROTO);
        };
        match self.answer(kind, &mut input) {
            Ok(body) => reply(kind + 1, tag, &body),
            Err(code) => error_reply(tag, code),
        }
    }

    fn answer(&mut self, kind: u8, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        if kind != kind::TVERSION && kind != kind::TATTACH && kind != kind::TFLUSH {
            // A folder the user no longer shares answers nothing more.
            if let Ok(fid) = input.peek_u32() {
                if let Some(open) = self.fids.get(&fid) {
                    if !(self.still_shared)(&open.share) {
                        return Err(errno::EACCES);
                    }
                }
            }
        }
        match kind {
            kind::TVERSION => self.version(input),
            kind::TAUTH => Err(errno::EOPNOTSUPP),
            kind::TATTACH => self.attach(input),
            kind::TFLUSH => Ok(Vec::new()),
            kind::TWALK => self.walk(input),
            kind::TGETATTR => self.getattr(input),
            kind::TSETATTR => self.setattr(input),
            kind::TLOPEN => self.lopen(input),
            kind::TLCREATE => self.lcreate(input),
            kind::TREAD => self.read(input),
            kind::TWRITE => self.write(input),
            kind::TREADDIR => self.readdir(input),
            kind::TCLUNK => {
                let fid = input.u32()?;
                self.fids
                    .remove(&fid)
                    .map(|_| Vec::new())
                    .ok_or(errno::EBADF)
            }
            kind::TREMOVE => {
                let fid = input.u32()?;
                let path = self.resolve(fid);
                self.fids.remove(&fid);
                remove(&path?)?;
                Ok(Vec::new())
            }
            kind::TMKDIR => self.mkdir(input),
            kind::TUNLINKAT => self.unlinkat(input),
            kind::TRENAMEAT => self.renameat(input),
            kind::TRENAME => self.rename(input),
            kind::TSTATFS => self.statfs(input),
            kind::TFSYNC => {
                let fid = input.u32()?;
                if let Some(Open::File(file)) =
                    self.fids.get(&fid).ok_or(errno::EBADF)?.open.as_ref()
                {
                    file.sync_all().map_err(errno_of)?;
                }
                Ok(Vec::new())
            }
            kind::TLOCK => {
                // Locks are the guest's own business; the host's files are not locked for it.
                self.known(input.u32()?)?;
                Ok(vec![0])
            }
            kind::TGETLOCK => self.getlock(input),
            kind::TXATTRWALK | kind::TXATTRCREATE => Err(errno::EOPNOTSUPP),
            kind::TREADLINK => Err(errno::EINVAL),
            kind::TSYMLINK | kind::TMKNOD | kind::TLINK => Err(errno::EPERM),
            _ => Err(errno::EOPNOTSUPP),
        }
    }

    fn known(&self, fid: u32) -> Result<&Fid, Errno> {
        self.fids.get(&fid).ok_or(errno::EBADF)
    }

    fn version(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let msize = input.u32()?;
        let version = input.string()?;
        self.fids.clear();
        self.msize = msize.clamp(4096, MAX_MESSAGE);
        let mut out = Output::default();
        out.u32(self.msize);
        out.string(if version.starts_with("9P2000.L") {
            "9P2000.L"
        } else {
            "unknown"
        });
        Ok(out.0)
    }

    fn attach(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let _afid = input.u32()?;
        let _uname = input.string()?;
        let share = input.string()?;
        let folder = (self.roots)(&share).ok_or(errno::EACCES)?;
        let root = std::fs::canonicalize(&folder).map_err(errno_of)?;
        let metadata = std::fs::metadata(&root).map_err(errno_of)?;
        if !metadata.is_dir() {
            return Err(errno::ENOTDIR);
        }
        let qid = qid_of(&root, &metadata);
        self.fids.insert(
            fid,
            Fid {
                root,
                share,
                names: Vec::new(),
                open: None,
            },
        );
        let mut out = Output::default();
        out.qid(qid);
        Ok(out.0)
    }

    /// The host path of `fid`, resolved, and checked to be inside its folder.
    fn resolve(&self, fid: u32) -> Result<PathBuf, Errno> {
        let fid = self.known(fid)?;
        inside(&fid.root, &joined(&fid.root, &fid.names))
    }

    fn walk(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let newfid = input.u32()?;
        let count = input.u16()?;
        let mut names = Vec::with_capacity(count as usize);
        for _ in 0..count {
            names.push(input.string()?);
        }
        let start = self.known(fid)?;
        if newfid != fid && self.fids.contains_key(&newfid) {
            return Err(errno::EBADF);
        }
        let (root, share) = (start.root.clone(), start.share.clone());
        let mut path_names = start.names.clone();
        let mut qids = Vec::new();
        for name in &names {
            let refused = check_name(name).and_then(|()| {
                let mut next = path_names.clone();
                next.push(name.clone());
                let path = inside(&root, &joined(&root, &next))?;
                let metadata = std::fs::metadata(&path).map_err(errno_of)?;
                Ok((next, qid_of(&path, &metadata)))
            });
            match refused {
                Ok((next, qid)) => {
                    path_names = next;
                    qids.push(qid);
                }
                // The first name failing fails the walk; a later one ends it short.
                Err(code) if qids.is_empty() => return Err(code),
                Err(_) => break,
            }
        }
        if qids.len() == names.len() {
            self.fids.insert(
                newfid,
                Fid {
                    root,
                    share,
                    names: path_names,
                    open: None,
                },
            );
        }
        let mut out = Output::default();
        out.u16(qids.len() as u16);
        for qid in qids {
            out.qid(qid);
        }
        Ok(out.0)
    }

    fn getattr(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let _mask = input.u64()?;
        let path = self.resolve(fid)?;
        let metadata = std::fs::metadata(&path).map_err(errno_of)?;
        let mut out = Output::default();
        out.u64(GETATTR_BASIC);
        out.qid(qid_of(&path, &metadata));
        out.u32(mode_of(&metadata));
        out.u32(GUEST_OWNER);
        out.u32(GUEST_OWNER);
        out.u64(1);
        out.u64(0);
        let size = if metadata.is_dir() {
            4096
        } else {
            metadata.len()
        };
        out.u64(size);
        out.u64(4096);
        out.u64(size.div_ceil(512));
        for time in [
            metadata.accessed().ok(),
            metadata.modified().ok(),
            metadata.modified().ok(),
            metadata.created().ok(),
        ] {
            let (seconds, nanos) = seconds_of(time);
            out.u64(seconds);
            out.u64(nanos);
        }
        out.u64(0);
        out.u64(0);
        Ok(out.0)
    }

    fn setattr(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let valid = input.u32()?;
        let _mode = input.u32()?;
        let _uid = input.u32()?;
        let _gid = input.u32()?;
        let size = input.u64()?;
        let _atime = (input.u64()?, input.u64()?);
        let mtime = (input.u64()?, input.u64()?);
        let path = self.resolve(fid)?;
        // A folder's times are the host's to keep.
        if valid & (SETATTR_SIZE | SETATTR_MTIME) == 0 || path.is_dir() {
            return Ok(Vec::new());
        }
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(errno_of)?;
        if valid & SETATTR_SIZE != 0 {
            file.set_len(size).map_err(errno_of)?;
        }
        if valid & SETATTR_MTIME != 0 {
            let when = if valid & SETATTR_MTIME_SET != 0 {
                UNIX_EPOCH + std::time::Duration::new(mtime.0, mtime.1.min(999_999_999) as u32)
            } else {
                SystemTime::now()
            };
            file.set_modified(when).map_err(errno_of)?;
        }
        Ok(Vec::new())
    }

    fn lopen(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let flags = input.u32()?;
        let path = self.resolve(fid)?;
        let metadata = std::fs::metadata(&path).map_err(errno_of)?;
        let open = if metadata.is_dir() {
            if flags & O_ACCMODE != 0 {
                return Err(errno::EISDIR);
            }
            Open::Dir(entries(&self.known(fid)?.root, &path)?)
        } else {
            Open::File(open_options(flags).open(&path).map_err(errno_of)?)
        };
        let qid = qid_of(&path, &metadata);
        self.fids.get_mut(&fid).ok_or(errno::EBADF)?.open = Some(open);
        let mut out = Output::default();
        out.qid(qid);
        out.u32(self.msize - READ_HEADER - 13);
        Ok(out.0)
    }

    fn lcreate(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let name = input.string()?;
        let flags = input.u32()?;
        let _mode = input.u32()?;
        let _gid = input.u32()?;
        check_name(&name)?;
        let dir = self.resolve(fid)?;
        let path = dir.join(&name);
        refuse_outside(&self.known(fid)?.root, &path)?;
        let file = open_options(flags | O_CREAT)
            .create_new(flags & O_EXCL != 0)
            .open(&path)
            .map_err(errno_of)?;
        let metadata = file.metadata().map_err(errno_of)?;
        let entry = self.fids.get_mut(&fid).ok_or(errno::EBADF)?;
        entry.names.push(name);
        entry.open = Some(Open::File(file));
        let mut out = Output::default();
        out.qid(qid_of(&path, &metadata));
        out.u32(self.msize - READ_HEADER - 13);
        Ok(out.0)
    }

    fn read(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let offset = input.u64()?;
        let count = input.u32()?.min(self.msize - READ_HEADER);
        let entry = self.fids.get_mut(&fid).ok_or(errno::EBADF)?;
        let Some(Open::File(file)) = entry.open.as_mut() else {
            return Err(errno::EBADF);
        };
        file.seek(SeekFrom::Start(offset)).map_err(errno_of)?;
        let mut data = vec![0u8; count as usize];
        let mut filled = 0;
        while filled < data.len() {
            match file.read(&mut data[filled..]) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(errno_of(error)),
            }
        }
        data.truncate(filled);
        let mut out = Output::default();
        out.u32(filled as u32);
        out.bytes(&data);
        Ok(out.0)
    }

    fn write(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let offset = input.u64()?;
        let count = input.u32()?;
        let data = input.take(count as usize)?;
        let entry = self.fids.get_mut(&fid).ok_or(errno::EBADF)?;
        let Some(Open::File(file)) = entry.open.as_mut() else {
            return Err(errno::EBADF);
        };
        file.seek(SeekFrom::Start(offset)).map_err(errno_of)?;
        file.write_all(data).map_err(errno_of)?;
        let mut out = Output::default();
        out.u32(count);
        Ok(out.0)
    }

    fn readdir(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let offset = input.u64()?;
        let count = input.u32()?.min(self.msize - READ_HEADER) as usize;
        let entry = self.fids.get(&fid).ok_or(errno::EBADF)?;
        let Some(Open::Dir(list)) = entry.open.as_ref() else {
            return Err(errno::ENOTDIR);
        };
        let mut data = Output::default();
        for (index, item) in list.iter().enumerate().skip(offset as usize) {
            let size = 13 + 8 + 1 + 2 + item.name.len();
            if data.0.len() + size > count {
                break;
            }
            data.qid(item.qid);
            data.u64(index as u64 + 1);
            data.u8(item.kind);
            data.string(&item.name);
        }
        let mut out = Output::default();
        out.u32(data.0.len() as u32);
        out.bytes(&data.0);
        Ok(out.0)
    }

    fn mkdir(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let name = input.string()?;
        let _mode = input.u32()?;
        let _gid = input.u32()?;
        check_name(&name)?;
        let path = self.resolve(fid)?.join(&name);
        std::fs::create_dir(&path).map_err(errno_of)?;
        let metadata = std::fs::metadata(&path).map_err(errno_of)?;
        let mut out = Output::default();
        out.qid(qid_of(&path, &metadata));
        Ok(out.0)
    }

    fn unlinkat(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let name = input.string()?;
        let flags = input.u32()?;
        check_name(&name)?;
        let root = self.known(fid)?.root.clone();
        let path = inside(&root, &self.resolve(fid)?.join(&name))?;
        let metadata = std::fs::metadata(&path).map_err(errno_of)?;
        if flags & AT_REMOVEDIR != 0 {
            if !metadata.is_dir() {
                return Err(errno::ENOTDIR);
            }
            std::fs::remove_dir(&path).map_err(errno_of)?;
        } else {
            if metadata.is_dir() {
                return Err(errno::EISDIR);
            }
            std::fs::remove_file(&path).map_err(errno_of)?;
        }
        Ok(Vec::new())
    }

    fn renameat(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let old_dir = input.u32()?;
        let old_name = input.string()?;
        let new_dir = input.u32()?;
        let new_name = input.string()?;
        check_name(&old_name)?;
        check_name(&new_name)?;
        let from = self.resolve(old_dir)?.join(&old_name);
        let to = self.resolve(new_dir)?.join(&new_name);
        self.rename_paths(old_dir, &from, new_dir, &to)?;
        Ok(Vec::new())
    }

    fn rename(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let new_dir = input.u32()?;
        let name = input.string()?;
        check_name(&name)?;
        let from = self.resolve(fid)?;
        let to = self.resolve(new_dir)?.join(&name);
        self.rename_paths(fid, &from, new_dir, &to)?;
        let mut names = self.known(new_dir)?.names.clone();
        names.push(name);
        if let Some(entry) = self.fids.get_mut(&fid) {
            entry.names = names;
        }
        Ok(Vec::new())
    }

    fn rename_paths(
        &self,
        from_fid: u32,
        from: &Path,
        to_fid: u32,
        to: &Path,
    ) -> Result<(), Errno> {
        let (from_root, to_root) = (&self.known(from_fid)?.root, &self.known(to_fid)?.root);
        // Both ends stay in one folder: a file never moves from one shared folder to another
        // behind the guest's back.
        if from_root != to_root {
            return Err(errno::EACCES);
        }
        inside(from_root, from)?;
        refuse_outside(to_root, to)?;
        std::fs::rename(from, to).map_err(errno_of)
    }

    fn statfs(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let path = self.resolve(fid)?;
        let (total, free) = (self.space)(&path).unwrap_or((1 << 40, 1 << 39));
        let mut out = Output::default();
        out.u32(V9FS_MAGIC);
        out.u32(4096);
        out.u64(total / 4096);
        out.u64(free / 4096);
        out.u64(free / 4096);
        out.u64(1 << 20);
        out.u64(1 << 19);
        out.u64(0);
        out.u32(MAX_NAME as u32);
        Ok(out.0)
    }

    fn getlock(&mut self, input: &mut Input<'_>) -> Result<Vec<u8>, Errno> {
        let fid = input.u32()?;
        let _kind = input.u8()?;
        let start = input.u64()?;
        let length = input.u64()?;
        let proc_id = input.u32()?;
        let client = input.string()?;
        self.known(fid)?;
        let mut out = Output::default();
        // F_UNLCK: nothing on the host holds the range.
        out.u8(2);
        out.u64(start);
        out.u64(length);
        out.u32(proc_id);
        out.string(&client);
        Ok(out.0)
    }
}

/// Refuses a name the guest sends that is not one plain name in a folder: empty, `.` or
/// `..`, too long, with a folder separator, a drive or stream separator (`:`), a control
/// character or a character the host's file systems do not take, ending in a dot or a space
/// (which some host file systems drop, so two names would be one file), or a device name the
/// host reserves.
pub fn check_name(name: &str) -> Result<(), Errno> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(errno::EACCES);
    }
    if name.len() > MAX_NAME {
        return Err(errno::ENAMETOOLONG);
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || name.chars().any(|c| c.is_control())
        || name.ends_with('.')
        || name.ends_with(' ')
    {
        return Err(errno::EINVAL);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(errno::EINVAL);
    }
    Ok(())
}

fn joined(root: &Path, names: &[String]) -> PathBuf {
    let mut path = root.to_path_buf();
    for name in names {
        path.push(name);
    }
    path
}

/// `path` resolved on the host (every link followed), when that is inside `root`.
fn inside(root: &Path, path: &Path) -> Result<PathBuf, Errno> {
    let resolved = std::fs::canonicalize(path).map_err(errno_of)?;
    if resolved.starts_with(root) {
        Ok(resolved)
    } else {
        Err(errno::EACCES)
    }
}

/// Refuses `path` when it exists and resolves outside `root`, as a link that points out
/// would; a path that does not exist yet is fine, since its folder was checked.
fn refuse_outside(root: &Path, path: &Path) -> Result<(), Errno> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => inside(root, path).map(|_| ()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(errno_of(error)),
    }
}

fn remove(path: &Path) -> Result<(), Errno> {
    let metadata = std::fs::metadata(path).map_err(errno_of)?;
    if metadata.is_dir() {
        std::fs::remove_dir(path).map_err(errno_of)
    } else {
        std::fs::remove_file(path).map_err(errno_of)
    }
}

fn open_options(flags: u32) -> OpenOptions {
    let mut options = OpenOptions::new();
    match flags & O_ACCMODE {
        O_WRONLY => options.write(true),
        O_RDWR => options.read(true).write(true),
        _ => options.read(true),
    };
    if flags & (O_TRUNC | O_CREAT | O_APPEND) != 0 {
        options.write(true);
    }
    if flags & O_TRUNC != 0 {
        options.truncate(true);
    }
    if flags & O_CREAT != 0 {
        options.create(true);
    }
    options
}

/// A folder's entries, `.` and `..` first, leaving out any that cannot be reached from the
/// guest: names it could not use, and links that point outside the shared folder.
fn entries(root: &Path, dir: &Path) -> Result<Vec<Entry>, Errno> {
    let own = std::fs::metadata(dir).map_err(errno_of)?;
    let mut list = vec![
        Entry {
            qid: qid_of(dir, &own),
            kind: DT_DIR,
            name: ".".into(),
        },
        Entry {
            qid: qid_of(dir, &own),
            kind: DT_DIR,
            name: "..".into(),
        },
    ];
    let mut found = Vec::new();
    for item in std::fs::read_dir(dir).map_err(errno_of)? {
        let Ok(item) = item else { continue };
        let Some(name) = item.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if check_name(&name).is_err() {
            continue;
        }
        let path = item.path();
        let Ok(resolved) = inside(root, &path) else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(&resolved) else {
            continue;
        };
        found.push(Entry {
            qid: qid_of(&resolved, &metadata),
            kind: if metadata.is_dir() { DT_DIR } else { DT_REG },
            name,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    list.extend(found);
    Ok(list)
}

fn mode_of(metadata: &Metadata) -> u32 {
    let read_only = metadata.permissions().readonly();
    if metadata.is_dir() {
        S_IFDIR | if read_only { 0o550 } else { 0o770 }
    } else {
        S_IFREG | if read_only { 0o440 } else { 0o660 }
    }
}

/// A file's identity for the guest: a hash of where it is on the host, and its last change.
fn qid_of(path: &Path, metadata: &Metadata) -> Qid {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let (seconds, _) = seconds_of(metadata.modified().ok());
    Qid {
        kind: if metadata.is_dir() { QID_DIR } else { QID_FILE },
        version: seconds as u32,
        path: hash,
    }
}

fn seconds_of(time: Option<SystemTime>) -> (u64, u64) {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| (since.as_secs(), u64::from(since.subsec_nanos())))
        .unwrap_or((0, 0))
}

/// The Linux error number for a host error, from what kind of error it is.
fn errno_of(error: io::Error) -> Errno {
    use io::ErrorKind as Kind;
    match error.kind() {
        Kind::NotFound => errno::ENOENT,
        Kind::PermissionDenied => errno::EACCES,
        Kind::AlreadyExists => errno::EEXIST,
        Kind::InvalidInput => errno::EINVAL,
        Kind::NotADirectory => errno::ENOTDIR,
        Kind::IsADirectory => errno::EISDIR,
        Kind::DirectoryNotEmpty => errno::ENOTEMPTY,
        Kind::StorageFull => errno::ENOSPC,
        Kind::ReadOnlyFilesystem => errno::EROFS,
        Kind::ResourceBusy | Kind::ExecutableFileBusy => errno::EBUSY,
        _ => errno::EIO,
    }
}

fn reply(kind: u8, tag: u16, body: &[u8]) -> Vec<u8> {
    let size = (7 + body.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(body);
    out
}

fn error_reply(tag: u16, code: Errno) -> Vec<u8> {
    reply(kind::RLERROR, tag, &code.to_le_bytes())
}

/// Reads the fields of a request.
pub struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Input<'a> {
    pub fn new(bytes: &'a [u8]) -> Input<'a> {
        Input { bytes, at: 0 }
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8], Errno> {
        let end = self.at.checked_add(count).ok_or(errno::EPROTO)?;
        let slice = self.bytes.get(self.at..end).ok_or(errno::EPROTO)?;
        self.at = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8, Errno> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, Errno> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, Errno> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, Errno> {
        let b = self.take(8)?;
        let mut eight = [0u8; 8];
        eight.copy_from_slice(b);
        Ok(u64::from_le_bytes(eight))
    }

    fn peek_u32(&self) -> Result<u32, Errno> {
        Input {
            bytes: self.bytes,
            at: self.at,
        }
        .u32()
    }

    pub fn string(&mut self) -> Result<String, Errno> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| errno::EINVAL)
    }
}

/// Writes the fields of a message.
#[derive(Default)]
pub struct Output(pub Vec<u8>);

impl Output {
    pub fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    pub fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn string(&mut self, value: &str) {
        self.u16(value.len() as u16);
        self.0.extend_from_slice(value.as_bytes());
    }

    pub fn bytes(&mut self, value: &[u8]) {
        self.0.extend_from_slice(value);
    }

    fn qid(&mut self, qid: Qid) {
        self.u8(qid.kind);
        self.u32(qid.version);
        self.u64(qid.path);
    }

    /// A whole message of type `kind` with `tag`, from the fields written so far.
    pub fn message(self, kind: u8, tag: u16) -> Vec<u8> {
        reply(kind, tag, &self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr29_names_that_leave_a_folder_are_refused() {
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "..\\x",
            "C:",
            "file:stream",
            "con",
            "NUL.txt",
            "com1",
            "a.",
            "a ",
            "a\nb",
            "a*b",
        ] {
            assert!(check_name(bad).is_err(), "{bad:?} was accepted");
        }
        for good in [
            "photo.jpg",
            "..hidden",
            "\u{d55c}\u{ae00}.txt",
            "console.log",
            "com10",
        ] {
            assert!(check_name(good).is_ok(), "{good:?} was refused");
        }
        assert_eq!(check_name(&"a".repeat(256)), Err(errno::ENAMETOOLONG));
    }

    #[test]
    fn fr29_an_unknown_request_is_answered_with_an_error() {
        let roots = |_: &str| None;
        let shared = |_: &str| true;
        let space = |_: &Path| None;
        let mut server = Server::new(&roots, &shared, &space);
        let reply = server.handle(&Output::default().message(250, 9));
        assert_eq!(reply[4], kind::RLERROR);
        assert_eq!(u16::from_le_bytes([reply[5], reply[6]]), 9);
        assert_eq!(server.handle(&[1, 2]).get(4), Some(&kind::RLERROR));
    }
}
