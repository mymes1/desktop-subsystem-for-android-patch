//! Writing an erofs image: a read-only file system the guest's kernel mounts through a loop
//! device, so the host can hand the guest a tree of files as one checked file.
//!
//! The image is uncompressed and as plain as erofs allows: 4 KiB blocks, compact (32-byte)
//! inodes, and every file, folder and link stored whole in blocks of its own
//! (`EROFS_INODE_FLAT_PLAIN`). Nothing has extended attributes, and everything belongs to
//! root; the guest mounts the image with one SELinux context for all of it. That is enough for
//! the trees the host builds (an ARM translator's libraries and settings), and it keeps the
//! writer small and exact:
//!
//! ```text
//! block 0         the superblock, at byte 1024
//! blocks 1..=M    the inodes, 32 bytes each, inode N at byte 32 * N (N >= 1)
//! after them      each inode's data, in inode order, each starting on a block
//! ```
//!
//! A folder's data is its entries in name order, split into blocks: each block holds the
//! fixed-size entries first (inode number, where the name starts, the file type) and then
//! their names, without terminators.

use std::collections::BTreeMap;
use std::io::Write;

/// The signature the superblock starts with.
pub const MAGIC: u32 = 0xE0F5_E1E2;
/// Where the superblock is.
pub const SUPERBLOCK_OFFSET: usize = 1024;
/// The block size, as a power of two.
const BLOCK_BITS: u8 = 12;
const BLOCK: usize = 1 << BLOCK_BITS;
/// The size of a compact inode, which is also the unit inode numbers count in.
const INODE: usize = 32;
/// The size of a folder entry before the names.
const DIRENT: usize = 12;
/// The longest name a file may have.
const MAX_NAME: usize = 255;

const TYPE_FILE: u8 = 1;
const TYPE_FOLDER: u8 = 2;
const TYPE_LINK: u8 = 7;

const S_IFREG: u16 = 0o100_000;
const S_IFDIR: u16 = 0o040_000;
const S_IFLNK: u16 = 0o120_000;

#[derive(Debug, Clone)]
enum Kind {
    File(Vec<u8>),
    Link(String),
    Folder(BTreeMap<Vec<u8>, usize>),
}

#[derive(Debug, Clone)]
struct Node {
    kind: Kind,
    /// The permission bits.
    permissions: u16,
    parent: usize,
}

/// A tree of files to write as an erofs image.
#[derive(Debug, Clone)]
pub struct Builder {
    nodes: Vec<Node>,
}

impl Default for Builder {
    fn default() -> Self {
        Builder::new()
    }
}

/// Splits a relative path into its names, refusing what could leave the tree.
fn names(path: &str) -> Result<Vec<&str>, String> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(format!("{path:?} names no file"));
    }
    for part in &parts {
        let plain = *part != "." && *part != ".." && !part.contains('\0');
        if !plain || part.len() > MAX_NAME {
            return Err(format!("{path:?} is not a plain relative path"));
        }
    }
    Ok(parts)
}

fn type_of(kind: &Kind) -> u8 {
    match kind {
        Kind::File(_) => TYPE_FILE,
        Kind::Link(_) => TYPE_LINK,
        Kind::Folder(_) => TYPE_FOLDER,
    }
}

/// The inode number of the node at `index`: inode 0 is left unused.
fn nid(index: usize) -> u64 {
    index as u64 + 1
}

impl Builder {
    /// An empty tree: only the root folder.
    pub fn new() -> Builder {
        let root = Node {
            kind: Kind::Folder(BTreeMap::new()),
            permissions: 0o755,
            parent: 0,
        };
        Builder { nodes: vec![root] }
    }

    /// The child of the folder `parent` called `name`, if there is one.
    fn child(&self, parent: usize, name: &str) -> Result<Option<usize>, String> {
        match &self.nodes[parent].kind {
            Kind::Folder(children) => Ok(children.get(name.as_bytes()).copied()),
            _ => Err(format!("{name:?} is inside a file")),
        }
    }

    /// The folder at `parts`, made (with mode 0755) where it is missing.
    fn folder_at(&mut self, parts: &[&str]) -> Result<usize, String> {
        let mut at = 0;
        for part in parts {
            at = match self.child(at, part)? {
                Some(child) => match self.nodes[child].kind {
                    Kind::Folder(_) => child,
                    _ => return Err(format!("{part:?} is a file, not a folder")),
                },
                None => {
                    let folder = Kind::Folder(BTreeMap::new());
                    self.insert(at, part, folder, 0o755)?
                }
            };
        }
        Ok(at)
    }

