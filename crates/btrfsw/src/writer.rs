//! Bare-metal Btrfs streaming filesystem generator (`BtrfsWriter`).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::min;

use disk::{FileKind, FsWriter, Meta, Storage};

use crate::crc::{btrfs_csum, btrfs_name_hash};
use crate::ondisk::{
    items, DiskKey, LeafNode, SuperBlock,
    BTRFS_BLOCK_GROUP_DATA, BTRFS_BLOCK_GROUP_METADATA, BTRFS_BLOCK_GROUP_SYSTEM,
    BTRFS_CHUNK_ITEM_KEY, BTRFS_CHUNK_TREE_OBJECTID, BTRFS_CSUM_TREE_OBJECTID,
    BTRFS_DATA_RELOC_TREE_OBJECTID, BTRFS_DEV_EXTENT_KEY, BTRFS_DEV_ITEMS_OBJECTID,
    BTRFS_DEV_ITEM_KEY, BTRFS_DEV_TREE_OBJECTID, BTRFS_DIR_INDEX_KEY, BTRFS_DIR_ITEM_KEY,
    BTRFS_EXTENT_CSUM_KEY, BTRFS_EXTENT_CSUM_OBJECTID, BTRFS_EXTENT_DATA_KEY,
    BTRFS_EXTENT_ITEM_KEY, BTRFS_METADATA_ITEM_KEY, BTRFS_EXTENT_TREE_OBJECTID, BTRFS_FIRST_CHUNK_TREE_OBJECTID,
    BTRFS_FIRST_FREE_OBJECTID, BTRFS_FS_TREE_OBJECTID, BTRFS_FT_BLKDEV, BTRFS_FT_CHRDEV,
    BTRFS_FT_DIR, BTRFS_FT_FIFO, BTRFS_FT_REG_FILE, BTRFS_FT_SYMLINK,
    BTRFS_INODE_ITEM_KEY, BTRFS_INODE_REF_KEY, BTRFS_NODE_SIZE,
    BTRFS_ROOT_BACKREF_KEY, BTRFS_ROOT_ITEM_KEY, BTRFS_ROOT_REF_KEY,
    BTRFS_ROOT_TREE_OBJECTID, BTRFS_SECTOR_SIZE, BTRFS_SUPER_INFO_OFFSET,
    BTRFS_BLOCK_GROUP_ITEM_KEY,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Io,
    NoSpace,
    NotFound,
    Exists,
    NotDir,
    IsDir,
    Invalid(&'static str),
}

pub type Result<T> = core::result::Result<T, Error>;

impl From<disk::Error> for Error {
    fn from(e: disk::Error) -> Self {
        match e {
            disk::Error::Io => Error::Io,
            disk::Error::NoSpace => Error::NoSpace,
            disk::Error::NotFound => Error::NotFound,
            disk::Error::Exists => Error::Exists,
            disk::Error::Size(s) | disk::Error::Invalid(s) => Error::Invalid(s),
        }
    }
}

pub fn to_disk_err(e: Error) -> disk::Error {
    match e {
        Error::Io => disk::Error::Io,
        Error::NoSpace => disk::Error::NoSpace,
        Error::NotFound => disk::Error::NotFound,
        Error::Exists => disk::Error::Exists,
        Error::NotDir | Error::IsDir => disk::Error::Invalid("directory type mismatch"),
        Error::Invalid(s) => disk::Error::Invalid(s),
    }
}

/// Subvolume configuration.
#[derive(Debug, Clone)]
pub struct SubvolConfig {
    pub name: String,
    pub mount: String,
}

/// Filesystem creation options.
#[derive(Debug, Clone)]
pub struct Options {
    pub label: String,
    pub uuid: [u8; 16],
    pub now: u64,
    pub subvolumes: Vec<SubvolConfig>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            label: "archroot".into(),
            uuid: [0u8; 16],
            now: 0,
            subvolumes: alloc::vec![
                SubvolConfig {
                    name: "@".into(),
                    mount: "/".into(),
                },
                SubvolConfig {
                    name: "@home".into(),
                    mount: "/home".into(),
                },
            ],
        }
    }
}

/// In-memory representation of an inode.
#[derive(Debug, Clone)]
struct InodeEntry {
    ino: u64,
    subvol_idx: usize,
    subvol_tree_id: u64,
    kind: FileKind,
    meta: Meta,
    size: u64,
    nlink: u32,
    rdev: u64,
    // File contents
    inline_data: Option<Vec<u8>>,
    // For regular extent files > 2048 bytes
    disk_bytenr: u64,
    disk_num_bytes: u64,
    // Directory children: name -> (child_ino, file_type, dir_index)
    entries: BTreeMap<String, (u64, u8, u64)>,
    next_dir_index: u64,
    // Parent backreference: parent_ino -> (name, dir_index)
    parents: Vec<(u64, String, u64)>,
}

/// Subvolume runtime tracking.
#[derive(Debug, Clone)]
struct SubvolRuntime {
    name: String,
    mount: String,
    tree_id: u64,
    root_ino: u64,
    root_inode: InodeEntry,
}

/// Currently streaming file state.
struct OpenFile {
    ino: u64,
    subvol_idx: usize,
    meta: Meta,
    path: String,
    buffer: Vec<u8>,
    is_regular_extent: bool,
    disk_bytenr: u64,
    disk_allocated_bytes: u64,
    bytes_written: u64,
    csums: Vec<u32>,
}

/// Write-once bare-metal Btrfs filesystem generator.
pub struct BtrfsWriter<S> {
    storage: S,
    total_bytes: u64,
    options: Options,
    subvols: Vec<SubvolRuntime>,

    // Allocators
    next_ino: u64,
    next_data_bytenr: u64,
    data_chunk_start: u64,
    data_chunk_end: u64,

    // Inode table: ino -> InodeEntry
    inodes: BTreeMap<u64, InodeEntry>,

    // Currently open file for streaming writes
    current_file: Option<OpenFile>,

    // Finished flag
    finished: bool,
}

impl<S: Storage> BtrfsWriter<S> {
    /// Creates a new BtrfsWriter.
    pub fn new(storage: S, total_bytes: u64, options: Options) -> Result<Self> {
        if total_bytes < 16 * 1024 * 1024 {
            return Err(Error::Invalid("disk too small for btrfs (min 16 MiB)"));
        }

        // Layout:
        // 0..1 MiB: superblock & boot padding
        // 1 MiB..3 MiB (2 MiB): SYSTEM chunk
        // 3 MiB..7 MiB (4 MiB): METADATA chunk
        // 7 MiB..total_bytes: DATA chunk
        let data_chunk_start = 7 * 1024 * 1024;
        let data_chunk_end = total_bytes;

        let mut subvols = Vec::new();
        let inodes = BTreeMap::new();

        // Initialize subvolumes
        for (i, cfg) in options.subvolumes.iter().enumerate() {
            let tree_id = 256 + i as u64;
            let root_ino = BTRFS_FIRST_FREE_OBJECTID; // 256

            // Virtual root directory inode for each subvolume
            let root_inode = InodeEntry {
                ino: root_ino,
                subvol_idx: i,
                subvol_tree_id: tree_id,
                kind: FileKind::Dir,
                meta: Meta {
                    mode: 0o755,
                    uid: 0,
                    gid: 0,
                    mtime: options.now,
                    xattrs: Vec::new(),
                },
                size: 0,
                nlink: 1,
                rdev: 0,
                inline_data: None,
                disk_bytenr: 0,
                disk_num_bytes: 0,
                entries: BTreeMap::new(),
                next_dir_index: 2,
                parents: Vec::new(),
            };

            subvols.push(SubvolRuntime {
                name: cfg.name.clone(),
                mount: cfg.mount.clone(),
                tree_id,
                root_ino,
                root_inode,
            });
        }

        Ok(Self {
            storage,
            total_bytes,
            options,
            subvols,
            next_ino: 257,
            next_data_bytenr: data_chunk_start,
            data_chunk_start,
            data_chunk_end,
            inodes,
            current_file: None,
            finished: false,
        })
    }

