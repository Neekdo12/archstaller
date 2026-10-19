//! F2FS (Flash-Friendly File System) write-once filesystem generator.
//!
//! Provides bare-metal streaming filesystem generation implementing `disk::FsWriter`.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use disk::{Error, FileKind, FsWriter, Meta, Result, Storage};

pub const F2FS_SUPER_MAGIC: u32 = 0xF2F5_2010;
pub const F2FS_MAJOR_VERSION: u16 = 1;
pub const F2FS_MINOR_VERSION: u16 = 14;

pub const F2FS_BLKSIZE: usize = 4096;
pub const F2FS_BLKS_PER_SEG: u32 = 512;
pub const F2FS_SEG_SIZE: usize = F2FS_BLKSIZE * F2FS_BLKS_PER_SEG as usize; // 2 MiB

pub const F2FS_SUPER_OFFSET: usize = 1024;
pub const SB_CHKSUM_OFFSET: usize = 3068;
pub const CP_CHKSUM_OFFSET: usize = 4092;

pub const F2FS_ROOT_INO: u32 = 3;
pub const F2FS_NODE_INO: u32 = 1;
pub const F2FS_META_INO: u32 = 2;

pub const F2FS_FEATURE_SB_CHKSUM: u32 = 0x0800;

pub const CP_UMOUNT_FLAG: u32 = 0x0000_0001;
pub const CP_COMPACT_SUM_FLAG: u32 = 0x0000_0004;

pub const CURSEG_HOT_DATA: usize = 0;
pub const CURSEG_WARM_DATA: usize = 1;
pub const CURSEG_COLD_DATA: usize = 2;
pub const CURSEG_HOT_NODE: usize = 0;
pub const CURSEG_WARM_NODE: usize = 1;
pub const CURSEG_COLD_NODE: usize = 2;

pub const NR_DENTRY_IN_BLOCK: usize = 214;
pub const SIZE_OF_DIR_ENTRY: usize = 11;
pub const F2FS_SLOT_LEN: usize = 8;
pub const SIZE_OF_DENTRY_BITMAP: usize = 27;
pub const SIZE_OF_RESERVED: usize = 3; // 4096 - (214 * 19 + 27) = 3

pub const DEF_ADDRS_PER_INODE: usize = 923;
pub const DEF_ADDRS_PER_BLOCK: usize = 1018;
pub const DEF_NIDS_PER_INODE: usize = 5;

pub const F2FS_FT_UNKNOWN: u8 = 0;
pub const F2FS_FT_REG_FILE: u8 = 1;
pub const F2FS_FT_DIR: u8 = 2;
pub const F2FS_FT_CHRDEV: u8 = 3;
pub const F2FS_FT_BLKDEV: u8 = 4;
pub const F2FS_FT_FIFO: u8 = 5;
pub const F2FS_FT_SOCK: u8 = 6;
pub const F2FS_FT_SYMLINK: u8 = 7;

pub const S_IFREG: u16 = 0o100000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IFLNK: u16 = 0o120000;
pub const S_IFCHR: u16 = 0o020000;
pub const S_IFBLK: u16 = 0o060000;
pub const S_IFIFO: u16 = 0o010000;

const CRCPOLY_LE: u32 = 0xEDB8_8320;
const DELTA: u32 = 0x9E37_79B9;

/// Sets bit `nr` in an F2FS bitmap using big-endian bit order within each byte.
#[inline]
pub fn f2fs_set_bit(nr: usize, addr: &mut [u8]) {
    addr[nr >> 3] |= 1 << (7 - (nr & 7));
}

/// Tests bit `nr` in an F2FS bitmap using big-endian bit order within each byte.
#[inline]
pub fn f2fs_test_bit(nr: usize, addr: &[u8]) -> bool {
    (addr[nr >> 3] & (1 << (7 - (nr & 7)))) != 0
}

/// Sets bit `nr` in an F2FS bitmap using little-endian bit order within each byte.
#[inline]
pub fn f2fs_set_bit_le(nr: usize, addr: &mut [u8]) {
    addr[nr >> 3] |= 1 << (nr & 7);
}

/// Tests bit `nr` in an F2FS bitmap using little-endian bit order within each byte.
#[inline]
pub fn f2fs_test_bit_le(nr: usize, addr: &[u8]) -> bool {
    (addr[nr >> 3] & (1 << (nr & 7))) != 0
}

/// Computes the F2FS CRC32 checksum with polynomial 0xEDB88320.
pub fn f2fs_crc32(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if (crc & 1) != 0 { CRCPOLY_LE } else { 0 };
        }
    }
    crc
}

/// Helper converting characters into packed 32-bit integer array for TEA.
pub fn str2hashbuf(msg: &[u8], buf: &mut [u32; 4]) {
    let len = msg.len();
    let mut pad = (len as u32) | ((len as u32) << 8);
    pad |= pad << 16;
    let mut val = pad;
    let mut num = 4usize;
    let limit = len.min(16);
    let mut buf_idx = 0;

    for (i, &b) in msg.iter().enumerate().take(limit) {
        if i % 4 == 0 {
            val = pad;
        }
        val = (b as u32).wrapping_add(val << 8);
        if i % 4 == 3 {
            buf[buf_idx] = val;
            buf_idx += 1;
            val = pad;
            num = num.saturating_sub(1);
        }
    }
    if num > 0 {
        buf[buf_idx] = val;
        buf_idx += 1;
        num = num.saturating_sub(1);
    }
    while num > 0 {
        buf[buf_idx] = pad;
        buf_idx += 1;
        num = num.saturating_sub(1);
    }
}

/// TEA (Tiny Encryption Algorithm) transform round function.
pub fn tea_transform(buf: &mut [u32; 4], input: &[u32; 4]) {
    let mut sum: u32 = 0;
    let mut b0 = buf[0];
    let mut b1 = buf[1];
    let a = input[0];
    let b = input[1];
    let c = input[2];
    let d = input[3];

    for _ in 0..16 {
        sum = sum.wrapping_add(DELTA);
        b0 = b0.wrapping_add(
            ((b1 << 4).wrapping_add(a)) ^ (b1.wrapping_add(sum)) ^ ((b1 >> 5).wrapping_add(b)),
        );
        b1 = b1.wrapping_add(
            ((b0 << 4).wrapping_add(c)) ^ (b0.wrapping_add(sum)) ^ ((b0 >> 5).wrapping_add(d)),
        );
    }

    buf[0] = buf[0].wrapping_add(b0);
    buf[1] = buf[1].wrapping_add(b1);
}

/// Computes the 32-bit F2FS directory entry hash for a name.
pub fn f2fs_dentry_hash(name: &[u8]) -> u32 {
    if (name.len() == 1 && name[0] == b'.')
        || (name.len() == 2 && name[0] == b'.' && name[1] == b'.')
    {
        return 0;
    }

    let mut buf = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476];
    let mut p = name;

    loop {
        let mut input = [0u32; 4];
        str2hashbuf(p, &mut input);
        tea_transform(&mut buf, &input);
        if p.len() <= 16 {
            break;
        }
        p = &p[16..];
    }

    buf[0]
}

/// Linux device node number encoding.
pub fn encode_dev(major: u32, minor: u32) -> u32 {
    (minor & 0xff) | ((major & 0xfff) << 8) | ((minor & !0xff) << 12)
}

/// Linux device node number decoding.
pub fn decode_dev(dev: u32) -> (u32, u32) {
    let major = (dev >> 8) & 0xfff;
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    (major, minor)
}

/// Splits a path into cleaned components.
pub fn components(path: &str) -> Result<Vec<&str>> {
    let mut out = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => return Err(Error::Invalid("'..' in path")),
            c => {
                if c.len() > 255 {
                    return Err(Error::Invalid("name too long"));
                }
                out.push(c);
            }
        }
    }
    Ok(out)
}

/// Filesystem creation options.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub label: String,
    pub uuid: [u8; 16],
    pub now: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct F2fsLayout {
    pub total_segments: u32,
    pub segment0_blkaddr: u32,
    pub cp_blkaddr: u32,
    pub segment_count_ckpt: u32,
    pub sit_blkaddr: u32,
    pub segment_count_sit: u32,
    pub nat_blkaddr: u32,
    pub segment_count_nat: u32,
    pub ssa_blkaddr: u32,
    pub segment_count_ssa: u32,
    pub main_blkaddr: u32,
    pub segment_count_main: u32,
    pub rsvd_segments: u32,
    pub overprov_segments: u32,
    pub user_block_count: u64,
}

/// Computes the disk layout geometry for a given partition size in bytes.
pub fn compute_layout(size_bytes: u64) -> Result<F2fsLayout> {
    let total_blocks = size_bytes / F2FS_BLKSIZE as u64;
    let total_segments = (total_blocks / F2FS_BLKS_PER_SEG as u64) as u32;

    // Minimum metadata segments:
    // 1 (Superblock area) + 2 (CP) + 2 (SIT) + 2 (NAT) + 1 (SSA) = 8
    // plus at least 6 main segments for the 6 active logs.
    const MIN_TOTAL_SEGMENTS: u32 = 14;
    if total_segments < MIN_TOTAL_SEGMENTS {
        return Err(Error::Size("partition too small for F2FS (minimum 28 MiB)"));
    }

    let segment0_blkaddr = F2FS_BLKS_PER_SEG; // 512 (2 MiB)
    let cp_blkaddr = segment0_blkaddr;
    let segment_count_ckpt = 2;

    let sit_blkaddr = cp_blkaddr + segment_count_ckpt * F2FS_BLKS_PER_SEG; // 1536
    let segment_count_sit = 2;

    let nat_blkaddr = sit_blkaddr + segment_count_sit * F2FS_BLKS_PER_SEG; // 2560
    let segment_count_nat = 2;

    let ssa_blkaddr = nat_blkaddr + segment_count_nat * F2FS_BLKS_PER_SEG; // 3584
    let segment_count_ssa = 1;

    let main_blkaddr = ssa_blkaddr + segment_count_ssa * F2FS_BLKS_PER_SEG; // 4096 (16 MiB)
    let meta_segments = 1 + segment_count_ckpt + segment_count_sit + segment_count_nat + segment_count_ssa;
    let segment_count_main = total_segments.saturating_sub(meta_segments);

    if segment_count_main < 6 {
        return Err(Error::Size("not enough main segments for F2FS active logs"));
    }

    let rsvd_segments = (segment_count_main / 20).max(1);
    let overprov_segments = (segment_count_main / 10).max(2) + rsvd_segments;
    let user_block_count = (segment_count_main.saturating_sub(overprov_segments) as u64) * F2FS_BLKS_PER_SEG as u64;

    Ok(F2fsLayout {
        total_segments,
        segment0_blkaddr,
        cp_blkaddr,
        segment_count_ckpt,
        sit_blkaddr,
        segment_count_sit,
        nat_blkaddr,
        segment_count_nat,
        ssa_blkaddr,
        segment_count_ssa,
        main_blkaddr,
        segment_count_main,
        rsvd_segments,
        overprov_segments,
        user_block_count,
    })
}