    fn insert(
        &mut self,
        parent: usize,
        name: &str,
        kind: Kind,
        permissions: u16,
    ) -> Result<usize, String> {
        if self.child(parent, name)?.is_some() {
            return Err(format!("{name:?} is added twice"));
        }
        let index = self.nodes.len();
        if let Kind::Folder(children) = &mut self.nodes[parent].kind {
            children.insert(name.as_bytes().to_vec(), index);
        }
        self.nodes.push(Node {
            kind,
            permissions: permissions & 0o7777,
            parent,
        });
        Ok(index)
    }

    fn add(&mut self, path: &str, kind: Kind, permissions: u16) -> Result<(), String> {
        let parts = names(path)?;
        let (name, folders) = parts.split_last().expect("at least one name");
        let parent = self.folder_at(folders)?;
        self.insert(parent, name, kind, permissions)?;
        Ok(())
    }

    /// Adds a regular file with these permission bits, making its folders as needed.
    pub fn file(&mut self, path: &str, permissions: u16, data: Vec<u8>) -> Result<(), String> {
        if data.len() > u32::MAX as usize {
            return Err(format!("{path} is larger than 4 GiB"));
        }
        self.add(path, Kind::File(data), permissions)
    }

    /// Adds a symbolic link to `target`, making its folders as needed.
    pub fn link(&mut self, path: &str, target: &str) -> Result<(), String> {
        if target.is_empty() || target.len() >= BLOCK || target.contains('\0') {
            return Err(format!("{path} links to {target:?}, which is not usable"));
        }
        self.add(path, Kind::Link(target.to_string()), 0o777)
    }

    /// Adds a folder (mode 0755) and its parents, where they are missing.
    pub fn folder(&mut self, path: &str) -> Result<(), String> {
        let parts = names(path)?;
        self.folder_at(&parts)?;
        Ok(())
    }

    /// The paths of the regular files in the tree, in path order.
    pub fn files(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect(0, "", &mut out);
        out
    }

    fn collect(&self, index: usize, prefix: &str, out: &mut Vec<String>) {
        if let Kind::Folder(children) = &self.nodes[index].kind {
            for (name, &child) in children {
                let name = String::from_utf8_lossy(name);
                let path = format!("{prefix}{name}");
                match self.nodes[child].kind {
                    Kind::Folder(_) => self.collect(child, &format!("{path}/"), out),
                    Kind::File(_) => out.push(path),
                    Kind::Link(_) => {}
                }
            }
        }
    }

    /// A folder's data: its entries, block by block, the last block cut where it ends.
    fn folder_data(&self, index: usize, children: &BTreeMap<Vec<u8>, usize>) -> Vec<u8> {
        let parent = self.nodes[index].parent;
        let mut entries: Vec<(&[u8], u64, u8)> = vec![
            (b".", nid(index), TYPE_FOLDER),
            (b"..", nid(parent), TYPE_FOLDER),
        ];
        for (name, &child) in children {
            let kind = type_of(&self.nodes[child].kind);
            entries.push((name.as_slice(), nid(child), kind));
        }
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut data = Vec::new();
        let mut rest = &entries[..];
        while !rest.is_empty() {
            // As many entries as fit in one block with their names.
            let mut count = 0;
            let mut used = 0;
            while count < rest.len() && used + DIRENT + rest[count].0.len() <= BLOCK {
                used += DIRENT + rest[count].0.len();
                count += 1;
            }
            let (block, after) = rest.split_at(count);
            let start = data.len();
            let mut name_offset = DIRENT * block.len();
            for (name, inode, kind) in block {
                data.extend_from_slice(&inode.to_le_bytes());
                data.extend_from_slice(&(name_offset as u16).to_le_bytes());
                data.push(*kind);
                data.push(0);
                name_offset += name.len();
            }
            for (name, _, _) in block {
                data.extend_from_slice(name);
            }
            rest = after;
            if !rest.is_empty() {
                data.resize(start + BLOCK, 0);
            }
        }
        data
    }

    /// The data each node stores: a file's content, a link's target, a folder's entries.
    fn node_data(&self, index: usize) -> Vec<u8> {
        match &self.nodes[index].kind {
            Kind::File(data) => data.clone(),
            Kind::Link(target) => target.as_bytes().to_vec(),
            Kind::Folder(children) => self.folder_data(index, children),
        }
    }

    /// How many links a node has: a folder has one from its parent, its own `.`, and the
    /// `..` of each folder in it.
    fn link_count(&self, index: usize) -> u16 {
        let Kind::Folder(children) = &self.nodes[index].kind else {
            return 1;
        };
        let is_folder = |child: &usize| matches!(self.nodes[*child].kind, Kind::Folder(_));
        let folders = children.values().filter(|c| is_folder(c)).count();
        (2 + folders).min(usize::from(u16::MAX)) as u16
    }

