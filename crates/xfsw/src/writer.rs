//! Write-once XFS V5 filesystem generator.
//!
//! Conforms to `disk::FsWriter` for bare-metal streaming installation.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use disk::{Error, FileKind, FsWriter, Meta, Result, Storage};

use crate::crc::xfs_write_cksum;

pub const BLOCK_SIZE: u64 = 4096;
pub const SECTOR_SIZE: u64 = 512;
pub const INODE_SIZE: u64 = 512;
pub const INOPBLOCK: u64 = BLOCK_SIZE / INODE_SIZE; // 8
pub const ROOT_INO: u64 = 128; // Block 16, inode index 0

pub const XFS_SB_MAGIC: u32 = 0x5846_5342; // "XFSB"
pub const XFS_AGF_MAGIC: u32 = 0x5841_4746; // "XAGF"
pub const XFS_AGI_MAGIC: u32 = 0x5841_4749; // "XAGI"
pub const XFS_AGFL_MAGIC: u32 = 0x5841_464C; // "XAFL"
pub const XFS_DINODE_MAGIC: u16 = 0x494E; // "IN"
pub const XFS_DIR3_BLOCK_MAGIC: u32 = 0x5844_4233; // "XDB3"
pub const XFS_AB3B_MAGIC: u32 = 0x4142_3342; // "AB3B"
pub const XFS_AB3C_MAGIC: u32 = 0x4142_3343; // "AB3C"
pub const XFS_IAB3_MAGIC: u32 = 0x4941_4233; // "IAB3"

pub const XFS_DINODE_FMT_DEV: u8 = 0;
pub const XFS_DINODE_FMT_LOCAL: u8 = 1;
pub const XFS_DINODE_FMT_EXTENTS: u8 = 2;

pub const XFS_DIR3_FT_UNKNOWN: u8 = 0;
pub const XFS_DIR3_FT_REG_FILE: u8 = 1;
pub const XFS_DIR3_FT_DIR: u8 = 2;
pub const XFS_DIR3_FT_CHRDEV: u8 = 3;
pub const XFS_DIR3_FT_BLKDEV: u8 = 4;
pub const XFS_DIR3_FT_FIFO: u8 = 5;
pub const XFS_DIR3_FT_SOCK: u8 = 6;
pub const XFS_DIR3_FT_SYMLINK: u8 = 7;

pub const S_IFREG: u16 = 0o100000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IFLNK: u16 = 0o120000;
pub const S_IFCHR: u16 = 0o020000;
pub const S_IFBLK: u16 = 0o060000;
pub const S_IFIFO: u16 = 0o010000;

pub const SB_CRC_OFF: usize = 224;
pub const AGF_CRC_OFF: usize = 216;
pub const AGI_CRC_OFF: usize = 312;
pub const AGFL_CRC_OFF: usize = 32;
pub const DINODE_CRC_OFF: usize = 100;
pub const DIR3_DATA_CRC_OFF: usize = 4;
pub const BTREE_SHDR_CRC_OFF: usize = 52;

pub const INODE_CORE_SIZE: usize = 176;
pub const INODE_FORK_SIZE: usize = 512 - INODE_CORE_SIZE; // 336 bytes

#[derive(Clone, Debug)]
pub struct Options {
    pub label: String,
    pub uuid: [u8; 16],
    pub now: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            label: String::from("archroot"),
            uuid: [0u8; 16],
            now: 0,
        }
    }
}

fn put_slice(buf: &mut [u8], off: usize, data: &[u8]) {
    if let Some(slot) = buf.get_mut(off..off + data.len()) {
        slot.copy_from_slice(data);
    }
}

fn put_u8(buf: &mut [u8], off: usize, val: u8) {
    if let Some(slot) = buf.get_mut(off) {
        *slot = val;
    }
}

fn put_be16(buf: &mut [u8], off: usize, val: u16) {
    put_slice(buf, off, &val.to_be_bytes());
}

fn put_be32(buf: &mut [u8], off: usize, val: u32) {
    put_slice(buf, off, &val.to_be_bytes());
}

fn put_be64(buf: &mut [u8], off: usize, val: u64) {
    put_slice(buf, off, &val.to_be_bytes());
}

/// Calculate XFS directory entry name hash.
pub fn xfs_da_hashname(name: &[u8]) -> u32 {
    let mut hash = 0u32;
    let mut ptr = name;

    while let Some((&[b0, b1, b2, b3], rest)) = ptr.split_first_chunk::<4>() {
        hash = ((b0 as u32) << 21)
            ^ ((b1 as u32) << 14)
            ^ ((b2 as u32) << 7)
            ^ (b3 as u32)
            ^ hash.rotate_left(28);
        ptr = rest;
    }

    match *ptr {
        [b0, b1, b2] => {
            ((b0 as u32) << 14) ^ ((b1 as u32) << 7) ^ (b2 as u32) ^ hash.rotate_left(21)
        }
        [b0, b1] => ((b0 as u32) << 7) ^ (b1 as u32) ^ hash.rotate_left(14),
        [b0] => (b0 as u32) ^ hash.rotate_left(7),
        _ => hash,
    }
}

/// Pack an extent into 16-byte `xfs_bmbt_rec`.
pub fn pack_bmbt_rec(startoff: u64, startblock: u64, blockcount: u32, unwritten: bool) -> [u8; 16] {
    let mut rec = [0u8; 16];
    let extent_flag = if unwritten { 1u64 } else { 0u64 };
    let l0 = (extent_flag << 63)
        | ((startoff & 0x003F_FFFF_FFFF_FFFF) << 9)
        | ((startblock >> 43) & 0x1FF);
    let l1 = (startblock << 21) | ((blockcount as u64) & 0x1F_FFFF);
    put_be64(&mut rec, 0, l0);
    put_be64(&mut rec, 8, l1);
    rec
}

/// Unpack a 16-byte `xfs_bmbt_rec`.
pub fn unpack_bmbt_rec(rec: &[u8; 16]) -> (u64, u64, u32, bool) {
    let l0 = rec.get(..8).map_or(0, |s| {
        let mut b = [0u8; 8];
        b.copy_from_slice(s);
        u64::from_be_bytes(b)
    });
    let l1 = rec.get(8..16).map_or(0, |s| {
        let mut b = [0u8; 8];
        b.copy_from_slice(s);
        u64::from_be_bytes(b)
    });
    let unwritten = (l0 >> 63) != 0;
    let startoff = (l0 & 0x7FFF_FFFF_FFFF_FFFF) >> 9;
    let startblock = ((l0 & 0x1FF) << 43) | (l1 >> 21);
    let blockcount = (l1 & 0x1F_FFFF) as u32;
    (startoff, startblock, blockcount, unwritten)
}

/// Format the XFS V5 superblock into a 512-byte sector.
#[expect(
    clippy::too_many_arguments,
    reason = "XFS superblock on-disk formatter requires all layout geometry and accounting fields"
)]
pub fn format_superblock(
    total_blocks: u64,
    rootino: u64,
    free_blocks: u64,
    allocated_inodes: u64,
    free_inodes: u64,
    logstart: u64,
    logblocks: u32,
    opts: &Options,
) -> [u8; 512] {
    let mut sb = [0u8; 512];
    put_be32(&mut sb, 0x00, XFS_SB_MAGIC);
    put_be32(&mut sb, 0x04, BLOCK_SIZE as u32);
    put_be64(&mut sb, 0x08, total_blocks);
    put_slice(&mut sb, 0x20, &opts.uuid);
    put_be64(&mut sb, 0x30, logstart);
    put_be64(&mut sb, 0x38, rootino);
    put_be64(&mut sb, 0x40, rootino + 1);         // rbmino
    put_be64(&mut sb, 0x48, rootino + 2);         // rsumino
    put_be32(&mut sb, 0x50, 1);                   // rextsize = 1 block
    put_be32(&mut sb, 0x54, total_blocks as u32); // agblocks
    put_be32(&mut sb, 0x58, 1);                   // agcount = 1
    put_be32(&mut sb, 0x60, logblocks);           // logblocks
    put_be16(&mut sb, 0x64, 0xB4A5);              // version 5
    put_be16(&mut sb, 0x66, SECTOR_SIZE as u16);
    put_be16(&mut sb, 0x68, INODE_SIZE as u16);
    put_be16(&mut sb, 0x6A, INOPBLOCK as u16);

    let label = opts.label.as_bytes();
    let n = label.len().min(12);
    if let Some(src) = label.get(..n) {
        put_slice(&mut sb, 0x6C, src);
    }

    put_u8(&mut sb, 0x78, 12); // log2(4096)
    put_u8(&mut sb, 0x79, 9);  // log2(512)
    put_u8(&mut sb, 0x7A, 9);  // log2(512)
    put_u8(&mut sb, 0x7B, 3);  // log2(8)
    let agblklog = (64 - total_blocks.saturating_sub(1).leading_zeros()) as u8;
    put_u8(&mut sb, 0x7C, agblklog);

    put_be64(&mut sb, 0x80, allocated_inodes); // icount
    put_be64(&mut sb, 0x88, free_inodes);      // ifree
    put_be64(&mut sb, 0x90, free_blocks);      // fdblocks

    put_be32(&mut sb, 0xC8, 0x0000_038A); // features2
    put_be32(&mut sb, 0xCC, 0x0000_038A); // bad_features2
    put_be32(&mut sb, 0xD8, 1);           // incompat: FTYPE (1)

    put_be64(&mut sb, 0xF0, 1);           // sb_lsn = 1
    put_slice(&mut sb, 0xF8, &opts.uuid); // meta_uuid

    xfs_write_cksum(&mut sb, SB_CRC_OFF);
    sb
}