#[derive(Clone, Debug)]
pub struct Dentry {
    pub ino: u32,
    pub name: String,
    pub file_type: u8,
}

#[derive(Clone, Debug)]
pub struct InodeRecord {
    pub ino: u32,
    pub pino: u32,
    pub name: String,
    pub kind: FileKind,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub mtime: u64,
    pub links: u32,
    pub rdev: (u32, u32),
    pub direct_blocks: Vec<u32>,
    pub indirect_blocks: Vec<u32>, // direct node blocks holding more data blkaddrs
    pub dentry_block_addrs: Vec<u32>,
}

#[derive(Debug)]
pub struct OpenFile {
    pub ino: u32,
    pub buffer: Vec<u8>,
    pub total_bytes_written: u64,
}

/// Streaming F2FS image writer.
pub struct F2fsWriter<S> {
    storage: S,
    options: Options,
    layout: F2fsLayout,
    next_free_nid: u32,

    // Active log segment indices in Main Area
    cur_node_seg: [u32; 3],
    cur_node_off: [u16; 3],
    cur_data_seg: [u32; 3],
    cur_data_off: [u16; 3],

    next_free_main_seg: u32,

    // Directory tree & inode tracking
    inodes: BTreeMap<u32, InodeRecord>,
    dir_entries: BTreeMap<u32, Vec<Dentry>>,
    path_to_ino: BTreeMap<String, u32>,

    open_file: Option<OpenFile>,
    finished: bool,
}

impl<S: Storage> F2fsWriter<S> {
    /// Formats a new F2FS filesystem of `size_bytes` on `storage`.
    pub fn new(storage: S, size_bytes: u64, options: Options) -> Result<Self> {
        let layout = compute_layout(size_bytes)?;

        // Allocate the first 6 segments of main area for the 6 active logs:
        // HOT_NODE (0), WARM_NODE (1), COLD_NODE (2),
        // HOT_DATA (3), WARM_DATA (4), COLD_DATA (5).
        let cur_node_seg = [0, 1, 2];
        let cur_node_off = [0, 0, 0];
        let cur_data_seg = [3, 4, 5];
        let cur_data_off = [0, 0, 0];
        let next_free_main_seg = 6;

        let mut writer = Self {
            storage,
            options,
            layout,
            next_free_nid: F2FS_ROOT_INO + 1, // 4
            cur_node_seg,
            cur_node_off,
            cur_data_seg,
            cur_data_off,
            next_free_main_seg,
            inodes: BTreeMap::new(),
            dir_entries: BTreeMap::new(),
            path_to_ino: BTreeMap::new(),
            open_file: None,
            finished: false,
        };

        // Initialize root directory at F2FS_ROOT_INO (3)
        let root_meta = Meta {
            mode: 0o755,
            uid: 0,
            gid: 0,
            mtime: writer.options.now,
            xattrs: Vec::new(),
        };
        writer.init_root_dir(&root_meta)?;

        Ok(writer)
    }

    fn init_root_dir(&mut self, meta: &Meta) -> Result<()> {
        let root_ino = F2FS_ROOT_INO;
        let root_rec = InodeRecord {
            ino: root_ino,
            pino: root_ino,
            name: String::new(),
            kind: FileKind::Dir,
            mode: S_IFDIR | (meta.mode & 0o7777),
            uid: meta.uid,
            gid: meta.gid,
            size: F2FS_BLKSIZE as u64,
            mtime: meta.mtime,
            links: 2,
            rdev: (0, 0),
            direct_blocks: Vec::new(),
            indirect_blocks: Vec::new(),
            dentry_block_addrs: Vec::new(),
        };

        self.inodes.insert(root_ino, root_rec);
        self.path_to_ino.insert(String::new(), root_ino);

        let entries = vec![
            Dentry {
                ino: root_ino,
                name: String::from("."),
                file_type: F2FS_FT_DIR,
            },
            Dentry {
                ino: root_ino,
                name: String::from(".."),
                file_type: F2FS_FT_DIR,
            },
        ];
        self.dir_entries.insert(root_ino, entries);

        Ok(())
    }

    /// Allocates the next block in one of the active data logs.
    fn alloc_data_block(&mut self, log_type: usize) -> Result<u32> {
        if self.cur_data_off[log_type] >= F2FS_BLKS_PER_SEG as u16 {
            if self.next_free_main_seg >= self.layout.segment_count_main {
                return Err(Error::NoSpace);
            }
            self.cur_data_seg[log_type] = self.next_free_main_seg;
            self.cur_data_off[log_type] = 0;
            self.next_free_main_seg += 1;
        }

        let pblk = self.layout.main_blkaddr
            + self.cur_data_seg[log_type] * F2FS_BLKS_PER_SEG
            + self.cur_data_off[log_type] as u32;
        self.cur_data_off[log_type] += 1;
        Ok(pblk)
    }

    /// Allocates the next block in one of the active node logs.
    fn alloc_node_block(&mut self, log_type: usize) -> Result<u32> {
        if self.cur_node_off[log_type] >= F2FS_BLKS_PER_SEG as u16 {
            if self.next_free_main_seg >= self.layout.segment_count_main {
                return Err(Error::NoSpace);
            }
            self.cur_node_seg[log_type] = self.next_free_main_seg;
            self.cur_node_off[log_type] = 0;
            self.next_free_main_seg += 1;
        }

        let pblk = self.layout.main_blkaddr
            + self.cur_node_seg[log_type] * F2FS_BLKS_PER_SEG
            + self.cur_node_off[log_type] as u32;
        self.cur_node_off[log_type] += 1;
        Ok(pblk)
    }

    fn ensure_parent_and_name(&mut self, path: &str) -> Result<(u32, String)> {
        let parts = components(path)?;
        if parts.is_empty() {
            return Err(Error::Invalid("empty path"));
        }
        let (leaf, parent_parts) = parts.split_last().ok_or(Error::Invalid("empty path"))?;
        let leaf_str = String::from(*leaf);

        let mut curr_ino = F2FS_ROOT_INO;
        let mut path_prefix = String::new();
        for &p in parent_parts {
            if !path_prefix.is_empty() {
                path_prefix.push('/');
            }
            path_prefix.push_str(p);

            let existing = self.dir_entries.get(&curr_ino).and_then(|entries| {
                entries.iter().find(|e| e.name == p).map(|e| (e.ino, e.file_type))
            });

            match existing {
                Some((ino, F2FS_FT_DIR)) => {
                    curr_ino = ino;
                }
                Some(_) => {
                    return Err(Error::Invalid("component is not a directory"));
                }
                None => {
                    let new_dir_ino = self.next_free_nid;
                    self.next_free_nid += 1;
                    let now = self.options.now;
                    let rec = InodeRecord {
                        ino: new_dir_ino,
                        pino: curr_ino,
                        name: String::from(p),
                        kind: FileKind::Dir,
                        mode: S_IFDIR | 0o755,
                        uid: 0,
                        gid: 0,
                        size: F2FS_BLKSIZE as u64,
                        mtime: now,
                        links: 2,
                        rdev: (0, 0),
                        direct_blocks: Vec::new(),
                        indirect_blocks: Vec::new(),
                        dentry_block_addrs: Vec::new(),
                    };
                    self.inodes.insert(new_dir_ino, rec);

                    let entries = vec![
                        Dentry {
                            ino: new_dir_ino,
                            name: String::from("."),
                            file_type: F2FS_FT_DIR,
                        },
                        Dentry {
                            ino: curr_ino,
                            name: String::from(".."),
                            file_type: F2FS_FT_DIR,
                        },
                    ];
                    self.dir_entries.insert(new_dir_ino, entries);

                    self.add_entry_to_parent(curr_ino, new_dir_ino, p, F2FS_FT_DIR)?;
                    self.path_to_ino.insert(path_prefix.clone(), new_dir_ino);

                    curr_ino = new_dir_ino;
                }
            }
        }

        Ok((curr_ino, leaf_str))
    }

    fn add_entry_to_parent(&mut self, parent_ino: u32, ino: u32, name: &str, file_type: u8) -> Result<()> {
        let entries = self.dir_entries.get_mut(&parent_ino).ok_or(Error::NotFound)?;
        if entries.iter().any(|e| e.name == name) {
            return Err(Error::Exists);
        }
        entries.push(Dentry {
            ino,
            name: String::from(name),
            file_type,
        });

        // Update directory size: each 214 slots spans 4096 bytes
        let total_slots: usize = entries
            .iter()
            .map(|e| e.name.len().div_ceil(F2FS_SLOT_LEN))
            .sum();
        let blks = total_slots.div_ceil(NR_DENTRY_IN_BLOCK);
        if let Some(parent_node) = self.inodes.get_mut(&parent_ino) {
            parent_node.size = (blks.max(1) * F2FS_BLKSIZE) as u64;
            if file_type == F2FS_FT_DIR {
                parent_node.links += 1;
            }
        }

        Ok(())
    }

    fn check_new_entry(&mut self, path: &str) -> Result<(u32, String)> {
        let (parent_ino, name) = self.ensure_parent_and_name(path)?;
        if let Some(entries) = self.dir_entries.get(&parent_ino) {
            if entries.iter().any(|e| e.name == name) {
                return Err(Error::Exists);
            }
        }
        Ok((parent_ino, name))
    }

    pub fn mkdir(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        if self.open_file.is_some() {
            return Err(Error::Invalid("file currently open"));
        }
        let parts = components(path)?;
        if parts.is_empty() {
            if let Some(root_node) = self.inodes.get_mut(&F2FS_ROOT_INO) {
                root_node.mode = S_IFDIR | (meta.mode & 0o7777);
                root_node.uid = meta.uid;
                root_node.gid = meta.gid;
                root_node.mtime = meta.mtime;
            }
            return Ok(F2FS_ROOT_INO as u64);
        }

        let (parent_ino, name) = self.ensure_parent_and_name(path)?;
        if let Some(existing) = self.dir_entries.get(&parent_ino).and_then(|entries| {
            entries.iter().find(|e| e.name == name).map(|e| (e.ino, e.file_type))
        }) {
            if existing.1 == F2FS_FT_DIR {
                if let Some(node) = self.inodes.get_mut(&existing.0) {
                    node.mode = S_IFDIR | (meta.mode & 0o7777);
                    node.uid = meta.uid;
                    node.gid = meta.gid;
                    node.mtime = meta.mtime;
                }
                return Ok(existing.0 as u64);
            }
            return Err(Error::Exists);
        }

        let ino = self.next_free_nid;
        self.next_free_nid += 1;

        let rec = InodeRecord {
            ino,
            pino: parent_ino,
            name: name.clone(),
            kind: FileKind::Dir,
            mode: S_IFDIR | (meta.mode & 0o7777),
            uid: meta.uid,
            gid: meta.gid,
            size: F2FS_BLKSIZE as u64,
            mtime: meta.mtime,
            links: 2,
            rdev: (0, 0),
            direct_blocks: Vec::new(),
            indirect_blocks: Vec::new(),
            dentry_block_addrs: Vec::new(),
        };

        self.inodes.insert(ino, rec);

        let entries = vec![
            Dentry {
                ino,
                name: String::from("."),
                file_type: F2FS_FT_DIR,
            },
            Dentry {
                ino: parent_ino,
                name: String::from(".."),
                file_type: F2FS_FT_DIR,
            },
        ];
        self.dir_entries.insert(ino, entries);

        self.add_entry_to_parent(parent_ino, ino, &name, F2FS_FT_DIR)?;

        let full_clean = components(path)?.join("/");
        self.path_to_ino.insert(full_clean, ino);

        Ok(ino as u64)
    }