    /// Writes the image to `out`, returning its length in bytes.
    pub fn write(&self, out: &mut dyn Write) -> Result<u64, String> {
        let count = self.nodes.len();
        if count >= usize::from(u16::MAX) {
            return Err(format!("{count} files are more than the image takes"));
        }
        let meta_blocks = ((count + 1) * INODE).div_ceil(BLOCK);
        let datas: Vec<Vec<u8>> = (0..count).map(|i| self.node_data(i)).collect();
        let mut block = 1 + meta_blocks as u64;
        let mut meta = vec![0u8; meta_blocks * BLOCK];
        for (index, node) in self.nodes.iter().enumerate() {
            let size = datas[index].len();
            let blocks = size.div_ceil(BLOCK) as u64;
            let start = if blocks == 0 { 0 } else { block };
            block += blocks;
            if block > u64::from(u32::MAX) {
                return Err("the image would be larger than erofs takes".into());
            }
            let format = match node.kind {
                Kind::File(_) => S_IFREG,
                Kind::Link(_) => S_IFLNK,
                Kind::Folder(_) => S_IFDIR,
            };
            let mode = format | node.permissions;
            let at = nid(index) as usize * INODE;
            let inode = &mut meta[at..at + INODE];
            // i_format 0: a compact inode whose data is in blocks of its own (FLAT_PLAIN), and
            // no extended attributes.
            inode[4..6].copy_from_slice(&mode.to_le_bytes());
            inode[6..8].copy_from_slice(&self.link_count(index).to_le_bytes());
            inode[8..12].copy_from_slice(&(size as u32).to_le_bytes());
            inode[16..20].copy_from_slice(&(start as u32).to_le_bytes());
            inode[20..24].copy_from_slice(&(nid(index) as u32).to_le_bytes());
        }
        let total_blocks = block;
        let mut head = vec![0u8; BLOCK];
        let end = SUPERBLOCK_OFFSET + 128;
        let superblock = &mut head[SUPERBLOCK_OFFSET..end];
        superblock[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        superblock[12] = BLOCK_BITS;
        superblock[14..16].copy_from_slice(&(nid(0) as u16).to_le_bytes());
        superblock[16..24].copy_from_slice(&(count as u64).to_le_bytes());
        superblock[36..40].copy_from_slice(&(total_blocks as u32).to_le_bytes());
        // The inodes start at block 1.
        superblock[40..44].copy_from_slice(&1u32.to_le_bytes());
        let io = |e: std::io::Error| format!("the image cannot be written: {e}");
        out.write_all(&head).map_err(io)?;
        out.write_all(&meta).map_err(io)?;
        for data in &datas {
            out.write_all(data).map_err(io)?;
            let pad = data.len().div_ceil(BLOCK) * BLOCK - data.len();
            out.write_all(&vec![0u8; pad]).map_err(io)?;
        }
        Ok(total_blocks * BLOCK as u64)
    }
}

/// What an image holds, read back the way the kernel reads it, for the tests of the writer and
/// of what is built with it.
#[cfg(test)]
pub(crate) mod read {
    use super::*;

    /// One thing in an image: its path, its mode (type and permissions), and its data (a
    /// link's target, a file's content, nothing for a folder).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Item {
        pub path: String,
        pub mode: u16,
        pub data: Vec<u8>,
    }

    fn u16_at(image: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([image[at], image[at + 1]])
    }

    fn u32_at(image: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(image[at..at + 4].try_into().unwrap())
    }

    fn inode(image: &[u8], number: u64) -> (u16, Vec<u8>) {
        let meta = u32_at(image, SUPERBLOCK_OFFSET + 40) as usize * BLOCK;
        let at = meta + number as usize * INODE;
        assert_eq!((u16_at(image, at) >> 1) & 7, 0, "flat plain");
        let mode = u16_at(image, at + 4);
        let size = u32_at(image, at + 8) as usize;
        let start = u32_at(image, at + 16) as usize * BLOCK;
        (mode, image[start..start + size].to_vec())
    }

    fn entries(data: &[u8]) -> Vec<(String, u64)> {
        let mut names = Vec::new();
        for block in data.chunks(BLOCK) {
            let count = u16_at(block, 8) as usize / DIRENT;
            for k in 0..count {
                let at = k * DIRENT;
                let child = u64::from_le_bytes(block[at..at + 8].try_into().unwrap());
                let start = u16_at(block, at + 8) as usize;
                let end = if k + 1 < count {
                    u16_at(block, at + DIRENT + 8) as usize
                } else {
                    let tail = &block[start..];
                    start + tail.iter().position(|&b| b == 0).unwrap_or(tail.len())
                };
                let name = String::from_utf8(block[start..end].to_vec()).unwrap();
                names.push((name, child));
            }
        }
        names
    }