/// Format the Allocation Group Free Space (AGF) sector (Sector 1).
pub fn format_agf(total_blocks: u64, free_blocks: u64, uuid: &[u8; 16]) -> [u8; 512] {
    let mut agf = [0u8; 512];
    put_be32(&mut agf, 0x00, XFS_AGF_MAGIC);
    put_be32(&mut agf, 0x04, 1); // version 1
    put_be32(&mut agf, 0x08, 0); // seqno 0
    put_be32(&mut agf, 0x0C, total_blocks as u32); // length
    put_be32(&mut agf, 0x10, 1); // bno_root = block 1
    put_be32(&mut agf, 0x14, 2); // cnt_root = block 2
    put_be32(&mut agf, 0x1C, 1); // bno_level = 1
    put_be32(&mut agf, 0x20, 1); // cnt_level = 1
    put_be32(&mut agf, 0x34, free_blocks as u32); // freeblks
    put_be32(&mut agf, 0x38, free_blocks as u32); // longest
    put_be32(&mut agf, 0x3C, 2); // btreeblks
    put_slice(&mut agf, 0x40, uuid);
    put_be64(&mut agf, 0xD0, 1); // agf_lsn

    xfs_write_cksum(&mut agf, AGF_CRC_OFF);
    agf
}

/// Format the Allocation Group Inode (AGI) sector (Sector 2).
pub fn format_agi(total_blocks: u64, allocated_inodes: u64, free_inodes: u64, uuid: &[u8; 16]) -> [u8; 512] {
    let mut agi = [0u8; 512];
    put_be32(&mut agi, 0x00, XFS_AGI_MAGIC);
    put_be32(&mut agi, 0x04, 1); // version 1
    put_be32(&mut agi, 0x08, 0); // seqno 0
    put_be32(&mut agi, 0x0C, total_blocks as u32); // length
    put_be32(&mut agi, 0x10, allocated_inodes as u32); // count
    put_be32(&mut agi, 0x14, 3); // root = block 3
    put_be32(&mut agi, 0x18, 1); // level = 1
    put_be32(&mut agi, 0x1C, free_inodes as u32); // freecount
    let used = allocated_inodes.saturating_sub(free_inodes);
    let newino = if used > 0 { ROOT_INO + used - 1 } else { 0 };
    put_be32(&mut agi, 0x20, newino as u32); // newino
    put_be32(&mut agi, 0x24, ROOT_INO as u32); // dirino

    // Initialize all 64 unlinked buckets to NULLAGINO (0xFFFFFFFF)
    for i in 0..64 {
        put_be32(&mut agi, 0x28 + i * 4, 0xFFFF_FFFF);
    }

    put_slice(&mut agi, 0x128, uuid);
    put_be64(&mut agi, 0x140, 1); // agi_lsn

    xfs_write_cksum(&mut agi, AGI_CRC_OFF);
    agi
}

/// Format the Allocation Group Free List (AGFL) sector (Sector 3).
pub fn format_agfl(uuid: &[u8; 16]) -> [u8; 512] {
    let mut agfl = [0u8; 512];
    put_be32(&mut agfl, 0x00, XFS_AGFL_MAGIC);
    put_be32(&mut agfl, 0x04, 0); // seqno 0
    put_slice(&mut agfl, 0x08, uuid);
    put_be64(&mut agfl, 0x18, 1); // agfl_lsn

    let mut off = 0x24;
    while off + 4 <= 512 {
        put_be32(&mut agfl, off, 0xFFFF_FFFF);
        off += 4;
    }

    xfs_write_cksum(&mut agfl, AGFL_CRC_OFF);
    agfl
}

/// Format a short btree root block (for bnobt at block 1 or cntbt at block 2).
pub fn format_allocbt_root(magic: u32, blkno: u64, start_free_block: u32, free_blocks: u32, uuid: &[u8; 16]) -> [u8; 4096] {
    let mut blk = [0u8; 4096];
    put_be32(&mut blk, 0x00, magic);
    put_be16(&mut blk, 0x04, 0); // level 0 (leaf)
    put_be16(&mut blk, 0x06, 1); // numrecs 1
    put_be32(&mut blk, 0x08, 0xFFFF_FFFF); // leftsib
    put_be32(&mut blk, 0x0C, 0xFFFF_FFFF); // rightsib
    put_be64(&mut blk, 0x10, blkno * 8);   // blkno (sectors)
    put_slice(&mut blk, 0x20, uuid);
    put_be32(&mut blk, 0x30, 0);           // owner (AG 0)

    // Record 0 at offset 56: ar_startblock (4 bytes), ar_blockcount (4 bytes)
    put_be32(&mut blk, 56, start_free_block);
    put_be32(&mut blk, 60, free_blocks);

    xfs_write_cksum(&mut blk, BTREE_SHDR_CRC_OFF);
    blk
}

/// Format an inobt root block (at block 3).
pub fn format_inobt_root(blkno: u64, start_ino: u32, free_count: u32, free_mask: u64, uuid: &[u8; 16]) -> [u8; 4096] {
    let mut blk = [0u8; 4096];
    put_be32(&mut blk, 0x00, XFS_IAB3_MAGIC);
    put_be16(&mut blk, 0x04, 0); // level 0 (leaf)
    put_be16(&mut blk, 0x06, 1); // numrecs 1
    put_be32(&mut blk, 0x08, 0xFFFF_FFFF); // leftsib
    put_be32(&mut blk, 0x0C, 0xFFFF_FFFF); // rightsib
    put_be64(&mut blk, 0x10, blkno * 8);   // blkno (sectors)
    put_slice(&mut blk, 0x20, uuid);
    put_be32(&mut blk, 0x30, 0);           // owner (AG 0)

    // Record 0 at offset 56: ir_startino (4), ir_freecount (4), ir_free (8)
    put_be32(&mut blk, 56, start_ino);
    put_be32(&mut blk, 60, free_count);
    put_be64(&mut blk, 64, free_mask);

    xfs_write_cksum(&mut blk, BTREE_SHDR_CRC_OFF);
    blk
}

/// Internal representation of an inode.
#[derive(Clone, Debug)]
pub struct InodeRecord {
    pub ino: u64,
    pub kind: FileKind,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
    pub nlink: u32,
    pub size: u64,
    pub format: u8,
    pub extents: Vec<[u8; 16]>,
    pub local_data: Vec<u8>,
    pub rdev: (u32, u32),
    pub parent: u64,
    pub children: BTreeMap<String, (u64, FileKind)>,
}

impl InodeRecord {
    pub fn new(ino: u64, kind: FileKind, meta: &Meta, parent: u64) -> Self {
        let type_mode = match kind {
            FileKind::Regular => S_IFREG,
            FileKind::Dir => S_IFDIR,
            FileKind::Symlink => S_IFLNK,
            FileKind::CharDev => S_IFCHR,
            FileKind::BlockDev => S_IFBLK,
            FileKind::Fifo => S_IFIFO,
        };
        let mode = type_mode | (meta.mode & 0o7777);
        let format = match kind {
            FileKind::Regular => XFS_DINODE_FMT_EXTENTS,
            FileKind::Dir | FileKind::Symlink => XFS_DINODE_FMT_LOCAL,
            FileKind::CharDev | FileKind::BlockDev | FileKind::Fifo => XFS_DINODE_FMT_DEV,
        };
        Self {
            ino,
            kind,
            mode,
            uid: meta.uid,
            gid: meta.gid,
            mtime: meta.mtime,
            nlink: if kind == FileKind::Dir { 2 } else { 1 },
            size: 0,
            format,
            extents: Vec::new(),
            local_data: Vec::new(),
            rdev: (0, 0),
            parent,
            children: BTreeMap::new(),
        }
    }
}

/// Formats an unallocated 512-byte V5 on-disk inode (`xfs_dinode`).
pub fn format_empty_inode(ino: u64, uuid: &[u8; 16]) -> [u8; 512] {
    let mut buf = [0u8; 512];
    put_be16(&mut buf, 0x00, XFS_DINODE_MAGIC);
    put_u8(&mut buf, 0x04, 3); // version 3
    put_be32(&mut buf, 0x60, 0xFFFF_FFFF); // di_next_unlinked = NULLAGINO
    put_be64(&mut buf, 0x98, ino);
    put_slice(&mut buf, 0xA0, uuid);
    xfs_write_cksum(&mut buf, DINODE_CRC_OFF);
    buf
}