    pub fn begin_file(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        if self.open_file.is_some() {
            return Err(Error::Invalid("file already open"));
        }
        let (parent_ino, name) = self.check_new_entry(path)?;
        let ino = self.next_free_nid;
        self.next_free_nid += 1;

        let rec = InodeRecord {
            ino,
            pino: parent_ino,
            name: name.clone(),
            kind: FileKind::Regular,
            mode: S_IFREG | (meta.mode & 0o7777),
            uid: meta.uid,
            gid: meta.gid,
            size: 0,
            mtime: meta.mtime,
            links: 1,
            rdev: (0, 0),
            direct_blocks: Vec::new(),
            indirect_blocks: Vec::new(),
            dentry_block_addrs: Vec::new(),
        };

        self.inodes.insert(ino, rec);
        self.add_entry_to_parent(parent_ino, ino, &name, F2FS_FT_REG_FILE)?;

        let full_clean = components(path)?.join("/");
        self.path_to_ino.insert(full_clean, ino);

        self.open_file = Some(OpenFile {
            ino,
            buffer: Vec::new(),
            total_bytes_written: 0,
        });

        Ok(ino as u64)
    }

    pub fn write_file(&mut self, data: &[u8]) -> Result<()> {
        let mut open = self.open_file.take().ok_or(Error::Invalid("no open file"))?;
        open.buffer.extend_from_slice(data);
        open.total_bytes_written += data.len() as u64;
        let ino = open.ino;

        // Stream full 4096-byte blocks to storage immediately
        while open.buffer.len() >= F2FS_BLKSIZE {
            let chunk = open.buffer.drain(..F2FS_BLKSIZE).collect::<Vec<u8>>();
            let pblk = match self.alloc_data_block(CURSEG_WARM_DATA) {
                Ok(b) => b,
                Err(e) => {
                    self.open_file = Some(open);
                    return Err(e);
                }
            };
            if let Err(e) = self.storage.write_at(pblk as u64 * F2FS_BLKSIZE as u64, &chunk) {
                self.open_file = Some(open);
                return Err(e);
            }

            let Some(inode) = self.inodes.get_mut(&ino) else {
                self.open_file = Some(open);
                return Err(Error::NotFound);
            };
            if inode.direct_blocks.len() < DEF_ADDRS_PER_INODE {
                inode.direct_blocks.push(pblk);
            } else {
                inode.indirect_blocks.push(pblk);
            }
        }

        self.open_file = Some(open);
        Ok(())
    }

    pub fn end_file(&mut self) -> Result<u64> {
        let Some(open) = self.open_file.take() else {
            return Err(Error::Invalid("no open file"));
        };

        if !open.buffer.is_empty() {
            let pblk = self.alloc_data_block(CURSEG_WARM_DATA)?;
            let mut padded = vec![0u8; F2FS_BLKSIZE];
            padded[..open.buffer.len()].copy_from_slice(&open.buffer);
            self.storage.write_at(pblk as u64 * F2FS_BLKSIZE as u64, &padded)?;

            let inode = self.inodes.get_mut(&open.ino).ok_or(Error::NotFound)?;
            if inode.direct_blocks.len() < DEF_ADDRS_PER_INODE {
                inode.direct_blocks.push(pblk);
            } else {
                inode.indirect_blocks.push(pblk);
            }
        }

        let inode = self.inodes.get_mut(&open.ino).ok_or(Error::NotFound)?;
        inode.size = open.total_bytes_written;

        Ok(open.ino as u64)
    }

    pub fn symlink(&mut self, path: &str, target: &str, meta: &Meta) -> Result<u64> {
        if self.open_file.is_some() {
            return Err(Error::Invalid("file already open"));
        }
        let (parent_ino, name) = self.check_new_entry(path)?;
        let ino = self.next_free_nid;
        self.next_free_nid += 1;

        let target_bytes = target.as_bytes();
        if target_bytes.len() > F2FS_BLKSIZE {
            return Err(Error::Invalid("symlink target too long"));
        }

        let pblk = self.alloc_data_block(CURSEG_WARM_DATA)?;
        let mut blk = vec![0u8; F2FS_BLKSIZE];
        blk[..target_bytes.len()].copy_from_slice(target_bytes);
        self.storage.write_at(pblk as u64 * F2FS_BLKSIZE as u64, &blk)?;

        let rec = InodeRecord {
            ino,
            pino: parent_ino,
            name: name.clone(),
            kind: FileKind::Symlink,
            mode: S_IFLNK | (meta.mode & 0o7777),
            uid: meta.uid,
            gid: meta.gid,
            size: target_bytes.len() as u64,
            mtime: meta.mtime,
            links: 1,
            rdev: (0, 0),
            direct_blocks: vec![pblk],
            indirect_blocks: Vec::new(),
            dentry_block_addrs: Vec::new(),
        };

        self.inodes.insert(ino, rec);
        self.add_entry_to_parent(parent_ino, ino, &name, F2FS_FT_SYMLINK)?;

        let full_clean = components(path)?.join("/");
        self.path_to_ino.insert(full_clean, ino);

        Ok(ino as u64)
    }

    pub fn hardlink(&mut self, path: &str, existing: &str) -> Result<u64> {
        if self.open_file.is_some() {
            return Err(Error::Invalid("file already open"));
        }
        let existing_clean = components(existing)?.join("/");
        let Some(&ino) = self.path_to_ino.get(&existing_clean) else {
            return Err(Error::NotFound);
        };

        let (parent_ino, name) = self.check_new_entry(path)?;
        let kind = {
            let inode = self.inodes.get_mut(&ino).ok_or(Error::NotFound)?;
            if inode.kind == FileKind::Dir {
                return Err(Error::Invalid("cannot hardlink a directory"));
            }
            inode.links += 1;
            inode.kind
        };

        let ft = match kind {
            FileKind::Regular => F2FS_FT_REG_FILE,
            FileKind::Symlink => F2FS_FT_SYMLINK,
            FileKind::CharDev => F2FS_FT_CHRDEV,
            FileKind::BlockDev => F2FS_FT_BLKDEV,
            FileKind::Fifo => F2FS_FT_FIFO,
            _ => F2FS_FT_UNKNOWN,
        };

        self.add_entry_to_parent(parent_ino, ino, &name, ft)?;
        let full_clean = components(path)?.join("/");
        self.path_to_ino.insert(full_clean, ino);

        Ok(ino as u64)
    }

    pub fn mknod(&mut self, path: &str, kind: FileKind, major: u32, minor: u32, meta: &Meta) -> Result<u64> {
        if self.open_file.is_some() {
            return Err(Error::Invalid("file already open"));
        }
        let (parent_ino, name) = self.check_new_entry(path)?;
        let ino = self.next_free_nid;
        self.next_free_nid += 1;

        let (mode_flag, ft) = match kind {
            FileKind::CharDev => (S_IFCHR, F2FS_FT_CHRDEV),
            FileKind::BlockDev => (S_IFBLK, F2FS_FT_BLKDEV),
            FileKind::Fifo => (S_IFIFO, F2FS_FT_FIFO),
            _ => return Err(Error::Invalid("unsupported special file kind")),
        };

        let rec = InodeRecord {
            ino,
            pino: parent_ino,
            name: name.clone(),
            kind,
            mode: mode_flag | (meta.mode & 0o7777),
            uid: meta.uid,
            gid: meta.gid,
            size: 0,
            mtime: meta.mtime,
            links: 1,
            rdev: (major, minor),
            direct_blocks: Vec::new(),
            indirect_blocks: Vec::new(),
            dentry_block_addrs: Vec::new(),
        };

        self.inodes.insert(ino, rec);
        self.add_entry_to_parent(parent_ino, ino, &name, ft)?;

        let full_clean = components(path)?.join("/");
        self.path_to_ino.insert(full_clean, ino);

        Ok(ino as u64)
    }

    pub fn lookup(&self, path: &str) -> Option<u64> {
        let parts = components(path).ok()?;
        let mut curr_ino = F2FS_ROOT_INO;
        for p in parts {
            let entries = self.dir_entries.get(&curr_ino)?;
            let found = entries.iter().find(|e| e.name == p)?;
            curr_ino = found.ino;
        }
        Some(curr_ino as u64)
    }

    pub fn file_size(&self, id: u64) -> u64 {
        let ino = id as u32;
        if let Some(ref open) = self.open_file {
            if open.ino == ino {
                return open.total_bytes_written;
            }
        }
        self.inodes.get(&ino).map_or(0, |i| i.size)
    }

    pub fn read_file(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let ino = id as u32;
        let Some(inode) = self.inodes.get(&ino) else {
            return Err(Error::NotFound);
        };

        let size = if let Some(ref open) = self.open_file {
            if open.ino == ino {
                open.total_bytes_written
            } else {
                inode.size
            }
        } else {
            inode.size
        };

        if offset >= size {
            return Ok(0);
        }

        let to_read = ((size - offset) as usize).min(buf.len());
        let mut bytes_read = 0;

        while bytes_read < to_read {
            let curr_offset = offset + bytes_read as u64;
            let blk_idx = (curr_offset / F2FS_BLKSIZE as u64) as usize;
            let off_in_blk = (curr_offset % F2FS_BLKSIZE as u64) as usize;
            let chunk = (F2FS_BLKSIZE - off_in_blk).min(to_read - bytes_read);

            let all_blocks = if inode.direct_blocks.len() > blk_idx {
                Some(inode.direct_blocks[blk_idx])
            } else if inode.direct_blocks.len() + inode.indirect_blocks.len() > blk_idx {
                Some(inode.indirect_blocks[blk_idx - inode.direct_blocks.len()])
            } else {
                None
            };

            if let Some(pblk) = all_blocks {
                let mut blk_data = [0u8; F2FS_BLKSIZE];
                self.storage.read_at(pblk as u64 * F2FS_BLKSIZE as u64, &mut blk_data)?;
                buf[bytes_read..bytes_read + chunk].copy_from_slice(&blk_data[off_in_blk..off_in_blk + chunk]);
            } else if let Some(ref open) = self.open_file {
                if open.ino == ino {
                    let flushed_bytes = (inode.direct_blocks.len() + inode.indirect_blocks.len()) as u64 * F2FS_BLKSIZE as u64;
                    if curr_offset >= flushed_bytes {
                        let buf_off = (curr_offset - flushed_bytes) as usize;
                        if buf_off < open.buffer.len() {
                            let available = (open.buffer.len() - buf_off).min(chunk);
                            buf[bytes_read..bytes_read + available]
                                .copy_from_slice(&open.buffer[buf_off..buf_off + available]);
                        }
                    }
                }
            }

            bytes_read += chunk;
        }

        Ok(bytes_read)
    }