    /// Normalizes path and resolves target subvolume and subvol-relative path.
    fn route_path<'a>(&self, path: &'a str) -> (usize, &'a str) {
        let clean = path.trim_start_matches("./").trim_start_matches('/');
        let clean = clean.trim_end_matches('/');

        // Check if there is a subvolume matching prefix
        for (idx, subvol) in self.subvols.iter().enumerate() {
            let m = subvol.mount.trim_start_matches('/');
            if !m.is_empty() {
                if clean == m {
                    return (idx, "");
                }
                if let Some(rest) = clean.strip_prefix(m).and_then(|r| r.strip_prefix('/')) {
                    return (idx, rest);
                }
            }
        }

        // Default to root subvolume (first subvolume, usually "@")
        (0, clean)
    }

    /// Splits subvolume-relative path into parent path and entry name.
    fn split_parent<'a>(&self, path: &'a str) -> (&'a str, &'a str) {
        match path.rfind('/') {
            Some(pos) => {
                let parent = path.get(..pos).unwrap_or("");
                let name = path.get(pos + 1..).unwrap_or("");
                (parent, name)
            }
            None => ("", path),
        }
    }

    fn get_inode(&self, subvol_idx: usize, ino: u64) -> Option<&InodeEntry> {
        if ino == BTRFS_FIRST_FREE_OBJECTID {
            self.subvols.get(subvol_idx).map(|s| &s.root_inode)
        } else {
            self.inodes.get(&ino)
        }
    }

    fn get_inode_mut(&mut self, subvol_idx: usize, ino: u64) -> Option<&mut InodeEntry> {
        if ino == BTRFS_FIRST_FREE_OBJECTID {
            self.subvols.get_mut(subvol_idx).map(|s| &mut s.root_inode)
        } else {
            self.inodes.get_mut(&ino)
        }
    }

    /// Looks up an inode within a specific subvolume.
    fn lookup_in_subvol(&self, subvol_idx: usize, rel_path: &str) -> Option<u64> {
        let root_ino = self.subvols.get(subvol_idx)?.root_ino;
        if rel_path.is_empty() {
            return Some(root_ino);
        }

        let mut cur_ino = root_ino;
        for segment in rel_path.split('/') {
            if segment.is_empty() {
                continue;
            }
            let inode = self.get_inode(subvol_idx, cur_ino)?;
            let &(child_ino, _, _) = inode.entries.get(segment)?;
            cur_ino = child_ino;
        }

        Some(cur_ino)
    }

    /// Resolves parent directory inode within a subvolume, creating intermediate directories if needed.
    fn ensure_parent_dir(&mut self, subvol_idx: usize, parent_rel_path: &str, meta: &Meta) -> Result<u64> {
        let root_ino = self.subvols[subvol_idx].root_ino;
        if parent_rel_path.is_empty() {
            return Ok(root_ino);
        }

        let mut cur_ino = root_ino;
        let segments: Vec<&str> = parent_rel_path.split('/').filter(|s| !s.is_empty()).collect();

        for seg in segments {
            let exists = {
                let parent_entry = self.get_inode(subvol_idx, cur_ino).ok_or(Error::NotFound)?;
                if parent_entry.kind != FileKind::Dir {
                    return Err(Error::NotDir);
                }
                parent_entry.entries.get(seg).copied()
            };

            match exists {
                Some((child_ino, _, _)) => {
                    cur_ino = child_ino;
                }
                None => {
                    let new_ino = self.alloc_ino();
                    let tree_id = self.subvols[subvol_idx].tree_id;
                    let dir_index = {
                        let parent = self.get_inode_mut(subvol_idx, cur_ino).ok_or(Error::NotFound)?;
                        let idx = parent.next_dir_index;
                        parent.next_dir_index += 1;
                        parent.entries.insert(seg.into(), (new_ino, BTRFS_FT_DIR, idx));
                        idx
                    };

                    let child_entry = InodeEntry {
                        ino: new_ino,
                        subvol_idx,
                        subvol_tree_id: tree_id,
                        kind: FileKind::Dir,
                        meta: meta.clone(),
                        size: 0,
                        nlink: 1,
                        rdev: 0,
                        inline_data: None,
                        disk_bytenr: 0,
                        disk_num_bytes: 0,
                        entries: BTreeMap::new(),
                        next_dir_index: 2,
                        parents: alloc::vec![(cur_ino, seg.into(), dir_index)],
                    };
                    self.inodes.insert(new_ino, child_entry);
                    cur_ino = new_ino;
                }
            }
        }

        let target = self.get_inode(subvol_idx, cur_ino).ok_or(Error::NotFound)?;
        if target.kind != FileKind::Dir {
            return Err(Error::NotDir);
        }

        Ok(cur_ino)
    }

    fn alloc_ino(&mut self) -> u64 {
        let ino = self.next_ino;
        self.next_ino += 1;
        ino
    }

    pub fn mkdir(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        let (subvol_idx, rel_path) = self.route_path(path);
        if rel_path.is_empty() {
            // Refers to subvolume root
            return Ok(self.subvols[subvol_idx].root_ino);
        }

        let (parent_path, name) = self.split_parent(rel_path);
        let parent_ino = self.ensure_parent_dir(subvol_idx, parent_path, meta)?;

        let parent = self.get_inode(subvol_idx, parent_ino).ok_or(Error::NotFound)?;
        if parent.entries.contains_key(name) {
            return Err(Error::Exists);
        }

        let new_ino = self.alloc_ino();
        let tree_id = self.subvols[subvol_idx].tree_id;

        let dir_index = {
            let parent = self.get_inode_mut(subvol_idx, parent_ino).ok_or(Error::NotFound)?;
            let idx = parent.next_dir_index;
            parent.next_dir_index += 1;
            parent.entries.insert(name.into(), (new_ino, BTRFS_FT_DIR, idx));
            idx
        };

        let entry = InodeEntry {
            ino: new_ino,
            subvol_idx,
            subvol_tree_id: tree_id,
            kind: FileKind::Dir,
            meta: meta.clone(),
            size: 0,
            nlink: 1,
            rdev: 0,
            inline_data: None,
            disk_bytenr: 0,
            disk_num_bytes: 0,
            entries: BTreeMap::new(),
            next_dir_index: 2,
            parents: alloc::vec![(parent_ino, name.into(), dir_index)],
        };
        self.inodes.insert(new_ino, entry);

        Ok(new_ino)
    }

    pub fn begin_file(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        if self.current_file.is_some() {
            return Err(Error::Invalid("file already open"));
        }

        let (subvol_idx, rel_path) = self.route_path(path);
        if rel_path.is_empty() {
            return Err(Error::IsDir);
        }

        let (parent_path, name) = self.split_parent(rel_path);
        let parent_ino = self.ensure_parent_dir(subvol_idx, parent_path, meta)?;

        let parent = self.get_inode(subvol_idx, parent_ino).ok_or(Error::NotFound)?;
        if parent.entries.contains_key(name) {
            return Err(Error::Exists);
        }

        let ino = self.alloc_ino();
        self.current_file = Some(OpenFile {
            ino,
            subvol_idx,
            meta: meta.clone(),
            path: rel_path.into(),
            buffer: Vec::new(),
            is_regular_extent: false,
            disk_bytenr: 0,
            disk_allocated_bytes: 0,
            bytes_written: 0,
            csums: Vec::new(),
        });

        Ok(ino)
    }

    pub fn write_file(&mut self, data: &[u8]) -> Result<()> {
        let cur = self.current_file.as_mut().ok_or(Error::Invalid("no open file"))?;
        cur.bytes_written += data.len() as u64;
        cur.buffer.extend_from_slice(data);

        if !cur.is_regular_extent && cur.buffer.len() > 2048 {
            cur.is_regular_extent = true;
            cur.disk_bytenr = self.next_data_bytenr;
        }

        if cur.is_regular_extent {
            while cur.buffer.len() >= BTRFS_SECTOR_SIZE as usize {
                let block: Vec<u8> = cur.buffer.drain(..BTRFS_SECTOR_SIZE as usize).collect();
                let pos = self.next_data_bytenr;
                let new_end = pos + BTRFS_SECTOR_SIZE as u64;
                if new_end > self.data_chunk_end {
                    return Err(Error::NoSpace);
                }
                self.storage.write_at(pos, &block)?;
                self.next_data_bytenr = new_end;
                cur.disk_allocated_bytes += BTRFS_SECTOR_SIZE as u64;
                let c = btrfs_csum(&block);
                cur.csums.push(c);
            }
        }

        Ok(())
    }

    pub fn end_file(&mut self) -> Result<u64> {
        let mut cur = self.current_file.take().ok_or(Error::Invalid("no open file"))?;

        let (parent_path, name) = self.split_parent(&cur.path);
        let parent_ino = self.ensure_parent_dir(cur.subvol_idx, parent_path, &cur.meta)?;

        let tree_id = self.subvols[cur.subvol_idx].tree_id;
        let ino = cur.ino;

        let (inline_data, disk_bytenr, disk_num_bytes) = if !cur.is_regular_extent {
            // Inline file
            (Some(cur.buffer), 0, 0)
        } else {
            // Regular file: if there are remaining tail bytes in buffer, flush them padded to sector size
            if !cur.buffer.is_empty() {
                let rem_len = cur.buffer.len();
                let pad = (BTRFS_SECTOR_SIZE as usize) - rem_len;
                let mut block = cur.buffer;
                block.resize(block.len() + pad, 0);

                let pos = self.next_data_bytenr;
                let new_end = pos + block.len() as u64;
                if new_end > self.data_chunk_end {
                    return Err(Error::NoSpace);
                }

                self.storage.write_at(pos, &block)?;
                self.next_data_bytenr = new_end;
                cur.disk_allocated_bytes += block.len() as u64;

                let c = btrfs_csum(&block);
                cur.csums.push(c);
            }

            (None, cur.disk_bytenr, cur.disk_allocated_bytes)
        };

        let dir_index = {
            let parent = self.get_inode_mut(cur.subvol_idx, parent_ino).ok_or(Error::NotFound)?;
            let idx = parent.next_dir_index;
            parent.next_dir_index += 1;
            parent.entries.insert(name.into(), (ino, BTRFS_FT_REG_FILE, idx));
            idx
        };

        let entry = InodeEntry {
            ino,
            subvol_idx: cur.subvol_idx,
            subvol_tree_id: tree_id,
            kind: FileKind::Regular,
            meta: cur.meta,
            size: cur.bytes_written,
            nlink: 1,
            rdev: 0,
            inline_data,
            disk_bytenr,
            disk_num_bytes,
            entries: BTreeMap::new(),
            next_dir_index: 2,
            parents: alloc::vec![(parent_ino, name.into(), dir_index)],
        };
        self.inodes.insert(ino, entry);

        Ok(ino)
    }

    pub fn symlink(&mut self, path: &str, target: &str, meta: &Meta) -> Result<u64> {
        let (subvol_idx, rel_path) = self.route_path(path);
        if rel_path.is_empty() {
            return Err(Error::IsDir);
        }

        let (parent_path, name) = self.split_parent(rel_path);
        let parent_ino = self.ensure_parent_dir(subvol_idx, parent_path, meta)?;

        let parent = self.get_inode(subvol_idx, parent_ino).ok_or(Error::NotFound)?;
        if parent.entries.contains_key(name) {
            return Err(Error::Exists);
        }

        let ino = self.alloc_ino();
        let tree_id = self.subvols[subvol_idx].tree_id;

        let dir_index = {
            let parent = self.get_inode_mut(subvol_idx, parent_ino).ok_or(Error::NotFound)?;
            let idx = parent.next_dir_index;
            parent.next_dir_index += 1;
            parent.entries.insert(name.into(), (ino, BTRFS_FT_SYMLINK, idx));
            idx
        };

        let target_bytes = target.as_bytes().to_vec();
        let entry = InodeEntry {
            ino,
            subvol_idx,
            subvol_tree_id: tree_id,
            kind: FileKind::Symlink,
            meta: meta.clone(),
            size: target_bytes.len() as u64,
            nlink: 1,
            rdev: 0,
            inline_data: Some(target_bytes),
            disk_bytenr: 0,
            disk_num_bytes: 0,
            entries: BTreeMap::new(),
            next_dir_index: 2,
            parents: alloc::vec![(parent_ino, name.into(), dir_index)],
        };
        self.inodes.insert(ino, entry);

        Ok(ino)
    }

    pub fn hardlink(&mut self, path: &str, existing: &str) -> Result<u64> {
        let existing_ino = self.lookup(existing).ok_or(Error::NotFound)?;
        let (subvol_idx, rel_path) = self.route_path(path);
        if rel_path.is_empty() {
            return Err(Error::IsDir);
        }

        let (_existing_subvol_idx, existing_kind, parent_meta) = {
            let existing_inode = self.get_inode(subvol_idx, existing_ino).ok_or(Error::NotFound)?;
            if existing_inode.kind == FileKind::Dir {
                return Err(Error::IsDir);
            }
            if existing_inode.subvol_idx != subvol_idx {
                return Err(Error::Invalid("cross-subvolume hardlink not permitted"));
            }
            (existing_inode.subvol_idx, existing_inode.kind, existing_inode.meta.clone())
        };

        let (parent_path, name) = self.split_parent(rel_path);
        let parent_ino = self.ensure_parent_dir(subvol_idx, parent_path, &parent_meta)?;

        let parent = self.get_inode(subvol_idx, parent_ino).ok_or(Error::NotFound)?;
        if parent.entries.contains_key(name) {
            return Err(Error::Exists);
        }

        let ft = match existing_kind {
            FileKind::Regular => BTRFS_FT_REG_FILE,
            FileKind::Symlink => BTRFS_FT_SYMLINK,
            FileKind::CharDev => BTRFS_FT_CHRDEV,
            FileKind::BlockDev => BTRFS_FT_BLKDEV,
            FileKind::Fifo => BTRFS_FT_FIFO,
            FileKind::Dir => BTRFS_FT_DIR,
        };

        let dir_index = {
            let parent = self.get_inode_mut(subvol_idx, parent_ino).ok_or(Error::NotFound)?;
            let idx = parent.next_dir_index;
            parent.next_dir_index += 1;
            parent.entries.insert(name.into(), (existing_ino, ft, idx));
            idx
        };

        let inode = self.inodes.get_mut(&existing_ino).ok_or(Error::NotFound)?;
        inode.nlink += 1;
        inode.parents.push((parent_ino, name.into(), dir_index));

        Ok(existing_ino)
    }

    pub fn mknod(
        &mut self,
        path: &str,
        kind: FileKind,
        major: u32,
        minor: u32,
        meta: &Meta,
    ) -> Result<u64> {
        if !matches!(kind, FileKind::CharDev | FileKind::BlockDev | FileKind::Fifo) {
            return Err(Error::Invalid("not a device node or fifo"));
        }

        let (subvol_idx, rel_path) = self.route_path(path);
        if rel_path.is_empty() {
            return Err(Error::IsDir);
        }

        let (parent_path, name) = self.split_parent(rel_path);
        let parent_ino = self.ensure_parent_dir(subvol_idx, parent_path, meta)?;

        let parent = self.get_inode(subvol_idx, parent_ino).ok_or(Error::NotFound)?;
        if parent.entries.contains_key(name) {
            return Err(Error::Exists);
        }

        let ino = self.alloc_ino();
        let tree_id = self.subvols[subvol_idx].tree_id;

        let ft = match kind {
            FileKind::CharDev => BTRFS_FT_CHRDEV,
            FileKind::BlockDev => BTRFS_FT_BLKDEV,
            FileKind::Fifo => BTRFS_FT_FIFO,
            _ => BTRFS_FT_REG_FILE,
        };

        let dir_index = {
            let parent = self.get_inode_mut(subvol_idx, parent_ino).ok_or(Error::NotFound)?;
            let idx = parent.next_dir_index;
            parent.next_dir_index += 1;
            parent.entries.insert(name.into(), (ino, ft, idx));
            idx
        };

        let rdev = ((minor & 0xff) | ((major & 0xfff) << 8) | ((minor & !0xff) << 12)) as u64;

        let entry = InodeEntry {
            ino,
            subvol_idx,
            subvol_tree_id: tree_id,
            kind,
            meta: meta.clone(),
            size: 0,
            nlink: 1,
            rdev,
            inline_data: None,
            disk_bytenr: 0,
            disk_num_bytes: 0,
            entries: BTreeMap::new(),
            next_dir_index: 2,
            parents: alloc::vec![(parent_ino, name.into(), dir_index)],
        };
        self.inodes.insert(ino, entry);

        Ok(ino)
    }

    pub fn lookup(&self, path: &str) -> Option<u64> {
        let (subvol_idx, rel_path) = self.route_path(path);
        self.lookup_in_subvol(subvol_idx, rel_path)
    }

    pub fn file_size(&self, id: u64) -> u64 {
        self.inodes.get(&id).map_or(0, |n| n.size)
    }

    pub fn read_file(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let inode = self.inodes.get(&id).ok_or(Error::NotFound)?;
        if offset >= inode.size {
            return Ok(0);
        }

        let to_read = min(buf.len() as u64, inode.size - offset) as usize;
        if to_read == 0 {
            return Ok(0);
        }

        if let Some(ref data) = inode.inline_data {
            let start = offset as usize;
            let end = start + to_read;
            buf[..to_read].copy_from_slice(&data[start..end]);
            Ok(to_read)
        } else if inode.disk_bytenr != 0 {
            let disk_offset = inode.disk_bytenr + offset;
            self.storage.read_at(disk_offset, &mut buf[..to_read])?;
            Ok(to_read)
        } else {
            Ok(0)
        }
    }

    /// Serializes metadata trees and superblocks to storage.
    pub fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }

        // Layout:
        // System Chunk: 0x100000 (1 MiB), length 2 MiB
        // Metadata Chunk: 0x300000 (3 MiB), length 4 MiB
        // Data Chunk: 0x700000 (7 MiB), length total_bytes - 7 MiB
        let system_chunk_start = 1024 * 1024;
        let system_chunk_len = 2 * 1024 * 1024;

        let metadata_chunk_start = 3 * 1024 * 1024;
        let metadata_chunk_len = 4 * 1024 * 1024;

        let data_chunk_start = self.data_chunk_start;
        let data_chunk_len = self.data_chunk_end - data_chunk_start;

        let mut next_meta_bytenr = metadata_chunk_start;

        // Tree root bytenrs:
        let chunk_root_bytenr = system_chunk_start; // 0x100000 in System chunk

        let root_tree_bytenr = next_meta_bytenr;
        next_meta_bytenr += BTRFS_NODE_SIZE as u64;

        let extent_tree_bytenr = next_meta_bytenr;
        next_meta_bytenr += BTRFS_NODE_SIZE as u64;

        let csum_tree_bytenr = next_meta_bytenr;
        next_meta_bytenr += BTRFS_NODE_SIZE as u64;

        let top_fs_tree_bytenr = next_meta_bytenr;
        next_meta_bytenr += BTRFS_NODE_SIZE as u64;

        let dev_tree_bytenr = next_meta_bytenr;
        next_meta_bytenr += BTRFS_NODE_SIZE as u64;

        let data_reloc_tree_bytenr = next_meta_bytenr;
        next_meta_bytenr += BTRFS_NODE_SIZE as u64;

        // Allocate a metadata leaf bytenr for each subvolume
        let mut subvol_bytenrs = Vec::new();
        for _ in &self.subvols {
            subvol_bytenrs.push(next_meta_bytenr);
            next_meta_bytenr += BTRFS_NODE_SIZE as u64;
        }

        let fsid = self.options.uuid;
        let gen = 1u64;

        // 1. Build Chunk Tree (Tree 3)
        let mut chunk_leaf = LeafNode::new(chunk_root_bytenr, fsid, gen, BTRFS_CHUNK_TREE_OBJECTID);
        // DEV_ITEM
        let dev_key = DiskKey::new(BTRFS_DEV_ITEMS_OBJECTID, BTRFS_DEV_ITEM_KEY, 1);
        let dev_data = items::serialize_dev_item(1, self.total_bytes, 0, fsid, fsid);
        chunk_leaf.insert(dev_key, dev_data.to_vec());

        // CHUNK_ITEM for System Chunk
        let sys_chunk_key = DiskKey::new(BTRFS_FIRST_CHUNK_TREE_OBJECTID, BTRFS_CHUNK_ITEM_KEY, system_chunk_start);
        let sys_chunk_data = items::serialize_chunk_item(system_chunk_len, BTRFS_EXTENT_TREE_OBJECTID, BTRFS_BLOCK_GROUP_SYSTEM, system_chunk_start, fsid);
        chunk_leaf.insert(sys_chunk_key, sys_chunk_data.to_vec());

        // CHUNK_ITEM for Metadata Chunk
        let meta_chunk_key = DiskKey::new(BTRFS_FIRST_CHUNK_TREE_OBJECTID, BTRFS_CHUNK_ITEM_KEY, metadata_chunk_start);
        let meta_chunk_data = items::serialize_chunk_item(metadata_chunk_len, BTRFS_EXTENT_TREE_OBJECTID, BTRFS_BLOCK_GROUP_METADATA, metadata_chunk_start, fsid);
        chunk_leaf.insert(meta_chunk_key, meta_chunk_data.to_vec());

        // CHUNK_ITEM for Data Chunk
        let data_chunk_key = DiskKey::new(BTRFS_FIRST_CHUNK_TREE_OBJECTID, BTRFS_CHUNK_ITEM_KEY, data_chunk_start);
        let data_chunk_data = items::serialize_chunk_item(data_chunk_len, BTRFS_EXTENT_TREE_OBJECTID, BTRFS_BLOCK_GROUP_DATA, data_chunk_start, fsid);
        chunk_leaf.insert(data_chunk_key, data_chunk_data.to_vec());

        self.storage.write_at(chunk_root_bytenr, &chunk_leaf.to_bytes())?;

        // 2. Build Subvolume FS Trees
        for (subvol_idx, subvol) in self.subvols.iter().enumerate() {
            let bytenr = subvol_bytenrs[subvol_idx];
            let mut fs_leaf = LeafNode::new(bytenr, fsid, gen, subvol.tree_id);

            let subvol_inodes = core::iter::once(&subvol.root_inode)
                .chain(self.inodes.values().filter(|n| n.subvol_idx == subvol_idx));

            for inode in subvol_inodes {
                let ino_key = DiskKey::new(inode.ino, BTRFS_INODE_ITEM_KEY, 0);
                let mode_bits = match inode.kind {
                    FileKind::Dir => 0o040000 | (inode.meta.mode as u32 & 0o7777),
                    FileKind::Regular => 0o100000 | (inode.meta.mode as u32 & 0o7777),
                    FileKind::Symlink => 0o120777,
                    FileKind::CharDev => 0o020000 | (inode.meta.mode as u32 & 0o7777),
                    FileKind::BlockDev => 0o060000 | (inode.meta.mode as u32 & 0o7777),
                    FileKind::Fifo => 0o010000 | (inode.meta.mode as u32 & 0o7777),
                };

                let ino_data = items::serialize_inode_item(
                    gen, gen, inode.size, inode.disk_num_bytes, 0,
                    inode.nlink, inode.meta.uid, inode.meta.gid, mode_bits,
                    inode.rdev, 0, 0,
                    inode.meta.mtime, inode.meta.mtime, inode.meta.mtime, inode.meta.mtime,
                );
                fs_leaf.insert(ino_key, ino_data.to_vec());

                // INODE_REF
                for &(parent_ino, ref name, dir_idx) in &inode.parents {
                    let ref_key = DiskKey::new(inode.ino, BTRFS_INODE_REF_KEY, parent_ino);
                    let ref_data = items::serialize_inode_ref(dir_idx, name);
                    fs_leaf.insert(ref_key, ref_data);
                }

                // If directory: DIR_ITEM and DIR_INDEX for each child
                if inode.kind == FileKind::Dir {
                    for (name, &(child_ino, ft, dir_idx)) in &inode.entries {
                        let hash = btrfs_name_hash(name.as_bytes());
                        let dir_item_key = DiskKey::new(inode.ino, BTRFS_DIR_ITEM_KEY, hash);
                        let loc = DiskKey::new(child_ino, BTRFS_INODE_ITEM_KEY, 0);
                        let dir_item_data = items::serialize_dir_item(loc, gen, ft, name);
                        fs_leaf.insert(dir_item_key, dir_item_data.clone());

                        let dir_idx_key = DiskKey::new(inode.ino, BTRFS_DIR_INDEX_KEY, dir_idx);
                        fs_leaf.insert(dir_idx_key, dir_item_data);
                    }
                }

                // If regular file or symlink: EXTENT_DATA
                if let Some(ref inline_bytes) = inode.inline_data {
                    let ext_key = DiskKey::new(inode.ino, BTRFS_EXTENT_DATA_KEY, 0);
                    let ext_data = items::serialize_file_extent_inline(gen, inline_bytes);
                    fs_leaf.insert(ext_key, ext_data);
                } else if inode.disk_bytenr != 0 {
                    let ext_key = DiskKey::new(inode.ino, BTRFS_EXTENT_DATA_KEY, 0);
                    let ext_data = items::serialize_file_extent_reg(
                        gen, inode.disk_num_bytes, inode.disk_bytenr, inode.disk_num_bytes, 0, inode.disk_num_bytes,
                    );
                    fs_leaf.insert(ext_key, ext_data.to_vec());
                }
            }

            self.storage.write_at(bytenr, &fs_leaf.to_bytes())?;
        }

        // 3. Build Top-level FS Tree (Tree 5)
        let mut top_fs_leaf = LeafNode::new(top_fs_tree_bytenr, fsid, gen, BTRFS_FS_TREE_OBJECTID);
        // Root dir of Tree 5
        let top_root_key = DiskKey::new(BTRFS_FIRST_FREE_OBJECTID, BTRFS_INODE_ITEM_KEY, 0);
        let top_root_data = items::serialize_inode_item(
            gen, gen, 0, 0, 0, 1, 0, 0, 0o040755, 0, 0, 0,
            self.options.now, self.options.now, self.options.now, self.options.now,
        );
        top_fs_leaf.insert(top_root_key, top_root_data.to_vec());

        // Directory entries in Tree 5 pointing to each subvolume
        for (i, subvol) in self.subvols.iter().enumerate() {
            let hash = btrfs_name_hash(subvol.name.as_bytes());
            let dir_item_key = DiskKey::new(BTRFS_FIRST_FREE_OBJECTID, BTRFS_DIR_ITEM_KEY, hash);
            let loc = DiskKey::new(subvol.tree_id, BTRFS_ROOT_ITEM_KEY, 0);
            let dir_item_data = items::serialize_dir_item(loc, gen, BTRFS_FT_DIR, &subvol.name);
            top_fs_leaf.insert(dir_item_key, dir_item_data.clone());

            let dir_idx_key = DiskKey::new(BTRFS_FIRST_FREE_OBJECTID, BTRFS_DIR_INDEX_KEY, 2 + i as u64);
            top_fs_leaf.insert(dir_idx_key, dir_item_data);
        }
        self.storage.write_at(top_fs_tree_bytenr, &top_fs_leaf.to_bytes())?;

        // 4. Build Dev Tree (Tree 4)
        let mut dev_leaf = LeafNode::new(dev_tree_bytenr, fsid, gen, BTRFS_DEV_TREE_OBJECTID);
        let dev_item_key = DiskKey::new(BTRFS_DEV_ITEMS_OBJECTID, BTRFS_DEV_ITEM_KEY, 1);
        let dev_item_data = items::serialize_dev_item(1, self.total_bytes, 0, fsid, fsid);
        dev_leaf.insert(dev_item_key, dev_item_data.to_vec());

        let sys_dev_ext_key = DiskKey::new(1, BTRFS_DEV_EXTENT_KEY, system_chunk_start);
        let sys_dev_ext_data = items::serialize_dev_extent(system_chunk_start, system_chunk_len, fsid);
        dev_leaf.insert(sys_dev_ext_key, sys_dev_ext_data.to_vec());

        let meta_dev_ext_key = DiskKey::new(1, BTRFS_DEV_EXTENT_KEY, metadata_chunk_start);
        let meta_dev_ext_data = items::serialize_dev_extent(metadata_chunk_start, metadata_chunk_len, fsid);
        dev_leaf.insert(meta_dev_ext_key, meta_dev_ext_data.to_vec());

        let data_dev_ext_key = DiskKey::new(1, BTRFS_DEV_EXTENT_KEY, data_chunk_start);
        let data_dev_ext_data = items::serialize_dev_extent(data_chunk_start, data_chunk_len, fsid);
        dev_leaf.insert(data_dev_ext_key, data_dev_ext_data.to_vec());

        self.storage.write_at(dev_tree_bytenr, &dev_leaf.to_bytes())?;

        // 5. Build Data Reloc Tree (-9ULL)
        let mut reloc_leaf = LeafNode::new(data_reloc_tree_bytenr, fsid, gen, BTRFS_DATA_RELOC_TREE_OBJECTID);
        let reloc_root_key = DiskKey::new(BTRFS_FIRST_FREE_OBJECTID, BTRFS_INODE_ITEM_KEY, 0);
        let reloc_root_data = items::serialize_inode_item(
            gen, gen, 0, 0, 0, 1, 0, 0, 0o040755, 0, 0, 0,
            self.options.now, self.options.now, self.options.now, self.options.now,
        );
        reloc_leaf.insert(reloc_root_key, reloc_root_data.to_vec());
        self.storage.write_at(data_reloc_tree_bytenr, &reloc_leaf.to_bytes())?;

        // 6. Build CSUM Tree (Tree 7)
        let mut csum_leaf = LeafNode::new(csum_tree_bytenr, fsid, gen, BTRFS_CSUM_TREE_OBJECTID);
        for inode in self.inodes.values() {
            if inode.disk_bytenr != 0 && inode.disk_num_bytes > 0 {
                let num_blocks = (inode.disk_num_bytes / BTRFS_SECTOR_SIZE as u64) as usize;
                let mut csums = Vec::with_capacity(num_blocks * 4);
                let mut block_buf = [0u8; BTRFS_SECTOR_SIZE as usize];

                for b in 0..num_blocks {
                    let off = inode.disk_bytenr + (b as u64 * BTRFS_SECTOR_SIZE as u64);
                    self.storage.read_at(off, &mut block_buf)?;
                    let c = btrfs_csum(&block_buf);
                    csums.extend_from_slice(&c.to_le_bytes());
                }

                let csum_key = DiskKey::new(BTRFS_EXTENT_CSUM_OBJECTID, BTRFS_EXTENT_CSUM_KEY, inode.disk_bytenr);
                csum_leaf.insert(csum_key, csums);
            }
        }
        self.storage.write_at(csum_tree_bytenr, &csum_leaf.to_bytes())?;

        // 7. Build Extent Tree (Tree 2)
        let mut extent_leaf = LeafNode::new(extent_tree_bytenr, fsid, gen, BTRFS_EXTENT_TREE_OBJECTID);

        // Record all metadata tree blocks
        let all_meta_blocks = [
            (chunk_root_bytenr, BTRFS_CHUNK_TREE_OBJECTID),
            (root_tree_bytenr, BTRFS_ROOT_TREE_OBJECTID),
            (extent_tree_bytenr, BTRFS_EXTENT_TREE_OBJECTID),
            (csum_tree_bytenr, BTRFS_CSUM_TREE_OBJECTID),
            (dev_tree_bytenr, BTRFS_DEV_TREE_OBJECTID),
            (data_reloc_tree_bytenr, BTRFS_DATA_RELOC_TREE_OBJECTID),
            (top_fs_tree_bytenr, BTRFS_FS_TREE_OBJECTID),
        ];

        for &(bytenr, tree_owner) in &all_meta_blocks {
            let key = DiskKey::new(bytenr, BTRFS_METADATA_ITEM_KEY, 0);
            let data = items::serialize_extent_item_tree_block(gen, tree_owner);
            extent_leaf.insert(key, data.to_vec());
        }

        for (idx, &bytenr) in subvol_bytenrs.iter().enumerate() {
            let tree_owner = self.subvols[idx].tree_id;
            let key = DiskKey::new(bytenr, BTRFS_METADATA_ITEM_KEY, 0);
            let data = items::serialize_extent_item_tree_block(gen, tree_owner);
            extent_leaf.insert(key, data.to_vec());
        }

        // Record regular data extents
        for inode in self.inodes.values() {
            if inode.disk_bytenr != 0 && inode.disk_num_bytes > 0 {
                let key = DiskKey::new(inode.disk_bytenr, BTRFS_EXTENT_ITEM_KEY, inode.disk_num_bytes);
                let data = items::serialize_extent_item_data(gen, inode.subvol_tree_id, inode.ino, 0);
                extent_leaf.insert(key, data.to_vec());
            }
        }

        // Record block group items in extent tree
        let bg_sys_key = DiskKey::new(system_chunk_start, BTRFS_BLOCK_GROUP_ITEM_KEY, system_chunk_len);
        let bg_sys_data = items::serialize_block_group_item(BTRFS_NODE_SIZE as u64, BTRFS_BLOCK_GROUP_SYSTEM, 0);
        extent_leaf.insert(bg_sys_key, bg_sys_data.to_vec());

        let meta_used = next_meta_bytenr - metadata_chunk_start;
        let bg_meta_key = DiskKey::new(metadata_chunk_start, BTRFS_BLOCK_GROUP_ITEM_KEY, metadata_chunk_len);
        let bg_meta_data = items::serialize_block_group_item(meta_used, BTRFS_BLOCK_GROUP_METADATA, 0);
        extent_leaf.insert(bg_meta_key, bg_meta_data.to_vec());

        let data_used = self.next_data_bytenr - data_chunk_start;
        let bg_data_key = DiskKey::new(data_chunk_start, BTRFS_BLOCK_GROUP_ITEM_KEY, data_chunk_len);
        let bg_data_data = items::serialize_block_group_item(data_used, BTRFS_BLOCK_GROUP_DATA, 0);
        extent_leaf.insert(bg_data_key, bg_data_data.to_vec());

        self.storage.write_at(extent_tree_bytenr, &extent_leaf.to_bytes())?;

        // 8. Build Root Tree (Tree 1)
        let mut root_leaf = LeafNode::new(root_tree_bytenr, fsid, gen, BTRFS_ROOT_TREE_OBJECTID);

        // Extent Tree ROOT_ITEM
        let ext_root_key = DiskKey::new(BTRFS_EXTENT_TREE_OBJECTID, BTRFS_ROOT_ITEM_KEY, 0);
        let ext_root_data = items::serialize_root_item(gen, 0, extent_tree_bytenr, BTRFS_NODE_SIZE as u64, 0, fsid);
        root_leaf.insert(ext_root_key, ext_root_data.to_vec());

        // Dev Tree ROOT_ITEM (ID 4)
        let dev_root_key = DiskKey::new(BTRFS_DEV_TREE_OBJECTID, BTRFS_ROOT_ITEM_KEY, 0);
        let dev_root_data = items::serialize_root_item(gen, 0, dev_tree_bytenr, BTRFS_NODE_SIZE as u64, 0, fsid);
        root_leaf.insert(dev_root_key, dev_root_data.to_vec());

        // CSUM Tree ROOT_ITEM
        let csum_root_key = DiskKey::new(BTRFS_CSUM_TREE_OBJECTID, BTRFS_ROOT_ITEM_KEY, 0);
        let csum_root_data = items::serialize_root_item(gen, 0, csum_tree_bytenr, BTRFS_NODE_SIZE as u64, 0, fsid);
        root_leaf.insert(csum_root_key, csum_root_data.to_vec());

        // Data Reloc Tree ROOT_ITEM (-9ULL)
        let reloc_root_key = DiskKey::new(BTRFS_DATA_RELOC_TREE_OBJECTID, BTRFS_ROOT_ITEM_KEY, 0);
        let reloc_root_data = items::serialize_root_item(gen, BTRFS_FIRST_FREE_OBJECTID, data_reloc_tree_bytenr, BTRFS_NODE_SIZE as u64, 0, fsid);
        root_leaf.insert(reloc_root_key, reloc_root_data.to_vec());

        // Top-level FS Tree (Tree 5) ROOT_ITEM
        let top_root_key = DiskKey::new(BTRFS_FS_TREE_OBJECTID, BTRFS_ROOT_ITEM_KEY, 0);
        let top_root_data = items::serialize_root_item(gen, BTRFS_FIRST_FREE_OBJECTID, top_fs_tree_bytenr, BTRFS_NODE_SIZE as u64, 0, fsid);
        root_leaf.insert(top_root_key, top_root_data.to_vec());

        // Subvolume ROOT_ITEMs and ROOT_REFs
        for (idx, subvol) in self.subvols.iter().enumerate() {
            let bytenr = subvol_bytenrs[idx];
            let sub_root_key = DiskKey::new(subvol.tree_id, BTRFS_ROOT_ITEM_KEY, 0);
            let sub_root_data = items::serialize_root_item(gen, BTRFS_FIRST_FREE_OBJECTID, bytenr, BTRFS_NODE_SIZE as u64, 0, fsid);
            root_leaf.insert(sub_root_key, sub_root_data.to_vec());

            // ROOT_REF: tree 5 -> subvol.tree_id
            let root_ref_key = DiskKey::new(BTRFS_FS_TREE_OBJECTID, BTRFS_ROOT_REF_KEY, subvol.tree_id);
            let root_ref_data = items::serialize_root_ref(BTRFS_FIRST_FREE_OBJECTID, 2 + idx as u64, &subvol.name);
            root_leaf.insert(root_ref_key, root_ref_data.clone());

            // ROOT_BACKREF: subvol.tree_id -> tree 5
            let backref_key = DiskKey::new(subvol.tree_id, BTRFS_ROOT_BACKREF_KEY, BTRFS_FS_TREE_OBJECTID);
            let backref_data = items::serialize_root_ref(BTRFS_FIRST_FREE_OBJECTID, 2 + idx as u64, &subvol.name);
            root_leaf.insert(backref_key, backref_data);
        }

        self.storage.write_at(root_tree_bytenr, &root_leaf.to_bytes())?;

        // 7. Write Superblock (at 65536)
        let mut sb = SuperBlock::new(fsid, self.total_bytes, root_tree_bytenr, chunk_root_bytenr);
        sb.set_label(&self.options.label);
        sb.add_sys_chunk(system_chunk_start, system_chunk_len, BTRFS_BLOCK_GROUP_SYSTEM);

        let bytes_allocated = (next_meta_bytenr - metadata_chunk_start)
            + (self.next_data_bytenr - data_chunk_start);
        sb.bytes_used = bytes_allocated;

        self.storage.write_at(BTRFS_SUPER_INFO_OFFSET, &sb.to_bytes())?;

        self.finished = true;
        Ok(())
    }
}