/// Formats a complete 512-byte V5 on-disk inode (`xfs_dinode`).
pub fn format_inode(rec: &InodeRecord, uuid: &[u8; 16]) -> [u8; 512] {
    let mut buf = [0u8; 512];
    put_be16(&mut buf, 0x00, XFS_DINODE_MAGIC);
    put_be16(&mut buf, 0x02, rec.mode);
    put_u8(&mut buf, 0x04, 3); // version 3
    put_u8(&mut buf, 0x05, rec.format);
    put_be32(&mut buf, 0x08, rec.uid);
    put_be32(&mut buf, 0x0C, rec.gid);
    put_be32(&mut buf, 0x10, rec.nlink);

    // Timestamps: atime, mtime, ctime (sec, nsec)
    let sec = (rec.mtime & 0xFFFF_FFFF) as u32;
    put_be32(&mut buf, 0x20, sec);
    put_be32(&mut buf, 0x28, sec);
    put_be32(&mut buf, 0x30, sec);

    put_be64(&mut buf, 0x38, rec.size);
    let nblocks = if rec.format == XFS_DINODE_FMT_EXTENTS {
        rec.extents
            .iter()
            .map(|e| unpack_bmbt_rec(e).2 as u64)
            .sum::<u64>()
    } else {
        0
    };
    put_be64(&mut buf, 0x40, nblocks);

    if rec.format == XFS_DINODE_FMT_EXTENTS {
        put_be32(&mut buf, 0x4C, rec.extents.len() as u32);
    }
    put_u8(&mut buf, 0x53, XFS_DINODE_FMT_EXTENTS); // aformat

    put_be32(&mut buf, 0x60, 0xFFFF_FFFF); // di_next_unlinked = NULLAGINO
    put_be64(&mut buf, 0x68, 1);           // di_changecount = 1
    put_be64(&mut buf, 0x70, 1);           // di_lsn = 1

    // Start of V3 fields
    put_be32(&mut buf, 0x90, sec); // crtime
    put_be64(&mut buf, 0x98, rec.ino);
    put_slice(&mut buf, 0xA0, uuid);

    // Data fork at offset 176 (0xB0)
    match rec.format {
        XFS_DINODE_FMT_LOCAL => {
            let n = rec.local_data.len().min(INODE_FORK_SIZE);
            if let Some(src) = rec.local_data.get(..n) {
                put_slice(&mut buf, INODE_CORE_SIZE, src);
            }
        }
        XFS_DINODE_FMT_EXTENTS => {
            let mut off = INODE_CORE_SIZE;
            for ext in &rec.extents {
                if off + 16 <= 512 {
                    put_slice(&mut buf, off, ext);
                    off += 16;
                }
            }
        }
        XFS_DINODE_FMT_DEV => {
            let (maj, min) = rec.rdev;
            let dev = (maj << 8) | (min & 0xFF);
            put_be32(&mut buf, INODE_CORE_SIZE, dev);
        }
        _ => {}
    }

    xfs_write_cksum(&mut buf, DINODE_CRC_OFF);
    buf
}

/// Encodes a shortform directory directly for storage in the inode data fork.
pub fn encode_shortform_dir(parent_ino: u64, entries: &[(String, u64, FileKind)]) -> Vec<u8> {
    let mut data = Vec::new();
    data.push(entries.len() as u8); // count
    data.push(0u8); // i8count = 0 (32-bit inodes)
    data.extend_from_slice(&(parent_ino as u32).to_be_bytes()); // parent

    let mut current_offset = 96u16; // data first offset (64 header + 16 "." + 16 "..")
    for (name, ino, kind) in entries {
        let name_bytes = name.as_bytes();
        data.push(name_bytes.len() as u8);
        data.extend_from_slice(&current_offset.to_be_bytes());
        data.extend_from_slice(name_bytes);

        let ftype = match kind {
            FileKind::Regular => XFS_DIR3_FT_REG_FILE,
            FileKind::Dir => XFS_DIR3_FT_DIR,
            FileKind::Symlink => XFS_DIR3_FT_SYMLINK,
            FileKind::CharDev => XFS_DIR3_FT_CHRDEV,
            FileKind::BlockDev => XFS_DIR3_FT_BLKDEV,
            FileKind::Fifo => XFS_DIR3_FT_FIFO,
        };
        data.push(ftype);
        data.extend_from_slice(&(*ino as u32).to_be_bytes());

        let entsize = (12 + name_bytes.len()).div_ceil(8) * 8;
        current_offset += entsize as u16;
    }
    data
}

/// Encodes a single-block directory (`XDB3`) in a 4096-byte block.
pub fn encode_block_dir(
    dir_ino: u64,
    parent_ino: u64,
    pblk: u64,
    uuid: &[u8; 16],
    entries: &[(String, u64, FileKind)],
) -> [u8; 4096] {
    let mut blk = [0u8; 4096];

    // Header: xfs_dir3_data_hdr (64 bytes)
    put_be32(&mut blk, 0x00, XFS_DIR3_BLOCK_MAGIC);
    put_be64(&mut blk, 0x08, pblk * 8); // blkno (sectors)
    put_slice(&mut blk, 0x18, uuid);
    put_be64(&mut blk, 0x28, dir_ino); // owner

    let mut all_entries: Vec<(&[u8], u64, u8)> = Vec::new();
    all_entries.push((b".", dir_ino, XFS_DIR3_FT_DIR));
    all_entries.push((b"..", parent_ino, XFS_DIR3_FT_DIR));
    for (name, ino, kind) in entries {
        let ft = match kind {
            FileKind::Regular => XFS_DIR3_FT_REG_FILE,
            FileKind::Dir => XFS_DIR3_FT_DIR,
            FileKind::Symlink => XFS_DIR3_FT_SYMLINK,
            FileKind::CharDev => XFS_DIR3_FT_CHRDEV,
            FileKind::BlockDev => XFS_DIR3_FT_BLKDEV,
            FileKind::Fifo => XFS_DIR3_FT_FIFO,
        };
        all_entries.push((name.as_bytes(), *ino, ft));
    }

    let mut leaf_entries: Vec<(u32, u32)> = Vec::new(); // (hashval, address)
    let mut offset = 64usize;

    for (name, ino, ft) in all_entries {
        let tag = offset as u16;
        let namelen = name.len();
        let entsize = (12 + namelen).div_ceil(8) * 8;

        put_be64(&mut blk, offset, ino);
        put_u8(&mut blk, offset + 8, namelen as u8);
        put_slice(&mut blk, offset + 9, name);
        put_u8(&mut blk, offset + 9 + namelen, ft);
        put_be16(&mut blk, offset + entsize - 2, tag);

        let hash = xfs_da_hashname(name);
        let address = (offset >> 3) as u32;
        leaf_entries.push((hash, address));

        offset += entsize;
    }

    // Sort leaf entries ascending by hashval
    leaf_entries.sort_by_key(|&(h, _)| h);

    // Leaf section at end of block
    let num_leaf = leaf_entries.len();
    let tail_off = 4096 - 8;
    let leaf_start = tail_off - num_leaf * 8;

    // Tail: count (4 bytes), stale = 0 (4 bytes)
    put_be32(&mut blk, tail_off, num_leaf as u32);
    put_be32(&mut blk, tail_off + 4, 0);

    for (i, &(hash, addr)) in leaf_entries.iter().enumerate() {
        let e_off = leaf_start + i * 8;
        put_be32(&mut blk, e_off, hash);
        put_be32(&mut blk, e_off + 4, addr);
    }

    // Format unused freespace between offset and leaf_start
    if offset < leaf_start {
        let free_len = (leaf_start - offset) as u16;
        put_be16(&mut blk, offset, 0xFFFF); // freetag
        put_be16(&mut blk, offset + 2, free_len);
        put_be16(&mut blk, leaf_start - 2, offset as u16); // tag

        // best_free[0] in header (offset 48)
        put_be16(&mut blk, 48, offset as u16);
        put_be16(&mut blk, 50, free_len);
    }

    xfs_write_cksum(&mut blk, DIR3_DATA_CRC_OFF);
    blk
}

struct OpenFile {
    ino: u64,
    buf: Vec<u8>,
    size: u64,
    next_lblk: u64,
}

fn split_path(path: &str) -> Result<Vec<&str>> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    Ok(trimmed.split('/').filter(|s| !s.is_empty()).collect())
}

/// Write-once streaming XFS filesystem generator.
pub struct XfsWriter<S: Storage> {
    storage: S,
    total_blocks: u64,
    opts: Options,
    inodes: BTreeMap<u64, InodeRecord>,
    next_ino: u64,
    logstart: u64,
    logblocks: u32,
    next_data_block: u64,
    open: Option<OpenFile>,
    finished: bool,
}

impl<S: Storage> XfsWriter<S> {
    /// Creates a new `XfsWriter` on `storage` with `total_bytes`.
    pub fn new(storage: S, total_bytes: u64, opts: Options) -> Result<Self> {
        if total_bytes < 16 * 1024 * 1024 {
            return Err(Error::Size("filesystem size must be at least 16 MiB"));
        }
        let total_blocks = total_bytes / BLOCK_SIZE;
        let mut inodes = BTreeMap::new();

        // Create root directory inode (ROOT_INO = 128)
        let root_meta = Meta {
            mode: 0o755,
            uid: 0,
            gid: 0,
            mtime: opts.now,
            xattrs: Vec::new(),
        };
        let root_rec = InodeRecord::new(ROOT_INO, FileKind::Dir, &root_meta, ROOT_INO);
        inodes.insert(ROOT_INO, root_rec);

        // Realtime bitmap inode (ROOT_INO + 1 = 129)
        let rt_meta = Meta {
            mode: 0,
            uid: 0,
            gid: 0,
            mtime: opts.now,
            xattrs: Vec::new(),
        };
        let rbmino_rec = InodeRecord::new(ROOT_INO + 1, FileKind::Regular, &rt_meta, 0);
        inodes.insert(ROOT_INO + 1, rbmino_rec);

        // Realtime summary inode (ROOT_INO + 2 = 130)
        let rsumino_rec = InodeRecord::new(ROOT_INO + 2, FileKind::Regular, &rt_meta, 0);
        inodes.insert(ROOT_INO + 2, rsumino_rec);

        let logstart = 64u64;
        let logblocks = if total_blocks >= 6000 {
            2560u32
        } else {
            (total_blocks / 4).max(512) as u32
        };
        let next_data_block = logstart + logblocks as u64;

        Ok(Self {
            storage,
            total_blocks,
            opts,
            inodes,
            next_ino: ROOT_INO + 3,
            logstart,
            logblocks,
            next_data_block,
            open: None,
            finished: false,
        })
    }