    /// Serializes and writes a directory's entries into dentry blocks on disk.
    fn write_directory_dentry_blocks(&mut self, dir_ino: u32) -> Result<Vec<u32>> {
        let entries = self.dir_entries.get(&dir_ino).cloned().unwrap_or_default();
        let mut allocated_pblks = Vec::new();

        let mut current_block = vec![0u8; F2FS_BLKSIZE];
        let mut current_slot = 0usize;

        for entry in entries {
            let name_bytes = entry.name.as_bytes();
            let name_len = name_bytes.len();
            let slots_needed = name_len.div_ceil(F2FS_SLOT_LEN);

            if current_slot + slots_needed > NR_DENTRY_IN_BLOCK {
                // Flush full dentry block
                let pblk = self.alloc_data_block(CURSEG_HOT_DATA)?;
                self.storage.write_at(pblk as u64 * F2FS_BLKSIZE as u64, &current_block)?;
                allocated_pblks.push(pblk);

                current_block = vec![0u8; F2FS_BLKSIZE];
                current_slot = 0;
            }

            let hash = f2fs_dentry_hash(name_bytes);
            let dentry_offset = SIZE_OF_DENTRY_BITMAP + SIZE_OF_RESERVED + current_slot * SIZE_OF_DIR_ENTRY;

            // Pack struct f2fs_dir_entry (11 bytes):
            // hash_code: le32, ino: le32, name_len: le16, file_type: u8
            current_block[dentry_offset..dentry_offset + 4].copy_from_slice(&hash.to_le_bytes());
            current_block[dentry_offset + 4..dentry_offset + 8].copy_from_slice(&entry.ino.to_le_bytes());
            current_block[dentry_offset + 8..dentry_offset + 10].copy_from_slice(&(name_len as u16).to_le_bytes());
            current_block[dentry_offset + 10] = entry.file_type;

            let filename_start = SIZE_OF_DENTRY_BITMAP + SIZE_OF_RESERVED + NR_DENTRY_IN_BLOCK * SIZE_OF_DIR_ENTRY;
            for k in 0..slots_needed {
                let slot_idx = current_slot + k;
                // Set bitmap bit using LE bit order
                f2fs_set_bit_le(slot_idx, &mut current_block);

                let fn_offset = filename_start + slot_idx * F2FS_SLOT_LEN;
                let start = k * F2FS_SLOT_LEN;
                let end = (start + F2FS_SLOT_LEN).min(name_len);
                if start < name_len {
                    current_block[fn_offset..fn_offset + (end - start)].copy_from_slice(&name_bytes[start..end]);
                }
            }

            current_slot += slots_needed;
        }

        // Flush last dentry block
        let pblk = self.alloc_data_block(CURSEG_HOT_DATA)?;
        self.storage.write_at(pblk as u64 * F2FS_BLKSIZE as u64, &current_block)?;
        allocated_pblks.push(pblk);

        Ok(allocated_pblks)
    }

    /// Serializes an inode into a 4096-byte node block.
    fn serialize_inode_block(
        &self,
        rec: &InodeRecord,
        cp_ver: u64,
        next_blkaddr: u32,
    ) -> [u8; F2FS_BLKSIZE] {
        let mut blk = [0u8; F2FS_BLKSIZE];

        // i_mode: le16
        blk[0..2].copy_from_slice(&rec.mode.to_le_bytes());
        // i_advise: u8
        blk[2] = 0;
        // i_inline: u8
        blk[3] = 0;
        // i_uid: le32
        blk[4..8].copy_from_slice(&rec.uid.to_le_bytes());
        // i_gid: le32
        blk[8..12].copy_from_slice(&rec.gid.to_le_bytes());
        // i_links: le32
        blk[12..16].copy_from_slice(&rec.links.to_le_bytes());
        // i_size: le64
        blk[16..24].copy_from_slice(&rec.size.to_le_bytes());
        // i_blocks: le64 (1 node block + data blocks)
        let total_blocks = 1 + (rec.direct_blocks.len() + rec.indirect_blocks.len() + rec.dentry_block_addrs.len()) as u64;
        blk[24..32].copy_from_slice(&total_blocks.to_le_bytes());
        // i_atime, i_ctime, i_mtime: le64
        blk[32..40].copy_from_slice(&rec.mtime.to_le_bytes());
        blk[40..48].copy_from_slice(&rec.mtime.to_le_bytes());
        blk[48..56].copy_from_slice(&rec.mtime.to_le_bytes());
        // current_depth for dir: 1
        if rec.kind == FileKind::Dir {
            blk[72..76].copy_from_slice(&1u32.to_le_bytes());
        }

        // i_pino: le32 at offset 84..88
        blk[84..88].copy_from_slice(&rec.pino.to_le_bytes());
        // i_namelen: le32 at offset 88..92
        let name_bytes = rec.name.as_bytes();
        let name_len = name_bytes.len().min(255);
        blk[88..92].copy_from_slice(&(name_len as u32).to_le_bytes());
        // i_name: offset 92..92+name_len
        blk[92..92 + name_len].copy_from_slice(&name_bytes[..name_len]);

        // Device encoding for char/block devices
        if rec.kind == FileKind::CharDev || rec.kind == FileKind::BlockDev {
            let dev = encode_dev(rec.rdev.0, rec.rdev.1);
            blk[360..364].copy_from_slice(&dev.to_le_bytes());
        } else {
            // Data block pointers in i_addr: offset 360 .. 360 + 923 * 4
            let all_data = if rec.kind == FileKind::Dir {
                &rec.dentry_block_addrs
            } else {
                &rec.direct_blocks
            };
            for (idx, &pblk) in all_data.iter().take(DEF_ADDRS_PER_INODE).enumerate() {
                let off = 360 + idx * 4;
                blk[off..off + 4].copy_from_slice(&pblk.to_le_bytes());
            }
        }

        // Node footer (24 bytes at offset 4072..4096):
        // nid: le32, ino: le32, flag: le32, cp_ver: le64, next_blkaddr: le32
        blk[4072..4076].copy_from_slice(&rec.ino.to_le_bytes());
        blk[4076..4080].copy_from_slice(&rec.ino.to_le_bytes());
        blk[4080..4084].copy_from_slice(&0u32.to_le_bytes());
        blk[4084..4092].copy_from_slice(&cp_ver.to_le_bytes());
        blk[4092..4096].copy_from_slice(&next_blkaddr.to_le_bytes());

        blk
    }

    /// Serializes the Superblock into a 3072-byte buffer.
    fn serialize_super_block(&self) -> [u8; 3072] {
        let mut sb = [0u8; 3072];

        // magic: 0xF2F52010
        sb[0..4].copy_from_slice(&F2FS_SUPER_MAGIC.to_le_bytes());
        // major_ver: 1, minor_ver: 14
        sb[4..6].copy_from_slice(&F2FS_MAJOR_VERSION.to_le_bytes());
        sb[6..8].copy_from_slice(&F2FS_MINOR_VERSION.to_le_bytes());

        // log_sectorsize: 9, log_sectors_per_block: 3
        sb[8..12].copy_from_slice(&9u32.to_le_bytes());
        sb[12..16].copy_from_slice(&3u32.to_le_bytes());
        // log_blocksize: 12, log_blocks_per_seg: 9
        sb[16..20].copy_from_slice(&12u32.to_le_bytes());
        sb[20..24].copy_from_slice(&9u32.to_le_bytes());
        // segs_per_sec: 1, secs_per_zone: 1
        sb[24..28].copy_from_slice(&1u32.to_le_bytes());
        sb[28..32].copy_from_slice(&1u32.to_le_bytes());
        // checksum_offset: SB_CHKSUM_OFFSET (3068)
        sb[32..36].copy_from_slice(&(SB_CHKSUM_OFFSET as u32).to_le_bytes());

        // block_count: le64
        let total_blocks = self.layout.total_segments as u64 * F2FS_BLKS_PER_SEG as u64;
        sb[36..44].copy_from_slice(&total_blocks.to_le_bytes());

        // section_count, segment_count
        let segment_count = (self.layout.main_blkaddr + self.layout.segment_count_main * F2FS_BLKS_PER_SEG - self.layout.segment0_blkaddr) / F2FS_BLKS_PER_SEG;
        sb[44..48].copy_from_slice(&self.layout.segment_count_main.to_le_bytes());
        sb[48..52].copy_from_slice(&segment_count.to_le_bytes());

        // segment_count_ckpt, segment_count_sit, segment_count_nat, segment_count_ssa, segment_count_main
        sb[52..56].copy_from_slice(&self.layout.segment_count_ckpt.to_le_bytes());
        sb[56..60].copy_from_slice(&self.layout.segment_count_sit.to_le_bytes());
        sb[60..64].copy_from_slice(&self.layout.segment_count_nat.to_le_bytes());
        sb[64..68].copy_from_slice(&self.layout.segment_count_ssa.to_le_bytes());
        sb[68..72].copy_from_slice(&self.layout.segment_count_main.to_le_bytes());

        // segment0_blkaddr, cp_blkaddr, sit_blkaddr, nat_blkaddr, ssa_blkaddr, main_blkaddr
        sb[72..76].copy_from_slice(&self.layout.segment0_blkaddr.to_le_bytes());
        sb[76..80].copy_from_slice(&self.layout.cp_blkaddr.to_le_bytes());
        sb[80..84].copy_from_slice(&self.layout.sit_blkaddr.to_le_bytes());
        sb[84..88].copy_from_slice(&self.layout.nat_blkaddr.to_le_bytes());
        sb[88..92].copy_from_slice(&self.layout.ssa_blkaddr.to_le_bytes());
        sb[92..96].copy_from_slice(&self.layout.main_blkaddr.to_le_bytes());

        // root_ino: 3, node_ino: 1, meta_ino: 2
        sb[96..100].copy_from_slice(&F2FS_ROOT_INO.to_le_bytes());
        sb[100..104].copy_from_slice(&F2FS_NODE_INO.to_le_bytes());
        sb[104..108].copy_from_slice(&F2FS_META_INO.to_le_bytes());

        // uuid (16 bytes)
        sb[108..124].copy_from_slice(&self.options.uuid);

        // volume_name: UTF-16LE (512 * 2 = 1024 bytes)
        for (idx, ch) in self.options.label.encode_utf16().take(512).enumerate() {
            let off = 124 + idx * 2;
            sb[off..off + 2].copy_from_slice(&ch.to_le_bytes());
        }

        // feature: F2FS_FEATURE_SB_CHKSUM (at offset 2184)
        sb[2184..2188].copy_from_slice(&F2FS_FEATURE_SB_CHKSUM.to_le_bytes());

        // crc checksum at offset 3068
        let crc = f2fs_crc32(F2FS_SUPER_MAGIC, &sb[..SB_CHKSUM_OFFSET]);
        sb[SB_CHKSUM_OFFSET..SB_CHKSUM_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());

        sb
    }