    fn walk(image: &[u8], folder: u64, prefix: &str, out: &mut Vec<Item>) {
        let (_, data) = inode(image, folder);
        let names = entries(&data);
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "entries are in name order");
        for (name, child) in names {
            if name == "." || name == ".." {
                continue;
            }
            let path = format!("{prefix}{name}");
            let (mode, data) = inode(image, child);
            let is_folder = mode & 0o170_000 == S_IFDIR;
            let data = if is_folder { Vec::new() } else { data };
            out.push(Item {
                path: path.clone(),
                mode,
                data,
            });
            if is_folder {
                walk(image, child, &format!("{path}/"), out);
            }
        }
    }

    /// Everything in `image`, in path order.
    pub fn items(image: &[u8]) -> Vec<Item> {
        assert_eq!(u32_at(image, SUPERBLOCK_OFFSET), MAGIC);
        assert_eq!(image[SUPERBLOCK_OFFSET + 12], BLOCK_BITS);
        let blocks = u32_at(image, SUPERBLOCK_OFFSET + 36) as usize;
        assert_eq!(blocks * BLOCK, image.len(), "every block is counted");
        let root = u64::from(u16_at(image, SUPERBLOCK_OFFSET + 14));
        let mut out = Vec::new();
        walk(image, root, "", &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::read::items;
    use super::*;

    fn put(tree: &mut Builder, path: &str, mode: u16, data: &[u8]) {
        tree.file(path, mode, data.to_vec()).unwrap();
    }

    #[test]
    fn fr25_an_erofs_image_holds_the_files_links_and_folders_it_was_given() {
        let mut tree = Builder::new();
        put(&mut tree, "system/lib64/libbridge.so", 0o644, b"bridge");
        let link = "system/lib64/libdsa_native_bridge.so";
        tree.link(link, "libbridge.so").unwrap();
        put(&mut tree, "system/bin/runner", 0o755, &[7; 9000]);
        put(&mut tree, "system/etc/empty", 0o644, b"");
        tree.folder("system/lib64/arm64").unwrap();
        let mut image = Vec::new();
        let length = tree.write(&mut image).unwrap();
        assert_eq!(length as usize, image.len());
        assert!(crate::arm_translation::is_erofs(&image));
        let found = items(&image);
        let paths: Vec<&str> = found.iter().map(|item| item.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "system",
                "system/bin",
                "system/bin/runner",
                "system/etc",
                "system/etc/empty",
                "system/lib64",
                "system/lib64/arm64",
                "system/lib64/libbridge.so",
                "system/lib64/libdsa_native_bridge.so",
            ]
        );
        let at = |path: &str| found.iter().find(|item| item.path == path).unwrap();
        assert_eq!(at("system/bin/runner").mode, 0o100_755);
        assert_eq!(at("system/bin/runner").data, vec![7u8; 9000]);
        assert_eq!(at("system/lib64/libbridge.so").mode, 0o100_644);
        assert_eq!(at(link).mode, 0o120_777);
        assert_eq!(at(link).data, b"libbridge.so");
        assert_eq!(at("system/etc/empty").data, b"");
        assert_eq!(at("system/lib64/arm64").mode, 0o040_755);
        let files = tree.files();
        assert_eq!(files.len(), 3, "{files:?}");
    }

    #[test]
    fn fr25_a_folder_of_many_files_spans_several_blocks_in_name_order() {
        let mut tree = Builder::new();
        for i in 0..400u32 {
            let path = format!("lib64/arm64/lib{i:04}_with_a_long_name.so");
            put(&mut tree, &path, 0o644, &i.to_le_bytes());
        }
        let mut image = Vec::new();
        tree.write(&mut image).unwrap();
        let found = items(&image);
        assert_eq!(found.len(), 402);
        assert_eq!(found[2].path, "lib64/arm64/lib0000_with_a_long_name.so");
        assert_eq!(found[401].data, 399u32.to_le_bytes());
    }

    #[test]
    fn fr25_a_path_that_could_leave_the_tree_is_refused() {
        let mut tree = Builder::new();
        for path in ["", "/", "../etc/passwd", "system/../../x", "a/./b"] {
            let added = tree.file(path, 0o644, Vec::new());
            assert!(added.is_err(), "{path:?}");
        }
        put(&mut tree, "a/b", 0o644, b"");
        let twice = tree.file("a/b", 0o644, Vec::new());
        assert!(twice.is_err());
        let inside_a_file = tree.file("a/b/c", 0o644, Vec::new());
        assert!(inside_a_file.is_err());
        assert!(tree.link("a/c", "").is_err());
    }
}