impl<S: Storage> FsWriter for BtrfsWriter<S> {
    fn mkdir(&mut self, path: &str, meta: &Meta) -> disk::Result<u64> {
        self.mkdir(path, meta).map_err(to_disk_err)
    }

    fn begin_file(&mut self, path: &str, meta: &Meta) -> disk::Result<u64> {
        self.begin_file(path, meta).map_err(to_disk_err)
    }

    fn write_file(&mut self, data: &[u8]) -> disk::Result<()> {
        self.write_file(data).map_err(to_disk_err)
    }

    fn end_file(&mut self) -> disk::Result<u64> {
        self.end_file().map_err(to_disk_err)
    }

    fn symlink(&mut self, path: &str, target: &str, meta: &Meta) -> disk::Result<u64> {
        self.symlink(path, target, meta).map_err(to_disk_err)
    }

    fn hardlink(&mut self, path: &str, existing: &str) -> disk::Result<u64> {
        self.hardlink(path, existing).map_err(to_disk_err)
    }

    fn mknod(
        &mut self,
        path: &str,
        kind: FileKind,
        major: u32,
        minor: u32,
        meta: &Meta,
    ) -> disk::Result<u64> {
        self.mknod(path, kind, major, minor, meta).map_err(to_disk_err)
    }

    fn lookup(&self, path: &str) -> Option<u64> {
        self.lookup(path)
    }