    /// Serializes the Checkpoint block into a 4096-byte buffer.
    fn serialize_checkpoint_block(&self, cp_ver: u64, valid_user_blks: u64, valid_node_cnt: u32, valid_ino_cnt: u32) -> [u8; F2FS_BLKSIZE] {
        let mut cp = [0u8; F2FS_BLKSIZE];

        // checkpoint_ver: le64
        cp[0..8].copy_from_slice(&cp_ver.to_le_bytes());
        // user_block_count: le64
        cp[8..16].copy_from_slice(&self.layout.user_block_count.to_le_bytes());
        // valid_block_count: le64
        cp[16..24].copy_from_slice(&valid_user_blks.to_le_bytes());

        // rsvd_segment_count, overprov_segment_count, free_segment_count
        let used_segs = self.next_free_main_seg;
        let free_segs = self.layout.segment_count_main.saturating_sub(used_segs);
        cp[24..28].copy_from_slice(&self.layout.rsvd_segments.to_le_bytes());
        cp[28..32].copy_from_slice(&self.layout.overprov_segments.to_le_bytes());
        cp[32..36].copy_from_slice(&free_segs.to_le_bytes());

        // cur_node_segno: 8 * 4 = 32 bytes (offset 36..68)
        for i in 0..3 {
            cp[36 + i * 4..36 + (i + 1) * 4].copy_from_slice(&self.cur_node_seg[i].to_le_bytes());
        }
        for i in 3..8 {
            cp[36 + i * 4..36 + (i + 1) * 4].copy_from_slice(&u32::MAX.to_le_bytes());
        }

        // cur_node_blkoff: 8 * 2 = 16 bytes (offset 68..84)
        for i in 0..3 {
            cp[68 + i * 2..68 + (i + 1) * 2].copy_from_slice(&self.cur_node_off[i].to_le_bytes());
        }

        // cur_data_segno: 8 * 4 = 32 bytes (offset 84..116)
        for i in 0..3 {
            cp[84 + i * 4..84 + (i + 1) * 4].copy_from_slice(&self.cur_data_seg[i].to_le_bytes());
        }
        for i in 3..8 {
            cp[84 + i * 4..84 + (i + 1) * 4].copy_from_slice(&u32::MAX.to_le_bytes());
        }

        // cur_data_blkoff: 8 * 2 = 16 bytes (offset 116..132)
        for i in 0..3 {
            cp[116 + i * 2..116 + (i + 1) * 2].copy_from_slice(&self.cur_data_off[i].to_le_bytes());
        }

        // ckpt_flags: le32 (offset 132..136)
        let flags = CP_UMOUNT_FLAG | CP_COMPACT_SUM_FLAG;
        cp[132..136].copy_from_slice(&flags.to_le_bytes());

        // cp_pack_total_block_count: 6, cp_pack_start_sum: 1
        cp[136..140].copy_from_slice(&6u32.to_le_bytes());
        cp[140..144].copy_from_slice(&1u32.to_le_bytes());

        // valid_node_count, valid_inode_count, next_free_nid
        cp[144..148].copy_from_slice(&valid_node_cnt.to_le_bytes());
        cp[148..152].copy_from_slice(&valid_ino_cnt.to_le_bytes());
        cp[152..156].copy_from_slice(&self.next_free_nid.to_le_bytes());

        // sit_ver_bitmap_bytesize: 64, nat_ver_bitmap_bytesize: 64, checksum_offset: 4092
        cp[156..160].copy_from_slice(&64u32.to_le_bytes());
        cp[160..164].copy_from_slice(&64u32.to_le_bytes());
        cp[164..168].copy_from_slice(&(CP_CHKSUM_OFFSET as u32).to_le_bytes());

        // elapsed_time: le64
        cp[168..176].copy_from_slice(&0u64.to_le_bytes());

        // crc at CP_CHKSUM_OFFSET (4092)
        let crc = f2fs_crc32(F2FS_SUPER_MAGIC, &cp[..CP_CHKSUM_OFFSET]);
        cp[CP_CHKSUM_OFFSET..CP_CHKSUM_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());

        cp
    }

    /// Flushes all metadata, superblocks, checkpoint, SIT, NAT, and SSA tables.
    pub fn finish(&mut self) -> Result<()> {
        if self.open_file.is_some() {
            return Err(Error::Invalid("cannot finish with open file"));
        }
        if self.finished {
            return Ok(());
        }

        // 1. Write dentry blocks for all directories
        let dir_inos: Vec<u32> = self.dir_entries.keys().copied().collect();
        for dir_ino in dir_inos {
            let pblks = self.write_directory_dentry_blocks(dir_ino)?;
            if let Some(inode) = self.inodes.get_mut(&dir_ino) {
                inode.dentry_block_addrs = pblks;
            }
        }

        // 2. Allocate and write inode blocks into HOT_NODE / WARM_NODE segments
        let mut nat_mappings: BTreeMap<u32, u32> = BTreeMap::new();
        let inos: Vec<u32> = self.inodes.keys().copied().collect();
        let cp_ver = 1u64;

        for ino in inos {
            let Some(rec) = self.inodes.get(&ino).cloned() else {
                continue;
            };
            let log_type = if rec.kind == FileKind::Dir {
                CURSEG_HOT_NODE
            } else {
                CURSEG_WARM_NODE
            };

            let pblk = self.alloc_node_block(log_type)?;
            let inode_block = self.serialize_inode_block(&rec, cp_ver, pblk + 1);
            self.storage.write_at(pblk as u64 * F2FS_BLKSIZE as u64, &inode_block)?;

            nat_mappings.insert(ino, pblk);
        }

        // 3. Write NAT table
        // Segment count for NAT is segment_count_nat (e.g. 2, so pack 0 is 1 segment).
        let mut nat_block = vec![0u8; F2FS_BLKSIZE];

        // Reserve node_ino (1) and meta_ino (2) with block_addr = 1 so kernel knows they are not free NIDs
        let node_off = F2FS_NODE_INO as usize * 9;
        nat_block[node_off] = 0;
        nat_block[node_off + 1..node_off + 5].copy_from_slice(&F2FS_NODE_INO.to_le_bytes());
        nat_block[node_off + 5..node_off + 9].copy_from_slice(&1u32.to_le_bytes());

        let meta_off = F2FS_META_INO as usize * 9;
        nat_block[meta_off] = 0;
        nat_block[meta_off + 1..meta_off + 5].copy_from_slice(&F2FS_META_INO.to_le_bytes());
        nat_block[meta_off + 5..meta_off + 9].copy_from_slice(&1u32.to_le_bytes());

        for (&nid, &blkaddr) in &nat_mappings {
            let entry_idx = nid as usize % 455;
            let off = entry_idx * 9;
            nat_block[off] = 0; // version
            nat_block[off + 1..off + 5].copy_from_slice(&nid.to_le_bytes());
            nat_block[off + 5..off + 9].copy_from_slice(&blkaddr.to_le_bytes());
        }
        // Write NAT pack 0
        let nat_pblk0 = self.layout.nat_blkaddr;
        self.storage.write_at(nat_pblk0 as u64 * F2FS_BLKSIZE as u64, &nat_block)?;
        // Write NAT pack 1
        let nat_pblk1 = self.layout.nat_blkaddr + (self.layout.segment_count_nat / 2) * F2FS_BLKS_PER_SEG;
        self.storage.write_at(nat_pblk1 as u64 * F2FS_BLKSIZE as u64, &nat_block)?;

        // 4. Write SIT table
        // Tracks valid blocks per Main Area segment
        let mut sit_block = vec![0u8; F2FS_BLKSIZE];
        for seg in 0..self.next_free_main_seg {
            let mut vblocks = 0u16;
            let mut valid_map = [0u8; 64];

            // Check node segments
            for i in 0..3 {
                if self.cur_node_seg[i] == seg {
                    let cnt = self.cur_node_off[i];
                    vblocks = cnt | ((i as u16 + 3) << 10);
                    for b in 0..cnt as usize {
                        f2fs_set_bit_le(b, &mut valid_map);
                    }
                }
            }
            // Check data segments
            for i in 0..3 {
                if self.cur_data_seg[i] == seg {
                    let cnt = self.cur_data_off[i];
                    vblocks = cnt | ((i as u16) << 10);
                    for b in 0..cnt as usize {
                        f2fs_set_bit_le(b, &mut valid_map);
                    }
                }
            }

            let entry_idx = seg as usize % 55;
            let off = entry_idx * 74;
            sit_block[off..off + 2].copy_from_slice(&vblocks.to_le_bytes());
            sit_block[off + 2..off + 66].copy_from_slice(&valid_map);
            sit_block[off + 66..off + 74].copy_from_slice(&0u64.to_le_bytes()); // mtime
        }
        // Write SIT pack 0
        let sit_pblk0 = self.layout.sit_blkaddr;
        self.storage.write_at(sit_pblk0 as u64 * F2FS_BLKSIZE as u64, &sit_block)?;
        // Write SIT pack 1
        let sit_pblk1 = self.layout.sit_blkaddr + (self.layout.segment_count_sit / 2) * F2FS_BLKS_PER_SEG;
        self.storage.write_at(sit_pblk1 as u64 * F2FS_BLKSIZE as u64, &sit_block)?;

        // 5. Write SSA table
        for seg in 0..self.next_free_main_seg {
            let mut sum_blk = vec![0u8; F2FS_BLKSIZE];
            let is_node = seg < 3;
            let footer_off = 4096 - 5;
            sum_blk[footer_off] = if is_node { 1 } else { 0 }; // SUM_TYPE_NODE / SUM_TYPE_DATA

            let pblk = self.layout.ssa_blkaddr + seg;
            self.storage.write_at(pblk as u64 * F2FS_BLKSIZE as u64, &sum_blk)?;
        }

        // 6. Write Checkpoint Packs
        let total_valid_blks: u64 = (self.cur_node_off.iter().map(|&x| x as u64).sum::<u64>())
            + (self.cur_data_off.iter().map(|&x| x as u64).sum::<u64>());
        let valid_node_cnt = self.cur_node_off.iter().map(|&x| x as u32).sum();
        let valid_ino_cnt = self.inodes.len() as u32;

        let cp_pack1 = self.serialize_checkpoint_block(cp_ver, total_valid_blks, valid_node_cnt, valid_ino_cnt);
        let cp_pblk1 = self.layout.cp_blkaddr;
        // Block 0: Header
        self.storage.write_at(cp_pblk1 as u64 * F2FS_BLKSIZE as u64, &cp_pack1)?;
        // Blocks 1..5: Summaries & footer copy
        for b in 1..5 {
            let mut dummy = vec![0u8; F2FS_BLKSIZE];
            dummy[4096 - 5] = if b >= 2 { 1 } else { 0 };
            self.storage.write_at((cp_pblk1 + b) as u64 * F2FS_BLKSIZE as u64, &dummy)?;
        }
        self.storage.write_at((cp_pblk1 + 5) as u64 * F2FS_BLKSIZE as u64, &cp_pack1)?;

        // CP pack 2 (version 0)
        let cp_pack2 = self.serialize_checkpoint_block(0, total_valid_blks, valid_node_cnt, valid_ino_cnt);
        let cp_pblk2 = self.layout.cp_blkaddr + F2FS_BLKS_PER_SEG;
        self.storage.write_at(cp_pblk2 as u64 * F2FS_BLKSIZE as u64, &cp_pack2)?;
        for b in 1..5 {
            let mut dummy = vec![0u8; F2FS_BLKSIZE];
            dummy[4096 - 5] = if b >= 2 { 1 } else { 0 };
            self.storage.write_at((cp_pblk2 + b) as u64 * F2FS_BLKSIZE as u64, &dummy)?;
        }
        self.storage.write_at((cp_pblk2 + 5) as u64 * F2FS_BLKSIZE as u64, &cp_pack2)?;

        // 7. Write Superblocks (Offset 1024 and 5120)
        let sb = self.serialize_super_block();
        self.storage.write_at(F2FS_SUPER_OFFSET as u64, &sb)?;
        self.storage.write_at((F2FS_SUPER_OFFSET + F2FS_BLKSIZE) as u64, &sb)?;

        self.finished = true;
        Ok(())
    }
}