    fn alloc_ino(&mut self) -> Result<u64> {
        let ino = self.next_ino;
        self.next_ino += 1;
        Ok(ino)
    }

    fn parent_of(&mut self, path: &str) -> Result<(u64, String)> {
        let parts = split_path(path)?;
        if parts.is_empty() {
            return Err(Error::Invalid("empty path has no parent"));
        }
        let (name, parent_parts) = parts.split_last().ok_or(Error::Invalid("empty path"))?;
        let name_str = String::from(*name);
        let mut cur = ROOT_INO;
        for part in parent_parts {
            let part_str = *part;
            let existing = self
                .inodes
                .get(&cur)
                .ok_or(Error::NotFound)?
                .children
                .get(part_str)
                .copied();
            match existing {
                Some((ino, FileKind::Dir)) => {
                    cur = ino;
                }
                Some(_) => {
                    return Err(Error::Invalid("component is not a directory"));
                }
                None => {
                    let new_dir_ino = self.alloc_ino()?;
                    let meta = Meta {
                        mode: 0o755,
                        uid: 0,
                        gid: 0,
                        mtime: self.opts.now,
                        xattrs: Vec::new(),
                    };
                    let rec = InodeRecord::new(new_dir_ino, FileKind::Dir, &meta, cur);
                    self.inodes.insert(new_dir_ino, rec);

                    let parent_node = self.inodes.get_mut(&cur).ok_or(Error::NotFound)?;
                    parent_node.children.insert(String::from(part_str), (new_dir_ino, FileKind::Dir));
                    parent_node.nlink += 1;

                    cur = new_dir_ino;
                }
            }
        }
        Ok((cur, name_str))
    }

    fn alloc_blocks(&mut self, count: u64) -> Result<u64> {
        if self.next_data_block + count > self.total_blocks {
            return Err(Error::NoSpace);
        }
        let blk = self.next_data_block;
        self.next_data_block += count;
        Ok(blk)
    }

    fn replace_slot(&mut self, parent: u64, name: &str) -> Result<()> {
        let parent_node = self.inodes.get_mut(&parent).ok_or(Error::NotFound)?;
        if let Some((ino, kind)) = parent_node.children.remove(name) {
            if kind == FileKind::Dir {
                return Err(Error::Invalid("cannot overwrite directory with file"));
            }
            if let Some(target) = self.inodes.get_mut(&ino) {
                target.nlink = target.nlink.saturating_sub(1);
            }
        }
        Ok(())
    }

    /// Creates a directory at `path` with permissions `meta`.
    pub fn mkdir(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        let parts = split_path(path)?;
        if parts.is_empty() {
            // Root directory
            let root = self.inodes.get_mut(&ROOT_INO).ok_or(Error::NotFound)?;
            root.mode = S_IFDIR | (meta.mode & 0o7777);
            root.uid = meta.uid;
            root.gid = meta.gid;
            root.mtime = meta.mtime;
            return Ok(ROOT_INO);
        }

        let (parent, name) = self.parent_of(path)?;
        let existing = self
            .inodes
            .get(&parent)
            .ok_or(Error::NotFound)?
            .children
            .get(&name)
            .copied();

        if let Some((existing_ino, kind)) = existing {
            if kind == FileKind::Dir {
                let d = self.inodes.get_mut(&existing_ino).ok_or(Error::NotFound)?;
                d.mode = S_IFDIR | (meta.mode & 0o7777);
                d.uid = meta.uid;
                d.gid = meta.gid;
                d.mtime = meta.mtime;
                return Ok(existing_ino);
            } else {
                return Err(Error::Exists);
            }
        }

        let ino = self.alloc_ino()?;
        let rec = InodeRecord::new(ino, FileKind::Dir, meta, parent);
        self.inodes.insert(ino, rec);

        let parent_node = self.inodes.get_mut(&parent).ok_or(Error::NotFound)?;
        parent_node.children.insert(name, (ino, FileKind::Dir));
        parent_node.nlink += 1;

        Ok(ino)
    }

    /// Starts writing a new regular file at `path`.
    pub fn begin_file(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        if self.open.is_some() {
            return Err(Error::Invalid("a file is already open"));
        }
        let (parent, name) = self.parent_of(path)?;
        self.replace_slot(parent, &name)?;

        let ino = self.alloc_ino()?;
        let rec = InodeRecord::new(ino, FileKind::Regular, meta, parent);
        self.inodes.insert(ino, rec);

        let parent_node = self.inodes.get_mut(&parent).ok_or(Error::NotFound)?;
        parent_node.children.insert(name, (ino, FileKind::Regular));

        self.open = Some(OpenFile {
            ino,
            buf: Vec::new(),
            size: 0,
            next_lblk: 0,
        });

        Ok(ino)
    }

    /// Streams sequential data into the currently open file.
    pub fn write_file(&mut self, data: &[u8]) -> Result<()> {
        let of = self.open.as_mut().ok_or(Error::Invalid("no open file"))?;
        of.buf.extend_from_slice(data);
        of.size += data.len() as u64;

        if of.buf.len() >= (1 << 20) {
            self.flush_open(false)?;
        }
        Ok(())
    }

    fn flush_open(&mut self, last: bool) -> Result<()> {
        let mut of = self.open.take().ok_or(Error::Invalid("no open file"))?;
        let mut buf = core::mem::take(&mut of.buf);

        if last && !(buf.len() as u64).is_multiple_of(BLOCK_SIZE) {
            let padded_len = (buf.len() as u64).div_ceil(BLOCK_SIZE) * BLOCK_SIZE;
            buf.resize(padded_len as usize, 0);
        }

        let full_blocks = (buf.len() as u64 / BLOCK_SIZE) as usize;
        let mut done_blocks = 0;

        while done_blocks < full_blocks {
            let want = (full_blocks - done_blocks) as u64;
            let pblk = self.alloc_blocks(want)?;
            let chunk_bytes = (want * BLOCK_SIZE) as usize;
            let start = done_blocks * BLOCK_SIZE as usize;
            let end = start + chunk_bytes;

            if let Some(chunk) = buf.get(start..end) {
                self.storage.write_at(pblk * BLOCK_SIZE, chunk)?;
            }

            let rec = self.inodes.get_mut(&of.ino).ok_or(Error::NotFound)?;
            let mut coalesced = false;
            if let Some(last_ext) = rec.extents.last_mut() {
                let (s_off, s_blk, count, unwritten) = unpack_bmbt_rec(last_ext);
                if !unwritten
                    && s_off + (count as u64) == of.next_lblk
                    && s_blk + (count as u64) == pblk
                    && (count as u64 + want) <= 0x1F_FFFF
                {
                    *last_ext = pack_bmbt_rec(s_off, s_blk, count + want as u32, false);
                    coalesced = true;
                }
            }
            if !coalesced {
                rec.extents.push(pack_bmbt_rec(of.next_lblk, pblk, want as u32, false));
            }

            of.next_lblk += want;
            done_blocks += want as usize;
        }

        buf.drain(..done_blocks * BLOCK_SIZE as usize);
        of.buf = buf;
        self.open = Some(of);
        Ok(())
    }

    /// Finalizes the currently open file and returns its inode.
    pub fn end_file(&mut self) -> Result<u64> {
        if self.open.is_none() {
            return Err(Error::Invalid("no open file"));
        }
        self.flush_open(true)?;
        let of = self.open.take().ok_or(Error::Invalid("no open file"))?;
        let ino = of.ino;
        let rec = self.inodes.get_mut(&ino).ok_or(Error::NotFound)?;
        rec.size = of.size;
        Ok(ino)
    }

    /// Creates a symbolic link at `path` pointing to `target`.
    pub fn symlink(&mut self, path: &str, target: &str, meta: &Meta) -> Result<u64> {
        let (parent, name) = self.parent_of(path)?;
        self.replace_slot(parent, &name)?;

        let ino = self.alloc_ino()?;
        let mut rec = InodeRecord::new(ino, FileKind::Symlink, meta, parent);
        rec.size = target.len() as u64;

        if target.len() <= INODE_FORK_SIZE {
            rec.format = XFS_DINODE_FMT_LOCAL;
            rec.local_data = target.as_bytes().to_vec();
        } else {
            rec.format = XFS_DINODE_FMT_EXTENTS;
            let blocks = (target.len() as u64).div_ceil(BLOCK_SIZE);
            let pblk = self.alloc_blocks(blocks)?;
            let mut block_data = alloc::vec![0u8; (blocks * BLOCK_SIZE) as usize];
            if let Some(dst) = block_data.get_mut(..target.len()) {
                dst.copy_from_slice(target.as_bytes());
            }
            self.storage.write_at(pblk * BLOCK_SIZE, &block_data)?;
            rec.extents.push(pack_bmbt_rec(0, pblk, blocks as u32, false));
        }

        self.inodes.insert(ino, rec);
        let parent_node = self.inodes.get_mut(&parent).ok_or(Error::NotFound)?;
        parent_node.children.insert(name, (ino, FileKind::Symlink));

        Ok(ino)
    }