    fn file_size(&self, id: u64) -> u64 {
        self.file_size(id)
    }

    fn read_file(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> disk::Result<usize> {
        self.read_file(id, offset, buf).map_err(to_disk_err)
    }

    fn finish(&mut self) -> disk::Result<()> {
        self.finish().map_err(to_disk_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemStorage(Vec<u8>);

    impl MemStorage {
        fn new(size: usize) -> Self {
            Self(alloc::vec![0u8; size])
        }
    }

    impl Storage for MemStorage {
        fn write_at(&mut self, offset: u64, data: &[u8]) -> disk::Result<()> {
            let start = offset as usize;
            let end = start + data.len();
            if end > self.0.len() {
                return Err(disk::Error::NoSpace);
            }
            self.0[start..end].copy_from_slice(data);
            Ok(())
        }

        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> disk::Result<()> {
            let start = offset as usize;
            let end = start + buf.len();
            if end > self.0.len() {
                return Err(disk::Error::NotFound);
            }
            buf.copy_from_slice(&self.0[start..end]);
            Ok(())
        }
    }

    #[test]
    fn test_empty_image_and_finish() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let opts = Options {
            label: "archroot".into(),
            uuid: [0x42; 16],
            now: 1234567,
            ..Default::default()
        };

        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, opts).unwrap();
        writer.finish().unwrap();

        // Verify superblock on disk at 65536
        let mut sb_bytes = [0u8; 4096];
        writer.storage.read_at(65536, &mut sb_bytes).unwrap();

        // Magic at 64
        assert_eq!(&sb_bytes[64..72], b"_BHRfS_M");
        // UUID at 32
        assert_eq!(&sb_bytes[32..48], &[0x42; 16]);
        // Valid checksum
        let csum = u32::from_le_bytes(sb_bytes[0..4].try_into().unwrap());
        assert_eq!(csum, btrfs_csum(&sb_bytes[32..4096]));
    }

    #[test]
    fn test_mkdir_hierarchy_and_lookup() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta {
            mode: 0o755,
            mtime: 1000,
            ..Default::default()
        };

        let etc_id = writer.mkdir("etc", &meta).unwrap();
        let ssh_id = writer.mkdir("etc/ssh", &meta).unwrap();
        assert_ne!(etc_id, ssh_id);

        assert_eq!(writer.lookup("etc"), Some(etc_id));
        assert_eq!(writer.lookup("/etc"), Some(etc_id));
        assert_eq!(writer.lookup("./etc"), Some(etc_id));
        assert_eq!(writer.lookup("etc/ssh"), Some(ssh_id));
        assert_eq!(writer.lookup("nonexistent"), None);

        // Error: directory already exists
        assert_eq!(writer.mkdir("etc", &meta), Err(Error::Exists));
    }