impl<S: Storage> FsWriter for F2fsWriter<S> {
    fn mkdir(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        self.mkdir(path, meta)
    }

    fn begin_file(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        self.begin_file(path, meta)
    }

    fn write_file(&mut self, data: &[u8]) -> Result<()> {
        self.write_file(data)
    }

    fn end_file(&mut self) -> Result<u64> {
        self.end_file()
    }

    fn symlink(&mut self, path: &str, target: &str, meta: &Meta) -> Result<u64> {
        self.symlink(path, target, meta)
    }

    fn hardlink(&mut self, path: &str, existing: &str) -> Result<u64> {
        self.hardlink(path, existing)
    }

    fn mknod(&mut self, path: &str, kind: FileKind, major: u32, minor: u32, meta: &Meta) -> Result<u64> {
        self.mknod(path, kind, major, minor, meta)
    }

    fn lookup(&self, path: &str) -> Option<u64> {
        self.lookup(path)
    }

    fn file_size(&self, id: u64) -> u64 {
        self.file_size(id)
    }

    fn read_file(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.read_file(id, offset, buf)
    }

    fn finish(&mut self) -> Result<()> {
        self.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    pub struct MemStorage(pub Vec<u8>);

    impl MemStorage {
        pub fn new(size: usize) -> Self {
            Self(vec![0u8; size])
        }
    }

    impl Storage for MemStorage {
        fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
            let off = offset as usize;
            let end = off + data.len();
            if end > self.0.len() {
                self.0.resize(end, 0);
            }
            self.0[off..end].copy_from_slice(data);
            Ok(())
        }

        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
            let off = offset as usize;
            if off >= self.0.len() {
                buf.fill(0);
                return Ok(());
            }
            let avail = (self.0.len() - off).min(buf.len());
            buf[..avail].copy_from_slice(&self.0[off..off + avail]);
            if avail < buf.len() {
                buf[avail..].fill(0);
            }
            Ok(())
        }
    }

    #[test]
    fn test_f2fs_crc32() {
        // Test CRC calculation properties
        let zero = [0u8; 64];
        let c1 = f2fs_crc32(F2FS_SUPER_MAGIC, &zero);
        assert_ne!(c1, 0);

        let data = b"test string for crc32 calculation in f2fs";
        let c2 = f2fs_crc32(F2FS_SUPER_MAGIC, data);
        assert_ne!(c2, c1);

        // Determinism check
        let c3 = f2fs_crc32(F2FS_SUPER_MAGIC, data);
        assert_eq!(c2, c3);
    }

    #[test]
    fn test_tea_and_dentry_hash() {
        // '.' and '..' must always hash to 0 per F2FS spec
        assert_eq!(f2fs_dentry_hash(b"."), 0);
        assert_eq!(f2fs_dentry_hash(b".."), 0);

        // General filenames produce non-zero deterministic hash
        let h_hostname = f2fs_dentry_hash(b"hostname");
        assert_ne!(h_hostname, 0);
        assert_eq!(h_hostname, f2fs_dentry_hash(b"hostname"));

        let h_passwd = f2fs_dentry_hash(b"passwd");
        assert_ne!(h_hostname, h_passwd);

        // Long filename (> 16 bytes) tests multi-round TEA transform
        let long_name = b"this_is_a_very_long_file_name_that_spans_multiple_tea_blocks";
        let h_long = f2fs_dentry_hash(long_name);
        assert_ne!(h_long, 0);
        assert_eq!(h_long, f2fs_dentry_hash(long_name));
    }

    #[test]
    fn test_str2hashbuf() {
        let mut buf = [0u32; 4];
        str2hashbuf(b"abc", &mut buf);
        assert_ne!(buf[0], 0);

        let mut buf16 = [0u32; 4];
        str2hashbuf(b"1234567890123456", &mut buf16);
        for &w in &buf16 {
            assert_ne!(w, 0);
        }
    }

    #[test]
    fn test_encode_decode_dev() {
        let (major, minor) = (8, 1);
        let encoded = encode_dev(major, minor);
        let (dec_maj, dec_min) = decode_dev(encoded);
        assert_eq!(major, dec_maj);
        assert_eq!(minor, dec_min);

        let (major2, minor2) = (254, 4096);
        let encoded2 = encode_dev(major2, minor2);
        let (dec_maj2, dec_min2) = decode_dev(encoded2);
        assert_eq!(major2, dec_maj2);
        assert_eq!(minor2, dec_min2);
    }

    #[test]
    fn test_components_parsing() {
        assert_eq!(components("etc/hostname").unwrap(), vec!["etc", "hostname"]);
        assert_eq!(components("/etc//hostname/").unwrap(), vec!["etc", "hostname"]);
        assert_eq!(components("file").unwrap(), vec!["file"]);
        components("etc/../hostname").unwrap_err();
        let long_str = "a".repeat(256);
        components(&long_str).unwrap_err();
    }

    #[test]
    fn test_compute_layout_too_small() {
        // Less than 28 MiB (14 segments) should fail
        let res = compute_layout(10 * 1024 * 1024);
        assert!(matches!(res, Err(Error::Size(_))));
    }

    #[test]
    fn test_compute_layout_valid() {
        let res = compute_layout(32 * 1024 * 1024).unwrap();
        assert_eq!(res.total_segments, 16);
        assert_eq!(res.segment0_blkaddr, 512);
        assert_eq!(res.cp_blkaddr, 512);
        assert_eq!(res.segment_count_ckpt, 2);
        assert_eq!(res.sit_blkaddr, 1536);
        assert_eq!(res.segment_count_sit, 2);
        assert_eq!(res.nat_blkaddr, 2560);
        assert_eq!(res.segment_count_nat, 2);
        assert_eq!(res.ssa_blkaddr, 3584);
        assert_eq!(res.segment_count_ssa, 1);
        assert_eq!(res.main_blkaddr, 4096);
        assert_eq!(res.segment_count_main, 8);
    }

    #[test]
    fn test_empty_filesystem_and_superblocks() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let opts = Options {
            label: String::from("ARCH_F2FS"),
            uuid: [0x12; 16],
            now: 1700000000,
        };
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, opts).unwrap();
        fs.finish().unwrap();

        // 1. Verify Superblock 0 (Offset 1024)
        let mut sb0 = [0u8; 3072];
        fs.storage.read_at(1024, &mut sb0).unwrap();

        let magic = u32::from_le_bytes(sb0[0..4].try_into().unwrap());
        assert_eq!(magic, F2FS_SUPER_MAGIC);
        assert_eq!(&sb0[0..4], &[0x10, 0x20, 0xF5, 0xF2]); // LE bytes

        let major = u16::from_le_bytes(sb0[4..6].try_into().unwrap());
        let minor = u16::from_le_bytes(sb0[6..8].try_into().unwrap());
        assert_eq!(major, 1);
        assert_eq!(minor, 14);

        let root_ino = u32::from_le_bytes(sb0[96..100].try_into().unwrap());
        assert_eq!(root_ino, 3);

        let crc_sb0 = u32::from_le_bytes(sb0[SB_CHKSUM_OFFSET..SB_CHKSUM_OFFSET + 4].try_into().unwrap());
        let expected_crc0 = f2fs_crc32(F2FS_SUPER_MAGIC, &sb0[..SB_CHKSUM_OFFSET]);
        assert_eq!(crc_sb0, expected_crc0);

        // 2. Verify Superblock 1 (Offset 1024 + 4096 = 5120)
        let mut sb1 = [0u8; 3072];
        fs.storage.read_at(1024 + 4096, &mut sb1).unwrap();
        assert_eq!(sb0, sb1);

        // 3. Verify Checkpoint (Segment 1 = block 512 = 2 MiB)
        let mut cp = [0u8; 4096];
        fs.storage.read_at(512 * 4096, &mut cp).unwrap();
        let cp_ver = u64::from_le_bytes(cp[0..8].try_into().unwrap());
        assert_eq!(cp_ver, 1);

        let cp_crc = u32::from_le_bytes(cp[CP_CHKSUM_OFFSET..CP_CHKSUM_OFFSET + 4].try_into().unwrap());
        let expected_cp_crc = f2fs_crc32(F2FS_SUPER_MAGIC, &cp[..CP_CHKSUM_OFFSET]);
        assert_eq!(cp_crc, expected_cp_crc);

        // Check active logs are 6 distinct segments
        let mut active_segs = Vec::new();
        for i in 0..3 {
            let n_seg = u32::from_le_bytes(cp[36 + i * 4..36 + (i + 1) * 4].try_into().unwrap());
            assert!(!active_segs.contains(&n_seg));
            active_segs.push(n_seg);
        }
        for i in 0..3 {
            let d_seg = u32::from_le_bytes(cp[84 + i * 4..84 + (i + 1) * 4].try_into().unwrap());
            assert!(!active_segs.contains(&d_seg));
            active_segs.push(d_seg);
        }
        assert_eq!(active_segs.len(), 6);
    }

    #[test]
    fn test_mkdir_and_lookup() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta {
            mode: 0o755,
            uid: 1000,
            gid: 1000,
            mtime: 12345,
            xattrs: Vec::new(),
        };

        let etc_id = fs.mkdir("etc", &meta).unwrap();
        assert_eq!(etc_id, 4);
        assert_eq!(fs.lookup("etc"), Some(etc_id));
        assert_eq!(fs.lookup("/etc"), Some(etc_id));

        // Duplicate mkdir -> updates and returns existing
        assert_eq!(fs.mkdir("etc", &meta), Ok(etc_id));

        // Nested directory
        let sub_id = fs.mkdir("etc/network", &meta).unwrap();
        assert_eq!(sub_id, 5);
        assert_eq!(fs.lookup("etc/network"), Some(sub_id));

        // Missing parent -> auto-creates parent and succeeds
        let log_id = fs.mkdir("var/log", &meta).unwrap();
        assert!(fs.lookup("var").is_some());
        assert_eq!(fs.lookup("var/log"), Some(log_id));
    }

    #[test]
    fn test_file_write_and_read_back() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta {
            mode: 0o644,
            uid: 0,
            gid: 0,
            mtime: 12345,
            xattrs: Vec::new(),
        };

        fs.begin_file("test.txt", &meta).unwrap();
        fs.write_file(b"Hello, ").unwrap();
        fs.write_file(b"F2FS!").unwrap();
        let file_id = fs.end_file().unwrap();

        assert_eq!(fs.lookup("test.txt"), Some(file_id));
        assert_eq!(fs.file_size(file_id), 12);

        // Read back
        let mut buf = [0u8; 12];
        let n = fs.read_file(file_id, 0, &mut buf).unwrap();
        assert_eq!(n, 12);
        assert_eq!(&buf, b"Hello, F2FS!");

        // Partial read
        let mut part = [0u8; 4];
        let n_part = fs.read_file(file_id, 7, &mut part).unwrap();
        assert_eq!(n_part, 4);
        assert_eq!(&part, b"F2FS");

        // EOF read
        let mut eof = [0u8; 5];
        let n_eof = fs.read_file(file_id, 12, &mut eof).unwrap();
        assert_eq!(n_eof, 0);

        // Past EOF
        let n_past = fs.read_file(file_id, 100, &mut eof).unwrap();
        assert_eq!(n_past, 0);

        // Non-existent id
        assert_eq!(fs.read_file(999, 0, &mut buf), Err(Error::NotFound));
    }

    #[test]
    fn test_file_error_states() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        // Write without begin_file -> Invalid
        fs.write_file(b"data").unwrap_err();
        // End without begin_file -> Invalid
        fs.end_file().unwrap_err();

        fs.begin_file("a.txt", &meta).unwrap();
        // Begin while another is open -> Invalid
        fs.begin_file("b.txt", &meta).unwrap_err();
        // Mkdir while file is open -> Invalid
        fs.mkdir("dir", &meta).unwrap_err();
        fs.end_file().unwrap();
    }

    #[test]
    fn test_symlink_and_hardlink() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        // Symlink
        let sym_id = fs.symlink("link", "target/path", &meta).unwrap();
        assert_eq!(fs.lookup("link"), Some(sym_id));
        assert_eq!(fs.file_size(sym_id), 11);
        let mut buf = [0u8; 11];
        fs.read_file(sym_id, 0, &mut buf).unwrap();
        assert_eq!(&buf, b"target/path");

        // Regular file + Hardlink
        fs.begin_file("real.txt", &meta).unwrap();
        fs.write_file(b"content").unwrap();
        let real_id = fs.end_file().unwrap();

        let hard_id = fs.hardlink("hard.txt", "real.txt").unwrap();
        assert_eq!(hard_id, real_id);
        assert_eq!(fs.lookup("hard.txt"), Some(real_id));

        // Hardlink to nonexistent
        assert_eq!(fs.hardlink("bad.txt", "none.txt"), Err(Error::NotFound));
    }

    #[test]
    fn test_mknod_devices() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        let chr_id = fs.mknod("dev_null", FileKind::CharDev, 1, 3, &meta).unwrap();
        assert_eq!(fs.lookup("dev_null"), Some(chr_id));
        let rec = &fs.inodes[&(chr_id as u32)];
        assert_eq!(rec.rdev, (1, 3));
        assert_eq!(rec.mode & S_IFCHR, S_IFCHR);

        let blk_id = fs.mknod("dev_sda", FileKind::BlockDev, 8, 0, &meta).unwrap();
        assert_eq!(fs.lookup("dev_sda"), Some(blk_id));
        let rec_blk = &fs.inodes[&(blk_id as u32)];
        assert_eq!(rec_blk.rdev, (8, 0));
        assert_eq!(rec_blk.mode & S_IFBLK, S_IFBLK);
    }

    #[test]
    fn test_multi_block_file() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        fs.begin_file("big.bin", &meta).unwrap();
        let data = vec![0xABu8; 10000]; // Spans 3 blocks: 4096 + 4096 + 1808
        fs.write_file(&data).unwrap();
        let id = fs.end_file().unwrap();

        assert_eq!(fs.file_size(id), 10000);
        let mut read_back = vec![0u8; 10000];
        let n = fs.read_file(id, 0, &mut read_back).unwrap();
        assert_eq!(n, 10000);
        assert_eq!(read_back, data);
    }

    #[test]
    fn test_fswriter_dyn_trait() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let w: &mut dyn FsWriter = &mut fs;
        let meta = Meta::default();

        let dir = w.mkdir("usr", &meta).unwrap();
        assert_eq!(dir, 4);

        w.begin_file("usr/bin", &meta).unwrap();
        w.write_file(b"binary").unwrap();
        let f = w.end_file().unwrap();
        assert_eq!(f, 5);

        assert_eq!(w.file_size(f), 6);
        w.finish().unwrap();
    }

    #[test]
    fn test_internal_alloc_blocks() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();

        // Data block allocation in WARM_DATA (segment 4)
        let b1 = fs.alloc_data_block(CURSEG_WARM_DATA).unwrap();
        assert_eq!(b1, fs.layout.main_blkaddr + 4 * F2FS_BLKS_PER_SEG);
        let b2 = fs.alloc_data_block(CURSEG_WARM_DATA).unwrap();
        assert_eq!(b2, fs.layout.main_blkaddr + 4 * F2FS_BLKS_PER_SEG + 1);

        // Node block allocation in WARM_NODE (segment 1)
        let n1 = fs.alloc_node_block(CURSEG_WARM_NODE).unwrap();
        assert_eq!(n1, fs.layout.main_blkaddr + F2FS_BLKS_PER_SEG);
        let n2 = fs.alloc_node_block(CURSEG_WARM_NODE).unwrap();
        assert_eq!(n2, fs.layout.main_blkaddr + F2FS_BLKS_PER_SEG + 1);
    }

    #[test]
    fn test_internal_check_new_entry_and_add_entry() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();

        // Valid new entry in root
        let (pino, name) = fs.check_new_entry("new_dir").unwrap();
        assert_eq!(pino, F2FS_ROOT_INO);
        assert_eq!(name, "new_dir");

        // Existing entry in root
        let meta = Meta::default();
        fs.mkdir("new_dir", &meta).unwrap();
        let err = fs.check_new_entry("new_dir").unwrap_err();
        assert_eq!(err, Error::Exists);

        // Missing parent -> auto-creates parent and returns child entry info
        let (pino_parent, child_name) = fs.check_new_entry("nonexistent/child").unwrap();
        assert_eq!(child_name, "child");
        assert_eq!(fs.lookup("nonexistent"), Some(pino_parent as u64));

        // Adding entry updates parent directory size
        fs.add_entry_to_parent(F2FS_ROOT_INO, 10, "file1", F2FS_FT_REG_FILE).unwrap();
        let root_size = fs.inodes[&F2FS_ROOT_INO].size;
        assert_eq!(root_size, F2FS_BLKSIZE as u64);
    }

    #[test]
    fn test_serialize_inode_block_layout() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();

        let rec = InodeRecord {
            ino: 42,
            pino: 3,
            name: String::from("file42"),
            kind: FileKind::Regular,
            mode: S_IFREG | 0o644,
            uid: 1000,
            gid: 1001,
            size: 8192,
            mtime: 1700000000,
            links: 3,
            rdev: (0, 0),
            direct_blocks: vec![4096, 4097],
            indirect_blocks: Vec::new(),
            dentry_block_addrs: Vec::new(),
        };

        let blk = fs.serialize_inode_block(&rec, 1, 4098);
        assert_eq!(u16::from_le_bytes(blk[0..2].try_into().unwrap()), S_IFREG | 0o644);
        assert_eq!(u32::from_le_bytes(blk[4..8].try_into().unwrap()), 1000);
        assert_eq!(u32::from_le_bytes(blk[8..12].try_into().unwrap()), 1001);
        assert_eq!(u32::from_le_bytes(blk[12..16].try_into().unwrap()), 3);
        assert_eq!(u64::from_le_bytes(blk[16..24].try_into().unwrap()), 8192);
        assert_eq!(u64::from_le_bytes(blk[24..32].try_into().unwrap()), 3); // 1 inode blk + 2 data blks

        // pino and name (offset 84 and 88)
        assert_eq!(u32::from_le_bytes(blk[84..88].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(blk[88..92].try_into().unwrap()), 6);
        assert_eq!(&blk[92..98], b"file42");

        // Block pointers in i_addr (offset 360)
        assert_eq!(u32::from_le_bytes(blk[360..364].try_into().unwrap()), 4096);
        assert_eq!(u32::from_le_bytes(blk[364..368].try_into().unwrap()), 4097);

        // Footer at 4072..4096
        assert_eq!(u32::from_le_bytes(blk[4072..4076].try_into().unwrap()), 42); // nid
        assert_eq!(u32::from_le_bytes(blk[4076..4080].try_into().unwrap()), 42); // ino
        assert_eq!(u64::from_le_bytes(blk[4084..4092].try_into().unwrap()), 1); // cp_ver
        assert_eq!(u32::from_le_bytes(blk[4092..4096].try_into().unwrap()), 4098); // next_blkaddr
    }

    #[test]
    fn test_nat_sit_ssa_on_disk_tables() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        let _dir = fs.mkdir("home", &meta).unwrap();
        fs.begin_file("home/test.txt", &meta).unwrap();
        fs.write_file(b"content").unwrap();
        let _file = fs.end_file().unwrap();

        fs.finish().unwrap();

        // Verify NAT on disk:
        // Root inode (nid 3), dir (nid 4), file (nid 5)
        let nat_offset = fs.layout.nat_blkaddr as u64 * F2FS_BLKSIZE as u64;
        let mut nat_bytes = [0u8; 4096];
        fs.storage.read_at(nat_offset, &mut nat_bytes).unwrap();

        // Entry 3 (root)
        let root_pblk = u32::from_le_bytes(nat_bytes[3 * 9 + 5..3 * 9 + 9].try_into().unwrap());
        assert_ne!(root_pblk, 0);

        // Entry 4 (dir)
        let dir_pblk = u32::from_le_bytes(nat_bytes[4 * 9 + 5..4 * 9 + 9].try_into().unwrap());
        assert_ne!(dir_pblk, 0);

        // Entry 5 (file)
        let file_pblk = u32::from_le_bytes(nat_bytes[5 * 9 + 5..5 * 9 + 9].try_into().unwrap());
        assert_ne!(file_pblk, 0);

        // Verify SIT on disk:
        let sit_offset = fs.layout.sit_blkaddr as u64 * F2FS_BLKSIZE as u64;
        let mut sit_bytes = [0u8; 4096];
        fs.storage.read_at(sit_offset, &mut sit_bytes).unwrap();

        // Check segment 0 (HOT_NODE) has valid blocks
        let seg0_vblocks = u16::from_le_bytes(sit_bytes[0..2].try_into().unwrap());
        let seg0_cnt = seg0_vblocks & 0x3FF;
        assert!(seg0_cnt > 0);
        // Bit 0 of valid_map must be set
        assert_ne!(sit_bytes[2], 0);

        // Verify SSA on disk:
        let ssa_offset = fs.layout.ssa_blkaddr as u64 * F2FS_BLKSIZE as u64;
        let mut ssa_bytes = [0u8; 4096];
        fs.storage.read_at(ssa_offset, &mut ssa_bytes).unwrap();
        // Footer entry_type at 4096 - 5
        assert_eq!(ssa_bytes[4096 - 5], 1); // Segment 0 is node segment (SUM_TYPE_NODE = 1)
    }

    #[test]
    fn test_finish_edge_cases() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        fs.begin_file("open.txt", &meta).unwrap();
        // Finish with open file fails
        assert_eq!(fs.finish(), Err(Error::Invalid("cannot finish with open file")));

        fs.end_file().unwrap();
        // First finish succeeds
        fs.finish().unwrap();
        // Second finish is idempotent and succeeds
        fs.finish().unwrap();
    }

    #[test]
    fn test_long_filename_handling() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        // 120-character filename (spans 15 slots of 8 bytes each)
        let long_name = "this_is_a_very_long_file_name_that_should_span_multiple_slots_in_the_f2fs_dentry_block_structure_without_problems.txt";
        assert_eq!(long_name.len(), 117);

        fs.begin_file(long_name, &meta).unwrap();
        fs.write_file(b"data").unwrap();
        let id = fs.end_file().unwrap();

        assert_eq!(fs.lookup(long_name), Some(id));
        assert_eq!(fs.file_size(id), 4);

        fs.finish().unwrap();

        // Verify lookup still works
        assert_eq!(fs.lookup(long_name), Some(id));
    }

    #[test]
    fn test_mkdir_auto_parents_and_idempotence() {
        let mem = MemStorage::new(32 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 32 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta::default();

        // 1. mkdir with deeply nested parent directories that do not yet exist
        let c_ino = fs.mkdir("usr/lib/systemd", &meta).unwrap();
        assert!(fs.lookup("usr").is_some());
        assert!(fs.lookup("usr/lib").is_some());
        assert_eq!(fs.lookup("usr/lib/systemd"), Some(c_ino));

        // 2. mkdir on existing directory updates it and returns same ino
        let c_ino2 = fs.mkdir("usr/lib/systemd", &Meta { mode: 0o700, ..Default::default() }).unwrap();
        assert_eq!(c_ino, c_ino2);

        // 3. mkdir on root ("" or "/")
        let root_ino = fs.mkdir("", &Meta { mode: 0o755, ..Default::default() }).unwrap();
        assert_eq!(root_ino, F2FS_ROOT_INO as u64);

        // 4. begin_file with nested non-existent parents
        fs.begin_file("etc/systemd/system/test.service", &meta).unwrap();
        fs.write_file(b"[Unit]\nDescription=Test\n").unwrap();
        let s_ino = fs.end_file().unwrap();
        assert!(fs.lookup("etc").is_some());
        assert!(fs.lookup("etc/systemd").is_some());
        assert!(fs.lookup("etc/systemd/system").is_some());
        assert_eq!(fs.lookup("etc/systemd/system/test.service"), Some(s_ino));

        // 5. mkdir on existing file fails with Exists
        assert_eq!(
            fs.mkdir("etc/systemd/system/test.service", &meta),
            Err(Error::Exists)
        );

        fs.finish().unwrap();
    }

    #[test]
    fn test_debug_linux_test_layout() {
        let mem = MemStorage::new(64 * 1024 * 1024);
        let mut fs = F2fsWriter::new(mem, 64 * 1024 * 1024, Options::default()).unwrap();
        let meta = Meta { mode: 0o755, ..Default::default() };
        for d in ["dev", "proc", "sys", "run", "etc", "usr/lib/systemd"] {
            fs.mkdir(d, &meta).unwrap();
        }
        fs.begin_file("usr/lib/systemd/systemd", &meta).unwrap();
        fs.write_file(&vec![0u8; 1000]).unwrap();
        fs.end_file().unwrap();
        fs.begin_file("etc/archstaller-marker", &meta).unwrap();
        fs.write_file(b"test").unwrap();
        fs.end_file().unwrap();
        fs.finish().unwrap();
        extern crate std;
        std::println!("cur_data_seg: {:?}", fs.cur_data_seg);
        std::println!("cur_data_off: {:?}", fs.cur_data_off);
        std::println!("cur_node_seg: {:?}", fs.cur_node_seg);
        std::println!("cur_node_off: {:?}", fs.cur_node_off);

        // Read SIT pack 0
        let sit_offset = fs.layout.sit_blkaddr as u64 * F2FS_BLKSIZE as u64;
        let mut sit_bytes = [0u8; 4096];
        fs.storage.read_at(sit_offset, &mut sit_bytes).unwrap();
        for seg in 0..6 {
            let off = seg * 74;
            let vblocks = u16::from_le_bytes(sit_bytes[off..off + 2].try_into().unwrap());
            let valid_cnt = vblocks & 0x3ff;
            let seg_type = vblocks >> 10;
            let map = &sit_bytes[off + 2..off + 10];
            std::println!("seg {seg}: type={seg_type}, valid_cnt={valid_cnt}, map[0..8]={:02x?}", map);
        }
    }

    #[test]
    fn test_f2fs_bitmap_bitops() {
        let mut map = [0u8; 64];

        // Bit 0 is MSB of byte 0 (0x80 = 128)
        assert!(!f2fs_test_bit(0, &map));
        f2fs_set_bit(0, &mut map);
        assert!(f2fs_test_bit(0, &map));
        assert_eq!(map[0], 0x80);

        // Bit 7 is LSB of byte 0 (0x01 = 1)
        assert!(!f2fs_test_bit(7, &map));
        f2fs_set_bit(7, &mut map);
        assert!(f2fs_test_bit(7, &map));
        assert_eq!(map[0], 0x81);

        // Bit 8 is MSB of byte 1 (0x80 = 128)
        assert!(!f2fs_test_bit(8, &map));
        f2fs_set_bit(8, &mut map);
        assert!(f2fs_test_bit(8, &map));
        assert_eq!(map[1], 0x80);

        // Bit 15 is LSB of byte 1 (0x01 = 1)
        assert!(!f2fs_test_bit(15, &map));
        f2fs_set_bit(15, &mut map);
        assert!(f2fs_test_bit(15, &map));
        assert_eq!(map[1], 0x81);

        // Bit 511 is LSB of byte 63
        assert!(!f2fs_test_bit(511, &map));
        f2fs_set_bit(511, &mut map);
        assert!(f2fs_test_bit(511, &map));
        assert_eq!(map[63], 0x01);
    }

    #[test]
    fn test_f2fs_bitmap_le_bitops() {
        let mut map = [0u8; 64];

        // Bit 0 is LSB of byte 0 (0x01 = 1) in LE bit order
        assert!(!f2fs_test_bit_le(0, &map));
        f2fs_set_bit_le(0, &mut map);
        assert!(f2fs_test_bit_le(0, &map));
        assert_eq!(map[0], 0x01);

        // Bit 7 is MSB of byte 0 (0x80 = 128) in LE bit order
        assert!(!f2fs_test_bit_le(7, &map));
        f2fs_set_bit_le(7, &mut map);
        assert!(f2fs_test_bit_le(7, &map));
        assert_eq!(map[0], 0x81);

        // Bit 8 is LSB of byte 1 (0x01 = 1) in LE bit order
        assert!(!f2fs_test_bit_le(8, &map));
        f2fs_set_bit_le(8, &mut map);
        assert!(f2fs_test_bit_le(8, &map));
        assert_eq!(map[1], 0x01);
    }

    #[test]
    fn test_f2fs_nat_special_inos_and_sb_segment_count() {
        let size = 64 * 1024 * 1024;
        let storage = MemStorage::new(size as usize);
        let mut writer = F2fsWriter::new(
            storage,
            size,
            Options {
                label: "test".into(),
                uuid: [1u8; 16],
                now: 123456,
            },
        )
        .expect("create writer");

        writer.mkdir("dir1", &Meta::default()).unwrap();
        writer.begin_file("dir1/file1.txt", &Meta::default()).unwrap();
        writer.write_file(b"content").unwrap();
        writer.end_file().unwrap();
        writer.finish().unwrap();

        // 1. Verify Superblock segment_count
        let mut sb = [0u8; 3072];
        writer.storage.read_at(F2FS_SUPER_OFFSET as u64, &mut sb).unwrap();
        let segment_count = u32::from_le_bytes(sb[48..52].try_into().unwrap());
        let total_segments = (size / F2FS_BLKSIZE as u64 / F2FS_BLKS_PER_SEG as u64) as u32;
        // segment_count must match segments from segment0_blkaddr to end
        assert_eq!(segment_count, total_segments - 1);

        // 2. Verify NAT table has entries for F2FS_NODE_INO (1) and F2FS_META_INO (2)
        let nat_pblk = writer.layout.nat_blkaddr;
        let mut nat_blk = [0u8; F2FS_BLKSIZE];
        writer.storage.read_at(nat_pblk as u64 * F2FS_BLKSIZE as u64, &mut nat_blk).unwrap();

        // Entry 1 (node_ino)
        let e1_ino = u32::from_le_bytes(nat_blk[9 + 1..9 + 5].try_into().unwrap());
        let e1_blk = u32::from_le_bytes(nat_blk[9 + 5..9 + 9].try_into().unwrap());
        assert_eq!(e1_ino, F2FS_NODE_INO);
        assert_eq!(e1_blk, 1);

        // Entry 2 (meta_ino)
        let e2_ino = u32::from_le_bytes(nat_blk[18 + 1..18 + 5].try_into().unwrap());
        let e2_blk = u32::from_le_bytes(nat_blk[18 + 5..18 + 9].try_into().unwrap());
        assert_eq!(e2_ino, F2FS_META_INO);
        assert_eq!(e2_blk, 1);

        // 3. Verify root dentry bitmap in first dentry block has bit 0 set to 0x01 (LE bit 0)
        let root_rec = &writer.inodes[&F2FS_ROOT_INO];
        let root_dentry_pblk = root_rec.dentry_block_addrs[0];
        let mut dentry_blk = [0u8; F2FS_BLKSIZE];
        writer.storage.read_at(root_dentry_pblk as u64 * F2FS_BLKSIZE as u64, &mut dentry_blk).unwrap();
        // Byte 0 has '.' at slot 0 (bit 0) and '..' at slot 1 (bit 1) and 'dir1' at slot 2 (bit 2)
        // In LE bit order, bits 0, 1, 2 set = 1 | 2 | 4 = 7
        assert_eq!(dentry_blk[0] & 0x07, 0x07);

        // 4. Verify SIT table valid_map uses LE bit order (bit 0 = 0x01)
        let sit_pblk = writer.layout.sit_blkaddr;
        let mut sit_blk = [0u8; F2FS_BLKSIZE];
        writer.storage.read_at(sit_pblk as u64 * F2FS_BLKSIZE as u64, &mut sit_blk).unwrap();
        assert_eq!(sit_blk[2] & 0x01, 0x01, "SIT valid_map bit 0 must be in LE bit position (0x01)");
    }
}