    /// Creates a hard link at `path` pointing to existing file `existing`.
    pub fn hardlink(&mut self, path: &str, existing: &str) -> Result<u64> {
        let target_ino = self.lookup(existing).ok_or(Error::NotFound)?;
        let target_kind = self
            .inodes
            .get(&target_ino)
            .ok_or(Error::NotFound)?
            .kind;
        if target_kind == FileKind::Dir {
            return Err(Error::Invalid("cannot hardlink a directory"));
        }

        let (parent, name) = self.parent_of(path)?;
        self.replace_slot(parent, &name)?;

        let parent_node = self.inodes.get_mut(&parent).ok_or(Error::NotFound)?;
        parent_node.children.insert(name, (target_ino, target_kind));

        let target_node = self.inodes.get_mut(&target_ino).ok_or(Error::NotFound)?;
        target_node.nlink += 1;

        Ok(target_ino)
    }

    /// Creates a device node (char or block) or a FIFO.
    pub fn mknod(&mut self, path: &str, kind: FileKind, major: u32, minor: u32, meta: &Meta) -> Result<u64> {
        if !matches!(kind, FileKind::CharDev | FileKind::BlockDev | FileKind::Fifo) {
            return Err(Error::Invalid("not a device node or fifo"));
        }
        let (parent, name) = self.parent_of(path)?;
        self.replace_slot(parent, &name)?;

        let ino = self.alloc_ino()?;
        let mut rec = InodeRecord::new(ino, kind, meta, parent);
        rec.rdev = (major, minor);
        self.inodes.insert(ino, rec);

        let parent_node = self.inodes.get_mut(&parent).ok_or(Error::NotFound)?;
        parent_node.children.insert(name, (ino, kind));

        Ok(ino)
    }

    /// Looks up an existing entry by path and returns its inode number.
    pub fn lookup(&self, path: &str) -> Option<u64> {
        let parts = split_path(path).ok()?;
        let mut cur = ROOT_INO;
        for part in parts {
            let node = self.inodes.get(&cur)?;
            if node.kind != FileKind::Dir {
                return None;
            }
            cur = node.children.get(part)?.0;
        }
        Some(cur)
    }

    /// Returns the size in bytes of the file identified by `id`.
    pub fn file_size(&self, id: u64) -> u64 {
        self.inodes.get(&id).map_or(0, |n| n.size)
    }

    /// Reads data from a file previously written.
    pub fn read_file(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let node = self.inodes.get(&id).ok_or(Error::NotFound)?;
        if node.kind != FileKind::Regular && node.kind != FileKind::Symlink {
            return Err(Error::Invalid("not a readable regular file or symlink"));
        }
        if offset >= node.size {
            return Ok(0);
        }
        let avail = (node.size - offset) as usize;
        let take = buf.len().min(avail);

        if node.format == XFS_DINODE_FMT_LOCAL {
            let off = offset as usize;
            if let Some(src) = node.local_data.get(off..off + take) {
                if let Some(dst) = buf.get_mut(..take) {
                    dst.copy_from_slice(src);
                }
            }
            return Ok(take);
        }

        let mut done = 0usize;
        while done < take {
            let cur_off = offset + done as u64;
            let lblk = cur_off / BLOCK_SIZE;
            let in_blk = cur_off % BLOCK_SIZE;

            // Find extent containing lblk
            let mut found = None;
            for ext in &node.extents {
                let (s_off, s_blk, count, _) = unpack_bmbt_rec(ext);
                if lblk >= s_off && lblk < s_off + count as u64 {
                    let pblk = s_blk + (lblk - s_off);
                    found = Some(pblk);
                    break;
                }
            }
            let pblk = found.ok_or(Error::Io)?;
            let to_read = (take - done).min((BLOCK_SIZE - in_blk) as usize);
            if let Some(dst) = buf.get_mut(done..done + to_read) {
                self.storage.read_at(pblk * BLOCK_SIZE + in_blk, dst)?;
            }
            done += to_read;
        }

        Ok(take)
    }

    /// Flushes all cached metadata, superblocks, and journals to disk.
    pub fn finish(&mut self) -> Result<()> {
        if self.open.is_some() {
            return Err(Error::Invalid("cannot finish with open file"));
        }
        if self.finished {
            return Ok(());
        }

        // 1. Finalize directories: convert to shortform or block directory
        let dir_inos: Vec<u64> = self.inodes
            .iter()
            .filter(|(_, r)| r.kind == FileKind::Dir)
            .map(|(k, _)| *k)
            .collect();

        for dir_ino in dir_inos {
            let (parent_ino, entries) = {
                let d = self.inodes.get(&dir_ino).ok_or(Error::NotFound)?;
                let entries: Vec<(String, u64, FileKind)> = d.children
                    .iter()
                    .map(|(k, &(i, f))| (k.clone(), i, f))
                    .collect();
                (d.parent, entries)
            };

            let sf_bytes = encode_shortform_dir(parent_ino, &entries);
            if sf_bytes.len() <= INODE_FORK_SIZE {
                let d = self.inodes.get_mut(&dir_ino).ok_or(Error::NotFound)?;
                d.format = XFS_DINODE_FMT_LOCAL;
                d.size = sf_bytes.len() as u64;
                d.local_data = sf_bytes;
            } else {
                // Exceeds shortform capacity: allocate 1 block and write XDB3 block dir
                let pblk = self.alloc_blocks(1)?;
                let blk = encode_block_dir(dir_ino, parent_ino, pblk, &self.opts.uuid, &entries);
                self.storage.write_at(pblk * BLOCK_SIZE, &blk)?;

                let d = self.inodes.get_mut(&dir_ino).ok_or(Error::NotFound)?;
                d.format = XFS_DINODE_FMT_EXTENTS;
                d.size = BLOCK_SIZE;
                d.extents = alloc::vec![pack_bmbt_rec(0, pblk, 1, false)];
            }
        }

        // 2. Write all inode blocks in chunks of 64 inodes
        let max_ino = self.inodes.keys().copied().max().unwrap_or(ROOT_INO);
        let start_ino = ROOT_INO;
        let needed_inodes = max_ino - start_ino + 1;
        let count_inos = needed_inodes.max(64).div_ceil(64) * 64;
        let inode_blocks = count_inos / INOPBLOCK;

        for blk_idx in 0..inode_blocks {
            let mut chunk = [0u8; 4096];
            for slot in 0..INOPBLOCK {
                let ino = start_ino + blk_idx * INOPBLOCK + slot;
                let formatted = if let Some(rec) = self.inodes.get(&ino) {
                    format_inode(rec, &self.opts.uuid)
                } else {
                    format_empty_inode(ino, &self.opts.uuid)
                };
                let off = slot as usize * INODE_SIZE as usize;
                put_slice(&mut chunk, off, &formatted);
            }
            let pblk = (start_ino / INOPBLOCK) + blk_idx;
            self.storage.write_at(pblk * BLOCK_SIZE, &chunk)?;
        }

        // 3. Write Sector 0..3
        let free_blocks = self.total_blocks.saturating_sub(self.next_data_block);
        let total_inodes = count_inos;
        let used_inodes = self.inodes.len() as u64;
        let free_inodes = total_inodes.saturating_sub(used_inodes);

        let sb = format_superblock(
            self.total_blocks,
            ROOT_INO,
            free_blocks,
            total_inodes,
            free_inodes,
            self.logstart,
            self.logblocks,
            &self.opts,
        );
        let agf = format_agf(self.total_blocks, free_blocks, &self.opts.uuid);
        let agi = format_agi(self.total_blocks, total_inodes, free_inodes, &self.opts.uuid);
        let agfl = format_agfl(&self.opts.uuid);

        let mut block0 = [0u8; 4096];
        put_slice(&mut block0, 0x000, &sb);
        put_slice(&mut block0, 0x200, &agf);
        put_slice(&mut block0, 0x400, &agi);
        put_slice(&mut block0, 0x600, &agfl);
        self.storage.write_at(0, &block0)?;

        // 4. Write btree root blocks (block 1 bnobt, block 2 cntbt, block 3 inobt)
        let bnobt = format_allocbt_root(XFS_AB3B_MAGIC, 1, self.next_data_block as u32, free_blocks as u32, &self.opts.uuid);
        let cntbt = format_allocbt_root(XFS_AB3C_MAGIC, 2, self.next_data_block as u32, free_blocks as u32, &self.opts.uuid);
        let free_mask = if used_inodes >= 64 {
            0
        } else {
            !((1u64 << used_inodes) - 1)
        };
        let inobt = format_inobt_root(3, ROOT_INO as u32, free_inodes.min(64) as u32, free_mask, &self.opts.uuid);

        self.storage.write_at(BLOCK_SIZE, &bnobt)?;
        self.storage.write_at(2 * BLOCK_SIZE, &cntbt)?;
        self.storage.write_at(3 * BLOCK_SIZE, &inobt)?;

        self.finished = true;
        Ok(())
    }
}