    #[test]
    fn test_small_inline_file_workflow() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta {
            mode: 0o644,
            mtime: 2000,
            ..Default::default()
        };

        writer.begin_file("etc/hostname", &meta).unwrap();
        writer.write_file(b"arch").unwrap();
        writer.write_file(b"box\n").unwrap();
        let file_id = writer.end_file().unwrap();

        assert_eq!(writer.file_size(file_id), 8);
        assert_eq!(writer.lookup("etc/hostname"), Some(file_id));

        // Read back
        let mut buf = [0u8; 8];
        let n = writer.read_file(file_id, 0, &mut buf).unwrap();
        assert_eq!(n, 8);
        assert_eq!(&buf, b"archbox\n");

        // Edge cases
        let mut partial = [0u8; 3];
        let pn = writer.read_file(file_id, 4, &mut partial).unwrap();
        assert_eq!(pn, 3);
        assert_eq!(&partial, b"box");

        let mut eof = [0u8; 4];
        let en = writer.read_file(file_id, 100, &mut eof).unwrap();
        assert_eq!(en, 0);
    }

    #[test]
    fn test_large_regular_file_extent() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta {
            mode: 0o644,
            mtime: 3000,
            ..Default::default()
        };

        // 8 KiB file (> 2048 bytes)
        let mut payload = Vec::new();
        for i in 0u32..8192 {
            payload.push((i % 251) as u8);
        }

        writer.begin_file("var/cache/data.bin", &meta).unwrap();
        writer.write_file(&payload[..4000]).unwrap();
        writer.write_file(&payload[4000..]).unwrap();
        let file_id = writer.end_file().unwrap();

        assert_eq!(writer.file_size(file_id), 8192);

        // Read back
        let mut read_buf = alloc::vec![0u8; 8192];
        let n = writer.read_file(file_id, 0, &mut read_buf).unwrap();
        assert_eq!(n, 8192);
        assert_eq!(read_buf, payload);

        // Partial cross-block read
        let mut cross_buf = [0u8; 100];
        let cn = writer.read_file(file_id, 4050, &mut cross_buf).unwrap();
        assert_eq!(cn, 100);
        assert_eq!(&cross_buf[..], &payload[4050..4150]);
    }

    #[test]
    fn test_unaligned_regular_file_extent_padding() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        // 5000 bytes (> 2048 bytes, not a multiple of 4096)
        let mut payload = Vec::new();
        for i in 0u32..5000 {
            payload.push((i % 179) as u8);
        }

        writer.begin_file("var/unaligned.dat", &meta).unwrap();
        writer.write_file(&payload).unwrap();
        let file_id = writer.end_file().unwrap();

        assert_eq!(writer.file_size(file_id), 5000);
        let inode = &writer.inodes[&file_id];
        assert_eq!(inode.size, 5000);
        // disk_num_bytes must be padded to a multiple of 4096
        assert_eq!(inode.disk_num_bytes, 8192);
        assert_eq!(inode.disk_num_bytes % BTRFS_SECTOR_SIZE as u64, 0);

        // Read back exact bytes
        let mut buf = alloc::vec![0u8; 5000];
        let n = writer.read_file(file_id, 0, &mut buf).unwrap();
        assert_eq!(n, 5000);
        assert_eq!(buf, payload);

        // Finish should succeed with aligned items
        writer.finish().unwrap();
    }

    #[test]
    fn test_symlink_creation_and_read() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta {
            mode: 0o777,
            mtime: 4000,
            ..Default::default()
        };

        let sym_id = writer.symlink("etc/localtime", "../usr/share/zoneinfo/UTC", &meta).unwrap();
        assert_eq!(writer.lookup("etc/localtime"), Some(sym_id));
        assert_eq!(writer.file_size(sym_id), 25);

        let mut buf = [0u8; 25];
        let n = writer.read_file(sym_id, 0, &mut buf).unwrap();
        assert_eq!(n, 25);
        assert_eq!(&buf, b"../usr/share/zoneinfo/UTC");
    }

    #[test]
    fn test_hardlink_functionality() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        writer.begin_file("etc/hosts", &meta).unwrap();
        writer.write_file(b"127.0.0.1 localhost\n").unwrap();
        let file_id = writer.end_file().unwrap();

        let link_id = writer.hardlink("etc/hosts.link", "etc/hosts").unwrap();
        assert_eq!(link_id, file_id);
        assert_eq!(writer.lookup("etc/hosts.link"), Some(file_id));

        // Error: hardlink to nonexistent
        assert_eq!(writer.hardlink("bad", "nonexistent"), Err(Error::NotFound));
        // Error: hardlink to dir
        writer.mkdir("dir", &meta).unwrap();
        assert_eq!(writer.hardlink("bad", "dir"), Err(Error::IsDir));
    }

    #[test]
    fn test_mknod_devices() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        let null_id = writer.mknod("dev/null", FileKind::CharDev, 1, 3, &meta).unwrap();
        assert_eq!(writer.lookup("dev/null"), Some(null_id));

        let sda_id = writer.mknod("dev/sda", FileKind::BlockDev, 8, 0, &meta).unwrap();
        assert_eq!(writer.lookup("dev/sda"), Some(sda_id));

        let fifo_id = writer.mknod("run/pipe", FileKind::Fifo, 0, 0, &meta).unwrap();
        assert_eq!(writer.lookup("run/pipe"), Some(fifo_id));

        // Invalid kind
        assert_eq!(
            writer.mknod("bad", FileKind::Regular, 0, 0, &meta),
            Err(Error::Invalid("not a device node or fifo"))
        );
    }

    #[test]
    fn test_subvolume_routing_at_and_home() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        // 1. In @ subvolume
        writer.begin_file("etc/issue", &meta).unwrap();
        writer.write_file(b"Arch Linux\n").unwrap();
        let issue_id = writer.end_file().unwrap();

        // 2. In @home subvolume
        writer.begin_file("home/user/.bashrc", &meta).unwrap();
        writer.write_file(b"export PATH=$PATH:~/bin\n").unwrap();
        let bashrc_id = writer.end_file().unwrap();

        assert_eq!(writer.lookup("etc/issue"), Some(issue_id));
        assert_eq!(writer.lookup("home/user/.bashrc"), Some(bashrc_id));

        // Finish and verify on-disk structures
        writer.finish().unwrap();

        // Verify superblock
        let mut sb = [0u8; 4096];
        writer.storage.read_at(65536, &mut sb).unwrap();
        assert_eq!(&sb[64..72], b"_BHRfS_M");
    }

    #[test]
    fn test_state_machine_errors() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        // write_file without begin_file
        assert_eq!(writer.write_file(b"data"), Err(Error::Invalid("no open file")));
        // end_file without begin_file
        assert_eq!(writer.end_file(), Err(Error::Invalid("no open file")));

        // double begin_file
        writer.begin_file("file1", &meta).unwrap();
        assert_eq!(writer.begin_file("file2", &meta), Err(Error::Invalid("file already open")));
        writer.end_file().unwrap();
    }

    #[test]
    fn test_disk_fswriter_polymorphism() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();

        // Treat as &mut dyn FsWriter
        let fs: &mut dyn FsWriter = &mut writer;
        let meta = Meta::default();

        let dir_id = fs.mkdir("var", &meta).unwrap();
        assert_eq!(fs.lookup("var"), Some(dir_id));

        fs.begin_file("var/test.txt", &meta).unwrap();
        fs.write_file(b"hello via FsWriter").unwrap();
        let file_id = fs.end_file().unwrap();
        assert_eq!(fs.file_size(file_id), 18);

        let mut buf = [0u8; 18];
        fs.read_file(file_id, 0, &mut buf).unwrap();
        assert_eq!(&buf, b"hello via FsWriter");

        fs.finish().unwrap();
    }

    #[test]
    fn test_error_conversions() {
        assert_eq!(to_disk_err(Error::Io), disk::Error::Io);
        assert_eq!(to_disk_err(Error::NoSpace), disk::Error::NoSpace);
        assert_eq!(to_disk_err(Error::NotFound), disk::Error::NotFound);
        assert_eq!(to_disk_err(Error::Exists), disk::Error::Exists);
        assert_eq!(to_disk_err(Error::NotDir), disk::Error::Invalid("directory type mismatch"));
        assert_eq!(to_disk_err(Error::IsDir), disk::Error::Invalid("directory type mismatch"));
        assert_eq!(to_disk_err(Error::Invalid("bad")), disk::Error::Invalid("bad"));

        assert_eq!(Error::from(disk::Error::Io), Error::Io);
        assert_eq!(Error::from(disk::Error::NoSpace), Error::NoSpace);
        assert_eq!(Error::from(disk::Error::NotFound), Error::NotFound);
        assert_eq!(Error::from(disk::Error::Exists), Error::Exists);
        assert_eq!(Error::from(disk::Error::Size("size")), Error::Invalid("size"));
        assert_eq!(Error::from(disk::Error::Invalid("inv")), Error::Invalid("inv"));
    }

    #[test]
    fn test_disk_too_small() {
        let storage = MemStorage::new(8 * 1024 * 1024);
        let res = BtrfsWriter::new(storage, 8 * 1024 * 1024, Options::default());
        assert_eq!(res.err(), Some(Error::Invalid("disk too small for btrfs (min 16 MiB)")));
    }

    #[test]
    fn test_idempotent_finish() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        writer.finish().unwrap();
        // Second call should return Ok without re-writing
        writer.finish().unwrap();
    }

    #[test]
    fn test_root_lookup_slash_and_empty() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        assert_eq!(writer.lookup(""), Some(256));
        assert_eq!(writer.lookup("/"), Some(256));
        assert_eq!(writer.lookup("home"), Some(256));
        assert_eq!(writer.lookup("/home"), Some(256));
    }

    #[test]
    fn test_cross_subvolume_hardlink_forbidden() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        writer.begin_file("etc/rootfile", &meta).unwrap();
        writer.write_file(b"data").unwrap();
        writer.end_file().unwrap();

        // Attempt cross-subvolume link from @ to @home
        assert_eq!(
            writer.hardlink("home/user/link", "etc/rootfile"),
            Err(Error::Invalid("cross-subvolume hardlink not permitted"))
        );
    }

    #[test]
    fn test_mkdir_parent_not_dir() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        writer.begin_file("etc/passwd", &meta).unwrap();
        writer.write_file(b"root:...").unwrap();
        writer.end_file().unwrap();

        // Attempt to create directory inside a file
        assert_eq!(writer.mkdir("etc/passwd/child", &meta), Err(Error::NotDir));
    }

    #[test]
    fn test_read_nonexistent_file() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let mut buf = [0u8; 10];
        assert_eq!(writer.read_file(9999, 0, &mut buf), Err(Error::NotFound));
    }

    #[test]
    fn test_begin_file_at_root_is_dir() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();
        assert_eq!(writer.begin_file("/", &meta), Err(Error::IsDir));
    }

    #[test]
    fn test_split_parent_helper() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        assert_eq!(writer.split_parent(""), ("", ""));
        assert_eq!(writer.split_parent("foo"), ("", "foo"));
        assert_eq!(writer.split_parent("foo/bar"), ("foo", "bar"));
        assert_eq!(writer.split_parent("a/b/c"), ("a/b", "c"));
    }

    #[test]
    fn test_route_path_helper() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        assert_eq!(writer.route_path("home/user/file"), (1, "user/file"));
        assert_eq!(writer.route_path("/home/user/file"), (1, "user/file"));
        assert_eq!(writer.route_path("home"), (1, ""));
        assert_eq!(writer.route_path("/home"), (1, ""));
        assert_eq!(writer.route_path("etc/issue"), (0, "etc/issue"));
        assert_eq!(writer.route_path("/etc/issue"), (0, "etc/issue"));
        assert_eq!(writer.route_path("./etc/issue"), (0, "etc/issue"));
    }

    #[test]
    fn test_dir_nlink_remains_one() {
        let storage = MemStorage::new(32 * 1024 * 1024);
        let mut writer = BtrfsWriter::new(storage, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        let parent_id = writer.mkdir("usr/lib", &meta).unwrap();
        let _child1_id = writer.mkdir("usr/lib/systemd", &meta).unwrap();
        let _child2_id = writer.mkdir("usr/lib/modules", &meta).unwrap();

        // In Btrfs on-disk format, directory nlink MUST be 1
        let parent = &writer.inodes[&parent_id];
        assert_eq!(parent.nlink, 1);
    }
}