impl<S: Storage> FsWriter for XfsWriter<S> {
    fn mkdir(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        XfsWriter::mkdir(self, path, meta)
    }

    fn begin_file(&mut self, path: &str, meta: &Meta) -> Result<u64> {
        XfsWriter::begin_file(self, path, meta)
    }

    fn write_file(&mut self, data: &[u8]) -> Result<()> {
        XfsWriter::write_file(self, data)
    }

    fn end_file(&mut self) -> Result<u64> {
        XfsWriter::end_file(self)
    }

    fn symlink(&mut self, path: &str, target: &str, meta: &Meta) -> Result<u64> {
        XfsWriter::symlink(self, path, target, meta)
    }

    fn hardlink(&mut self, path: &str, existing: &str) -> Result<u64> {
        XfsWriter::hardlink(self, path, existing)
    }

    fn mknod(&mut self, path: &str, kind: FileKind, major: u32, minor: u32, meta: &Meta) -> Result<u64> {
        XfsWriter::mknod(self, path, kind, major, minor, meta)
    }

    fn lookup(&self, path: &str) -> Option<u64> {
        XfsWriter::lookup(self, path)
    }

    fn file_size(&self, id: u64) -> u64 {
        XfsWriter::file_size(self, id)
    }

    fn read_file(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize> {
        XfsWriter::read_file(self, id, offset, buf)
    }

    fn finish(&mut self) -> Result<()> {
        XfsWriter::finish(self)
    }
}

#[cfg(test)]
#[expect(clippy::get_unwrap, reason = "test assertions on known inserted map keys")]
mod tests {
    use super::*;
    use alloc::vec;
    use crate::crc::xfs_verify_cksum;

    struct MemStorage(Vec<u8>);

    impl MemStorage {
        fn new(size: usize) -> Self {
            Self(vec![0u8; size])
        }
    }

    impl Storage for MemStorage {
        fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
            let end = (offset as usize) + data.len();
            if end > self.0.len() {
                self.0.resize(end, 0);
            }
            if let Some(slot) = self.0.get_mut(offset as usize..end) {
                slot.copy_from_slice(data);
            }
            Ok(())
        }

        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
            let off = offset as usize;
            if off + buf.len() > self.0.len() {
                return Err(Error::Io);
            }
            if let Some(src) = self.0.get(off..off + buf.len()) {
                buf.copy_from_slice(src);
            }
            Ok(())
        }
    }

    fn test_opts() -> Options {
        Options {
            label: "testroot".into(),
            uuid: [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
            now: 1700000000,
        }
    }

    #[test]
    fn test_xfs_da_hashname_known_vectors() {
        assert_eq!(xfs_da_hashname(b""), 0);
        let h_dot = xfs_da_hashname(b".");
        let h_dotdot = xfs_da_hashname(b"..");
        assert_ne!(h_dot, 0);
        assert_ne!(h_dotdot, 0);
        assert_ne!(h_dot, h_dotdot);

        let h1 = xfs_da_hashname(b"test");
        let h2 = xfs_da_hashname(b"test1");
        assert_ne!(h1, h2);

        // Deterministic
        assert_eq!(xfs_da_hashname(b"test1"), h2);

        // Multi-length strings
        assert_ne!(xfs_da_hashname(b"a"), 0);
        assert_ne!(xfs_da_hashname(b"ab"), 0);
        assert_ne!(xfs_da_hashname(b"abc"), 0);
        assert_ne!(xfs_da_hashname(b"abcd"), 0);
        assert_ne!(xfs_da_hashname(b"abcdefg"), 0);
        assert_ne!(xfs_da_hashname(b"abcdefgh"), 0);
    }

    #[test]
    fn test_extent_packing_unpacking() {
        let (startoff, startblock, blockcount, unwritten) = (0, 32, 1, false);
        let rec = pack_bmbt_rec(startoff, startblock, blockcount, unwritten);
        let unpacked = unpack_bmbt_rec(&rec);
        assert_eq!(unpacked, (startoff, startblock, blockcount, unwritten));

        let (startoff2, startblock2, blockcount2, unwritten2) = (1000, 0x12345, 512, true);
        let rec2 = pack_bmbt_rec(startoff2, startblock2, blockcount2, unwritten2);
        let unpacked2 = unpack_bmbt_rec(&rec2);
        assert_eq!(unpacked2, (startoff2, startblock2, blockcount2, unwritten2));
    }

    #[test]
    fn test_superblock_formatting_and_crc() {
        let opts = test_opts();
        let total_blocks = 4096;
        let rootino = 128;
        let logstart = 64;
        let logblocks = 1024;
        let sb = format_superblock(total_blocks, rootino, 3000, 64, 63, logstart, logblocks, &opts);

        assert_eq!(&sb[0..4], b"XFSB");
        assert_eq!(u32::from_be_bytes(sb[4..8].try_into().unwrap()), 4096);
        assert_eq!(u64::from_be_bytes(sb[8..16].try_into().unwrap()), total_blocks);
        assert_eq!(&sb[0x20..0x30], &opts.uuid);
        assert_eq!(u64::from_be_bytes(sb[0x30..0x38].try_into().unwrap()), logstart);
        assert_eq!(u64::from_be_bytes(sb[0x38..0x40].try_into().unwrap()), rootino);
        assert_eq!(u64::from_be_bytes(sb[0x40..0x48].try_into().unwrap()), rootino + 1);
        assert_eq!(u64::from_be_bytes(sb[0x48..0x50].try_into().unwrap()), rootino + 2);
        assert_eq!(u32::from_be_bytes(sb[0x50..0x54].try_into().unwrap()), 1); // rextsize
        assert_eq!(u32::from_be_bytes(sb[0x60..0x64].try_into().unwrap()), logblocks);
        assert_eq!(u16::from_be_bytes(sb[0x68..0x6A].try_into().unwrap()), 512); // inodesize
        assert_eq!(u16::from_be_bytes(sb[0x6A..0x6C].try_into().unwrap()), 8);   // inopblock
        assert_eq!(u32::from_be_bytes(sb[0xD8..0xDC].try_into().unwrap()), 1);   // incompat FTYPE
        assert!(xfs_verify_cksum(&sb, SB_CRC_OFF));
    }

    #[test]
    fn test_ag_headers_and_crc() {
        let opts = test_opts();
        let total_blocks = 4096;
        let agf = format_agf(total_blocks, 4000, &opts.uuid);
        assert_eq!(&agf[0..4], b"XAGF");
        assert!(xfs_verify_cksum(&agf, AGF_CRC_OFF));

        let agi = format_agi(total_blocks, 64, 63, &opts.uuid);
        assert_eq!(&agi[0..4], b"XAGI");
        assert!(xfs_verify_cksum(&agi, AGI_CRC_OFF));

        let agfl = format_agfl(&opts.uuid);
        assert_eq!(&agfl[0..4], b"XAFL");
        assert!(xfs_verify_cksum(&agfl, AGFL_CRC_OFF));
    }

    #[test]
    fn test_btree_root_blocks_and_crc() {
        let opts = test_opts();
        let bnobt = format_allocbt_root(XFS_AB3B_MAGIC, 1, 32, 4000, &opts.uuid);
        assert_eq!(&bnobt[0..4], &XFS_AB3B_MAGIC.to_be_bytes());
        assert!(xfs_verify_cksum(&bnobt, BTREE_SHDR_CRC_OFF));

        let cntbt = format_allocbt_root(XFS_AB3C_MAGIC, 2, 32, 4000, &opts.uuid);
        assert_eq!(&cntbt[0..4], &XFS_AB3C_MAGIC.to_be_bytes());
        assert!(xfs_verify_cksum(&cntbt, BTREE_SHDR_CRC_OFF));

        let inobt = format_inobt_root(3, 128, 0, 0, &opts.uuid);
        assert_eq!(&inobt[0..4], &XFS_IAB3_MAGIC.to_be_bytes());
        assert!(xfs_verify_cksum(&inobt, BTREE_SHDR_CRC_OFF));
    }

    #[test]
    fn test_inode_formatting_and_crc() {
        let opts = test_opts();
        let meta = Meta {
            mode: 0o755,
            uid: 1000,
            gid: 1000,
            mtime: opts.now,
            xattrs: Vec::new(),
        };
        let rec = InodeRecord::new(128, FileKind::Dir, &meta, 128);
        let raw = format_inode(&rec, &opts.uuid);

        assert_eq!(&raw[0..2], b"IN");
        assert_eq!(raw[4], 3); // version 3
        assert_eq!(&raw[0x60..0x64], &[0xFF, 0xFF, 0xFF, 0xFF]); // NULLAGINO
        assert_eq!(u64::from_be_bytes(raw[0x68..0x70].try_into().unwrap()), 1); // changecount
        assert_eq!(u64::from_be_bytes(raw[0x70..0x78].try_into().unwrap()), 1); // lsn
        assert_eq!(&raw[0xA0..0xB0], &opts.uuid);
        assert_eq!(u64::from_be_bytes(raw[0x98..0xA0].try_into().unwrap()), 128);
        assert!(xfs_verify_cksum(&raw, DINODE_CRC_OFF));
    }

    #[test]
    fn test_empty_inode_formatting_and_crc() {
        let opts = test_opts();
        let raw = format_empty_inode(129, &opts.uuid);

        assert_eq!(&raw[0..2], b"IN");
        assert_eq!(raw[4], 3); // version 3
        assert_eq!(u16::from_be_bytes(raw[2..4].try_into().unwrap()), 0); // mode = 0
        assert_eq!(&raw[0x60..0x64], &[0xFF, 0xFF, 0xFF, 0xFF]); // NULLAGINO
        assert_eq!(u64::from_be_bytes(raw[0x98..0xA0].try_into().unwrap()), 129);
        assert_eq!(&raw[0xA0..0xB0], &opts.uuid);
        assert!(xfs_verify_cksum(&raw, DINODE_CRC_OFF));
    }

    #[test]
    fn test_empty_filesystem_creation_and_finish() {
        let size = 16 * 1024 * 1024;
        let storage = MemStorage::new(size);
        let opts = test_opts();
        let mut xfs = XfsWriter::new(storage, size as u64, opts.clone()).unwrap();
        xfs.finish().unwrap();

        // Check Sector 0..3 on storage
        let mut sector = [0u8; 512];
        xfs.storage.read_at(0, &mut sector).unwrap();
        assert_eq!(&sector[0..4], b"XFSB");
        assert!(xfs_verify_cksum(&sector, SB_CRC_OFF));

        xfs.storage.read_at(512, &mut sector).unwrap();
        assert_eq!(&sector[0..4], b"XAGF");
        assert!(xfs_verify_cksum(&sector, AGF_CRC_OFF));

        xfs.storage.read_at(1024, &mut sector).unwrap();
        assert_eq!(&sector[0..4], b"XAGI");
        assert!(xfs_verify_cksum(&sector, AGI_CRC_OFF));

        xfs.storage.read_at(1536, &mut sector).unwrap();
        assert_eq!(&sector[0..4], b"XAFL");
        assert!(xfs_verify_cksum(&sector, AGFL_CRC_OFF));

        // Root inode at block 16 (offset 16 * 4096 = 65536)
        xfs.storage.read_at(65536, &mut sector).unwrap();
        assert_eq!(&sector[0..2], b"IN");
        assert!(xfs_verify_cksum(&sector, DINODE_CRC_OFF));

        // All 64 inodes in the first chunk must be formatted and valid CRC
        for ino_idx in 0..64 {
            let offset = 65536 + ino_idx * 512;
            let mut ino_buf = [0u8; 512];
            xfs.storage.read_at(offset, &mut ino_buf).unwrap();
            assert_eq!(&ino_buf[0..2], b"IN");
            assert_eq!(ino_buf[4], 3);
            assert_eq!(&ino_buf[0x60..0x64], &[0xFF, 0xFF, 0xFF, 0xFF]);
            assert!(xfs_verify_cksum(&ino_buf, DINODE_CRC_OFF));
        }
    }

    #[test]
    fn test_mkdir_hierarchy_and_edge_cases() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta {
            mode: 0o755,
            uid: 0,
            gid: 0,
            mtime: 12345,
            xattrs: Vec::new(),
        };

        // mkdir on root returns root inode 128
        let r1 = xfs.mkdir("/", &meta).unwrap();
        assert_eq!(r1, ROOT_INO);
        let r2 = xfs.mkdir("", &meta).unwrap();
        assert_eq!(r2, ROOT_INO);

        // nested dirs
        let etc = xfs.mkdir("etc", &meta).unwrap();
        assert_eq!(etc, 131);
        assert_eq!(xfs.lookup("etc"), Some(131));

        let systemd = xfs.mkdir("etc/systemd", &meta).unwrap();
        assert_eq!(systemd, 132);
        assert_eq!(xfs.lookup("etc/systemd"), Some(132));

        // re-mkdir existing dir updates metadata and succeeds
        let meta2 = Meta { mode: 0o700, ..meta.clone() };
        let etc_again = xfs.mkdir("etc", &meta2).unwrap();
        assert_eq!(etc_again, etc);
        assert_eq!(xfs.inodes.get(&etc).unwrap().mode & 0o777, 0o700);

        // lookup nonexistent
        assert_eq!(xfs.lookup("nonexistent"), None);
        assert_eq!(xfs.lookup("etc/nonexistent"), None);
    }

    #[test]
    fn test_regular_file_write_and_read_back() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta {
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            mtime: 5555,
            xattrs: Vec::new(),
        };

        let ino = xfs.begin_file("etc/hostname", &meta).unwrap();
        assert_eq!(xfs.lookup("etc"), Some(131));
        assert_eq!(ino, 132);
        xfs.write_file(b"arch").unwrap();
        xfs.write_file(b"staler\n").unwrap();
        let ended = xfs.end_file().unwrap();
        assert_eq!(ended, ino);

        assert_eq!(xfs.lookup("etc/hostname"), Some(ino));
        assert_eq!(xfs.file_size(ino), 11);

        // Read back exact content
        let mut buf = [0u8; 11];
        let n = xfs.read_file(ino, 0, &mut buf).unwrap();
        assert_eq!(n, 11);
        assert_eq!(&buf, b"archstaler\n");

        // Partial read at offset
        let mut part = [0u8; 6];
        let pn = xfs.read_file(ino, 4, &mut part).unwrap();
        assert_eq!(pn, 6);
        assert_eq!(&part, b"staler");

        // Read past EOF
        let mut eof = [0u8; 10];
        let en = xfs.read_file(ino, 100, &mut eof).unwrap();
        assert_eq!(en, 0);
    }

    #[test]
    fn test_file_already_open_and_orphan_writes_edge_cases() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta::default();

        // Write without begin_file
        assert_eq!(xfs.write_file(b"data"), Err(Error::Invalid("no open file")));
        assert_eq!(xfs.end_file(), Err(Error::Invalid("no open file")));

        // Begin file twice
        xfs.begin_file("file1", &meta).unwrap();
        assert_eq!(
            xfs.begin_file("file2", &meta),
            Err(Error::Invalid("a file is already open"))
        );
        xfs.end_file().unwrap();
    }

    #[test]
    fn test_large_file_streaming_and_coalescing() {
        let size = 32 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta::default();

        let ino = xfs.begin_file("bigfile", &meta).unwrap();
        // 64 KiB of data (16 blocks)
        let chunk = [0xABu8; 4096];
        for _ in 0..16 {
            xfs.write_file(&chunk).unwrap();
        }
        xfs.end_file().unwrap();

        assert_eq!(xfs.file_size(ino), 64 * 1024);
        let rec = xfs.inodes.get(&ino).unwrap();
        // Extents should coalesce into a single extent of 16 blocks
        assert_eq!(rec.extents.len(), 1);
        let (_, _, count, _) = unpack_bmbt_rec(&rec.extents[0]);
        assert_eq!(count, 16);

        // Read back across blocks
        let mut read_buf = [0u8; 4096];
        xfs.read_file(ino, 4096 * 5, &mut read_buf).unwrap();
        assert_eq!(read_buf, chunk);
    }

    #[test]
    fn test_symlinks_inline_and_long() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta::default();

        // 1. Short inline symlink
        let sym_short = xfs.symlink("shortlink", "/bin/sh", &meta).unwrap();
        assert_eq!(xfs.lookup("shortlink"), Some(sym_short));
        assert_eq!(xfs.file_size(sym_short), 7);
        let rec = xfs.inodes.get(&sym_short).unwrap();
        assert_eq!(rec.format, XFS_DINODE_FMT_LOCAL);

        let mut buf = [0u8; 7];
        xfs.read_file(sym_short, 0, &mut buf).unwrap();
        assert_eq!(&buf, b"/bin/sh");

        // 2. Long symlink (> 336 bytes)
        let long_target = alloc::vec![b'a'; 400];
        let long_str = core::str::from_utf8(&long_target).unwrap();
        let sym_long = xfs.symlink("longlink", long_str, &meta).unwrap();
        let rec_long = xfs.inodes.get(&sym_long).unwrap();
        assert_eq!(rec_long.format, XFS_DINODE_FMT_EXTENTS);
        assert_eq!(xfs.file_size(sym_long), 400);

        let mut long_read = alloc::vec![0u8; 400];
        xfs.read_file(sym_long, 0, &mut long_read).unwrap();
        assert_eq!(long_read, long_target);
    }

    #[test]
    fn test_device_nodes_mknod() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta { mode: 0o660, ..Default::default() };

        let null_dev = xfs.mknod("dev/null", FileKind::CharDev, 1, 3, &meta).unwrap();
        assert_eq!(xfs.lookup("dev/null"), Some(null_dev));
        let rec = xfs.inodes.get(&null_dev).unwrap();
        assert_eq!(rec.format, XFS_DINODE_FMT_DEV);
        assert_eq!(rec.mode & S_IFCHR, S_IFCHR);
        assert_eq!(rec.rdev, (1, 3));

        let sda = xfs.mknod("dev/sda", FileKind::BlockDev, 8, 0, &meta).unwrap();
        assert_eq!(xfs.lookup("dev/sda"), Some(sda));
        let rec_sda = xfs.inodes.get(&sda).unwrap();
        assert_eq!(rec_sda.mode & S_IFBLK, S_IFBLK);
        assert_eq!(rec_sda.rdev, (8, 0));

        let fifo = xfs.mknod("dev/initctl", FileKind::Fifo, 0, 0, &meta).unwrap();
        assert_eq!(xfs.lookup("dev/initctl"), Some(fifo));
        let rec_fifo = xfs.inodes.get(&fifo).unwrap();
        assert_eq!(rec_fifo.mode & S_IFIFO, S_IFIFO);

        // Invalid mknod kind
        assert_eq!(
            xfs.mknod("bad", FileKind::Regular, 0, 0, &meta),
            Err(Error::Invalid("not a device node or fifo"))
        );
    }

    #[test]
    fn test_hardlink_workflow_and_edge_cases() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta::default();

        xfs.begin_file("original", &meta).unwrap();
        xfs.write_file(b"hello hardlink").unwrap();
        let orig_ino = xfs.end_file().unwrap();

        let link_ino = xfs.hardlink("linked", "original").unwrap();
        assert_eq!(link_ino, orig_ino);
        assert_eq!(xfs.lookup("linked"), Some(orig_ino));
        assert_eq!(xfs.inodes.get(&orig_ino).unwrap().nlink, 2);

        // Hardlink to nonexistent
        assert_eq!(xfs.hardlink("bad", "nonexistent"), Err(Error::NotFound));

        // Hardlink to directory rejected
        xfs.mkdir("somedir", &meta).unwrap();
        assert_eq!(
            xfs.hardlink("dlink", "somedir"),
            Err(Error::Invalid("cannot hardlink a directory"))
        );
    }

    #[test]
    fn test_directory_block_conversion_xdb3() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta::default();

        let dir_ino = xfs.mkdir("manyfiles", &meta).unwrap();

        // Create 35 files inside "manyfiles" to exceed 336 bytes of shortform capacity
        for i in 0..35 {
            let path = alloc::format!("manyfiles/file_{:04}", i);
            xfs.begin_file(&path, &meta).unwrap();
            xfs.write_file(b"x").unwrap();
            xfs.end_file().unwrap();
        }

        // Finish flushes directory conversion to XDB3
        xfs.finish().unwrap();

        let dir_rec = xfs.inodes.get(&dir_ino).unwrap();
        assert_eq!(dir_rec.format, XFS_DINODE_FMT_EXTENTS);
        assert_eq!(dir_rec.size, BLOCK_SIZE);
        assert_eq!(dir_rec.extents.len(), 1);

        let (_, pblk, count, _) = unpack_bmbt_rec(&dir_rec.extents[0]);
        assert_eq!(count, 1);

        // Read the XDB3 block from storage and verify CRC & magic
        let mut blk = [0u8; 4096];
        xfs.storage.read_at(pblk * BLOCK_SIZE, &mut blk).unwrap();
        assert_eq!(&blk[0..4], b"XDB3");
        assert!(xfs_verify_cksum(&blk, DIR3_DATA_CRC_OFF));
    }

    #[test]
    fn test_fswriter_trait_workflow() {
        let size = 16 * 1024 * 1024;
        let storage = MemStorage::new(size);
        let mut xfs = XfsWriter::new(storage, size as u64, test_opts()).unwrap();

        let w: &mut dyn FsWriter = &mut xfs;
        let meta = Meta {
            mode: 0o755,
            uid: 0,
            gid: 0,
            mtime: 12345,
            xattrs: Vec::new(),
        };

        let dir_id = w.mkdir("etc", &meta).unwrap();
        assert_eq!(w.lookup("etc"), Some(dir_id));

        w.begin_file("etc/passwd", &meta).unwrap();
        w.write_file(b"root:x:0:0::/root:/bin/sh\n").unwrap();
        let file_id = w.end_file().unwrap();

        assert_eq!(w.lookup("etc/passwd"), Some(file_id));
        assert_eq!(w.file_size(file_id), 26);

        let mut buf = [0u8; 26];
        let n = w.read_file(file_id, 0, &mut buf).unwrap();
        assert_eq!(n, 26);
        assert_eq!(&buf, b"root:x:0:0::/root:/bin/sh\n");

        let sym_id = w.symlink("etc/os-release", "../usr/lib/os-release", &meta).unwrap();
        assert_eq!(w.lookup("etc/os-release"), Some(sym_id));

        w.finish().unwrap();
    }

    #[test]
    fn test_split_path_cases() {
        assert_eq!(split_path("").unwrap(), Vec::<&str>::new());
        assert_eq!(split_path("/").unwrap(), Vec::<&str>::new());
        assert_eq!(split_path("///").unwrap(), Vec::<&str>::new());
        assert_eq!(split_path("a").unwrap(), alloc::vec!["a"]);
        assert_eq!(split_path("/a/b/c/").unwrap(), alloc::vec!["a", "b", "c"]);
        assert_eq!(split_path("///var///log///arch///").unwrap(), alloc::vec!["var", "log", "arch"]);
    }

    #[test]
    fn test_encode_shortform_dir_direct() {
        let entries = alloc::vec![
            (String::from("file1"), 130u64, FileKind::Regular),
            (String::from("sub"), 131u64, FileKind::Dir),
        ];
        let bytes = encode_shortform_dir(128, &entries);
        assert_eq!(bytes[0], 2); // count = 2
        assert_eq!(bytes[1], 0); // i8count = 0
        assert_eq!(u32::from_be_bytes(bytes[2..6].try_into().unwrap()), 128); // parent

        // Entry 1: namelen 5, offset 96, name "file1", ftype 1, ino 130
        assert_eq!(bytes[6], 5);
        assert_eq!(u16::from_be_bytes(bytes[7..9].try_into().unwrap()), 96);
        assert_eq!(&bytes[9..14], b"file1");
        assert_eq!(bytes[14], XFS_DIR3_FT_REG_FILE);
        assert_eq!(u32::from_be_bytes(bytes[15..19].try_into().unwrap()), 130);
    }

    #[test]
    fn test_encode_block_dir_structure() {
        let entries = alloc::vec![
            (String::from("alpha"), 130u64, FileKind::Regular),
            (String::from("beta"), 131u64, FileKind::Regular),
        ];
        let uuid = [7u8; 16];
        let blk = encode_block_dir(128, 128, 50, &uuid, &entries);

        assert_eq!(&blk[0..4], b"XDB3");
        assert_eq!(u64::from_be_bytes(blk[8..16].try_into().unwrap()), 50 * 8); // sector blkno
        assert_eq!(&blk[24..40], &uuid);
        assert_eq!(u64::from_be_bytes(blk[40..48].try_into().unwrap()), 128); // owner
        assert!(xfs_verify_cksum(&blk, DIR3_DATA_CRC_OFF));

        // Leaf entries count at end of block should be 4 (. + .. + alpha + beta)
        let tail_off = 4096 - 8;
        assert_eq!(u32::from_be_bytes(blk[tail_off..tail_off + 4].try_into().unwrap()), 4);
    }

    #[test]
    fn test_xfs_writer_new_size_too_small() {
        let storage = MemStorage::new(8 * 1024 * 1024);
        let res = XfsWriter::new(storage, 8 * 1024 * 1024, test_opts());
        assert!(matches!(res, Err(Error::Size(_))));
    }

    #[test]
    fn test_alloc_blocks_no_space() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        // Requesting more blocks than available returns NoSpace
        let res = xfs.alloc_blocks(100_000);
        assert_eq!(res, Err(Error::NoSpace));
    }

    #[test]
    fn test_replace_slot_overwrite_directory_error() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta::default();
        xfs.mkdir("sub", &meta).unwrap();
        // Attempting to begin_file on an existing directory path fails
        assert_eq!(
            xfs.begin_file("sub", &meta),
            Err(Error::Invalid("cannot overwrite directory with file"))
        );
    }

    #[test]
    fn test_read_file_non_regular_or_symlink_fails() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta::default();
        let dir_ino = xfs.mkdir("mydir", &meta).unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(
            xfs.read_file(dir_ino, 0, &mut buf),
            Err(Error::Invalid("not a readable regular file or symlink"))
        );
        // Nonexistent file
        assert_eq!(xfs.read_file(9999, 0, &mut buf), Err(Error::NotFound));
    }

    #[test]
    fn test_finish_with_open_file_fails_and_idempotence() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta::default();

        xfs.begin_file("pending", &meta).unwrap();
        assert_eq!(xfs.finish(), Err(Error::Invalid("cannot finish with open file")));
        xfs.end_file().unwrap();

        assert_eq!(xfs.finish(), Ok(()));
        // Idempotent finish call returns Ok(())
        assert_eq!(xfs.finish(), Ok(()));
    }

    #[test]
    fn test_mkdir_component_not_dir() {
        let size = 16 * 1024 * 1024;
        let mut xfs = XfsWriter::new(MemStorage::new(size), size as u64, test_opts()).unwrap();
        let meta = Meta::default();
        xfs.begin_file("file", &meta).unwrap();
        xfs.end_file().unwrap();

        // Attempting mkdir inside a regular file
        assert_eq!(
            xfs.mkdir("file/sub", &meta),
            Err(Error::Invalid("component is not a directory"))
        );
    }
}
