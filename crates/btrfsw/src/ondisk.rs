//! Btrfs on-disk structures, constants, and leaf node packing.

use alloc::vec::Vec;
use crate::crc::btrfs_csum;

pub const BTRFS_MAGIC: &[u8; 8] = b"_BHRfS_M";
pub const BTRFS_MAGIC_U64: u64 = 0x4D5F536648425F; // "_BHRfS_M" little endian

pub const BTRFS_SUPER_INFO_OFFSET: u64 = 65536; // 64 KiB
pub const BTRFS_SUPER_INFO_SIZE: usize = 4096;
pub const BTRFS_NODE_SIZE: usize = 16384; // 16 KiB
pub const BTRFS_SECTOR_SIZE: u32 = 4096;
pub const BTRFS_STRIPE_SIZE: u32 = 65536;

pub const BTRFS_HEADER_FLAG_WRITTEN: u64 = 1 << 0;

// Object IDs
pub const BTRFS_ROOT_TREE_OBJECTID: u64 = 1;
pub const BTRFS_EXTENT_TREE_OBJECTID: u64 = 2;
pub const BTRFS_CHUNK_TREE_OBJECTID: u64 = 3;
pub const BTRFS_DEV_TREE_OBJECTID: u64 = 4;
pub const BTRFS_FS_TREE_OBJECTID: u64 = 5;
pub const BTRFS_ROOT_TREE_DIR_OBJECTID: u64 = 6;
pub const BTRFS_CSUM_TREE_OBJECTID: u64 = 7;
pub const BTRFS_DATA_RELOC_TREE_OBJECTID: u64 = 0xFFFF_FFFF_FFFF_FFF7; // -9ULL
pub const BTRFS_EXTENT_CSUM_OBJECTID: u64 = 0xFFFF_FFFF_FFFF_FFF6; // -10ULL
pub const BTRFS_FIRST_FREE_OBJECTID: u64 = 256;
pub const BTRFS_FIRST_CHUNK_TREE_OBJECTID: u64 = 256;
pub const BTRFS_DEV_ITEMS_OBJECTID: u64 = 1;

// Key types
pub const BTRFS_INODE_ITEM_KEY: u8 = 1;
pub const BTRFS_INODE_REF_KEY: u8 = 12;
pub const BTRFS_DIR_ITEM_KEY: u8 = 84;
pub const BTRFS_DIR_INDEX_KEY: u8 = 96;
pub const BTRFS_EXTENT_DATA_KEY: u8 = 108;
pub const BTRFS_EXTENT_CSUM_KEY: u8 = 128;
pub const BTRFS_ROOT_ITEM_KEY: u8 = 132;
pub const BTRFS_ROOT_BACKREF_KEY: u8 = 144;
pub const BTRFS_ROOT_REF_KEY: u8 = 156;
pub const BTRFS_EXTENT_ITEM_KEY: u8 = 168;
pub const BTRFS_METADATA_ITEM_KEY: u8 = 169;
pub const BTRFS_TREE_BLOCK_REF_KEY: u8 = 176;
pub const BTRFS_EXTENT_DATA_REF_KEY: u8 = 178;
pub const BTRFS_BLOCK_GROUP_ITEM_KEY: u8 = 192;
pub const BTRFS_DEV_EXTENT_KEY: u8 = 204;
pub const BTRFS_DEV_ITEM_KEY: u8 = 216;
pub const BTRFS_CHUNK_ITEM_KEY: u8 = 228;

// Block group / chunk types
pub const BTRFS_BLOCK_GROUP_DATA: u64 = 1 << 0;
pub const BTRFS_BLOCK_GROUP_SYSTEM: u64 = 1 << 1;
pub const BTRFS_BLOCK_GROUP_METADATA: u64 = 1 << 2;

// File extent types
pub const BTRFS_FILE_EXTENT_INLINE: u8 = 0;
pub const BTRFS_FILE_EXTENT_REG: u8 = 1;

// Extent item flags
pub const BTRFS_EXTENT_FLAG_DATA: u64 = 1 << 0;
pub const BTRFS_EXTENT_FLAG_TREE_BLOCK: u64 = 1 << 1;
pub const BTRFS_BLOCK_FLAG_FULL_BACKREF: u64 = 1 << 8;

// Directory entry file types
pub const BTRFS_FT_REG_FILE: u8 = 1;
pub const BTRFS_FT_DIR: u8 = 2;
pub const BTRFS_FT_CHRDEV: u8 = 3;
pub const BTRFS_FT_BLKDEV: u8 = 4;
pub const BTRFS_FT_FIFO: u8 = 5;
pub const BTRFS_FT_SOCK: u8 = 6;
pub const BTRFS_FT_SYMLINK: u8 = 7;

// Incompat feature flags
pub const BTRFS_FEATURE_INCOMPAT_MIXED_BACKREF: u64 = 1 << 0;
pub const BTRFS_FEATURE_INCOMPAT_DEFAULT_SUBVOL: u64 = 1 << 1;
pub const BTRFS_FEATURE_INCOMPAT_BIG_METADATA: u64 = 1 << 3;
pub const BTRFS_FEATURE_INCOMPAT_EXTENDED_IREF: u64 = 1 << 6;
pub const BTRFS_FEATURE_INCOMPAT_SKINNY_METADATA: u64 = 1 << 8;

pub const DEFAULT_INCOMPAT_FLAGS: u64 = BTRFS_FEATURE_INCOMPAT_MIXED_BACKREF
    | BTRFS_FEATURE_INCOMPAT_DEFAULT_SUBVOL
    | BTRFS_FEATURE_INCOMPAT_BIG_METADATA
    | BTRFS_FEATURE_INCOMPAT_EXTENDED_IREF
    | BTRFS_FEATURE_INCOMPAT_SKINNY_METADATA;

pub const BTRFS_HEADER_SIZE: usize = 101;
pub const BTRFS_ITEM_SIZE: usize = 25;
pub const BTRFS_LEAF_DATA_OFFSET: usize = BTRFS_HEADER_SIZE; // 101

/// 17-byte Btrfs disk key: (objectid, type, offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiskKey {
    pub objectid: u64,
    pub key_type: u8,
    pub offset: u64,
}

impl DiskKey {
    pub const fn new(objectid: u64, key_type: u8, offset: u64) -> Self {
        Self {
            objectid,
            key_type,
            offset,
        }
    }

    pub fn to_bytes(&self) -> [u8; 17] {
        let mut b = [0u8; 17];
        b[0..8].copy_from_slice(&self.objectid.to_le_bytes());
        b[8] = self.key_type;
        b[9..17].copy_from_slice(&self.offset.to_le_bytes());
        b
    }

    pub fn from_bytes(bytes: &[u8; 17]) -> Self {
        let mut obj = [0u8; 8];
        obj.copy_from_slice(&bytes[0..8]);
        let objectid = u64::from_le_bytes(obj);
        let key_type = bytes[8];
        let mut off = [0u8; 8];
        off.copy_from_slice(&bytes[9..17]);
        let offset = u64::from_le_bytes(off);
        Self {
            objectid,
            key_type,
            offset,
        }
    }
}

/// 101-byte Btrfs tree node header.
#[derive(Debug, Clone)]
pub struct BtrfsHeader {
    pub csum: [u8; 32],
    pub fsid: [u8; 16],
    pub bytenr: u64,
    pub flags: u64,
    pub chunk_tree_uuid: [u8; 16],
    pub generation: u64,
    pub owner: u64,
    pub nritems: u32,
    pub level: u8,
}

impl BtrfsHeader {
    pub fn new(bytenr: u64, fsid: [u8; 16], generation: u64, owner: u64, level: u8) -> Self {
        Self {
            csum: [0u8; 32],
            fsid,
            bytenr,
            flags: BTRFS_HEADER_FLAG_WRITTEN,
            chunk_tree_uuid: fsid,
            generation,
            owner,
            nritems: 0,
            level,
        }
    }

    pub fn write_into(&self, buf: &mut [u8]) {
        assert!(buf.len() >= BTRFS_HEADER_SIZE);
        buf[0..32].copy_from_slice(&self.csum);
        buf[32..48].copy_from_slice(&self.fsid);
        buf[48..56].copy_from_slice(&self.bytenr.to_le_bytes());
        buf[56..64].copy_from_slice(&self.flags.to_le_bytes());
        buf[64..80].copy_from_slice(&self.chunk_tree_uuid);
        buf[80..88].copy_from_slice(&self.generation.to_le_bytes());
        buf[88..96].copy_from_slice(&self.owner.to_le_bytes());
        buf[96..100].copy_from_slice(&self.nritems.to_le_bytes());
        buf[100] = self.level;
    }
}

/// An entry in a leaf node before layout packing.
#[derive(Debug, Clone)]
pub struct LeafItem {
    pub key: DiskKey,
    pub data: Vec<u8>,
}

/// A 16 KiB B-tree leaf node (level 0).
#[derive(Debug, Clone)]
pub struct LeafNode {
    pub header: BtrfsHeader,
    pub items: Vec<LeafItem>,
}

impl LeafNode {
    pub fn new(bytenr: u64, fsid: [u8; 16], generation: u64, owner: u64) -> Self {
        Self {
            header: BtrfsHeader::new(bytenr, fsid, generation, owner, 0),
            items: Vec::new(),
        }
    }

    /// Remaining bytes in leaf node for new items and payloads.
    pub fn free_space(&self) -> usize {
        let total_item_headers = self.items.len() * BTRFS_ITEM_SIZE;
        let total_payloads: usize = self.items.iter().map(|it| it.data.len()).sum();
        let used = BTRFS_HEADER_SIZE + total_item_headers + total_payloads;
        BTRFS_NODE_SIZE.saturating_sub(used)
    }

    /// Inserts an item maintaining key sort order.
    pub fn insert(&mut self, key: DiskKey, data: Vec<u8>) -> bool {
        let need = BTRFS_ITEM_SIZE + data.len();
        if self.free_space() < need {
            return false;
        }
        let pos = match self.items.binary_search_by_key(&key, |it| it.key) {
            Ok(idx) => {
                // Key already exists, replace data if space allows
                let old_len = self.items[idx].data.len();
                if self.free_space().saturating_add(old_len) < data.len() {
                    return false;
                }
                self.items[idx].data = data;
                return true;
            }
            Err(pos) => pos,
        };
        self.items.insert(pos, LeafItem { key, data });
        true
    }

    /// Serializes this leaf node into a 16 KiB buffer with header and calculated CRC32C.
    pub fn to_bytes(&self) -> [u8; BTRFS_NODE_SIZE] {
        let mut buf = [0u8; BTRFS_NODE_SIZE];
        let mut hdr = self.header.clone();
        hdr.nritems = self.items.len() as u32;
        hdr.write_into(&mut buf);

        // Data payload grows backwards from end of node (relative to BTRFS_LEAF_DATA_OFFSET = 101).
        // That means the maximum payload offset is (16384 - 101) = 16283.
        let usable_data_len = BTRFS_NODE_SIZE - BTRFS_LEAF_DATA_OFFSET;
        let mut current_offset_from_end = 0usize;

        for (slot, item) in self.items.iter().enumerate() {
            let item_len = item.data.len();
            current_offset_from_end += item_len;
            let item_offset_in_leaf_data = usable_data_len - current_offset_from_end;

            // 1. Write btrfs_item struct at offset 101 + slot * 25
            let item_hdr_off = BTRFS_LEAF_DATA_OFFSET + slot * BTRFS_ITEM_SIZE;
            buf[item_hdr_off..item_hdr_off + 17].copy_from_slice(&item.key.to_bytes());
            buf[item_hdr_off + 17..item_hdr_off + 21]
                .copy_from_slice(&(item_offset_in_leaf_data as u32).to_le_bytes());
            buf[item_hdr_off + 21..item_hdr_off + 25]
                .copy_from_slice(&(item_len as u32).to_le_bytes());

            // 2. Write payload at BTRFS_LEAF_DATA_OFFSET + item_offset_in_leaf_data
            let data_pos = BTRFS_LEAF_DATA_OFFSET + item_offset_in_leaf_data;
            buf[data_pos..data_pos + item_len].copy_from_slice(&item.data);
        }

        // 3. Compute CRC32C over bytes 32..16384 and write into buf[0..4]
        let csum = btrfs_csum(&buf[32..BTRFS_NODE_SIZE]);
        buf[0..4].copy_from_slice(&csum.to_le_bytes());

        buf
    }
}

/// Serializers for various Btrfs items.
pub mod items {
    use super::*;

    /// Serializes an INODE_ITEM (160 bytes).
    #[expect(clippy::too_many_arguments, reason = "Btrfs inode item layout specifies 16 on-disk fields")]
    pub fn serialize_inode_item(
        generation: u64,
        transid: u64,
        size: u64,
        nbytes: u64,
        block_group: u64,
        nlink: u32,
        uid: u32,
        gid: u32,
        mode: u32,
        rdev: u64,
        flags: u64,
        sequence: u64,
        atime_sec: u64,
        mtime_sec: u64,
        ctime_sec: u64,
        otime_sec: u64,
    ) -> [u8; 160] {
        let mut b = [0u8; 160];
        b[0..8].copy_from_slice(&generation.to_le_bytes());
        b[8..16].copy_from_slice(&transid.to_le_bytes());
        b[16..24].copy_from_slice(&size.to_le_bytes());
        b[24..32].copy_from_slice(&nbytes.to_le_bytes());
        b[32..40].copy_from_slice(&block_group.to_le_bytes());
        b[40..44].copy_from_slice(&nlink.to_le_bytes());
        b[44..48].copy_from_slice(&uid.to_le_bytes());
        b[48..52].copy_from_slice(&gid.to_le_bytes());
        b[52..56].copy_from_slice(&mode.to_le_bytes());
        b[56..64].copy_from_slice(&rdev.to_le_bytes());
        b[64..72].copy_from_slice(&flags.to_le_bytes());
        b[72..80].copy_from_slice(&sequence.to_le_bytes());
        // reserved[4] = 80..112 (zeroed)
        // atime: sec (8), nsec (4)
        b[112..120].copy_from_slice(&atime_sec.to_le_bytes());
        // ctime
        b[124..132].copy_from_slice(&ctime_sec.to_le_bytes());
        // mtime
        b[136..144].copy_from_slice(&mtime_sec.to_le_bytes());
        // otime
        b[148..156].copy_from_slice(&otime_sec.to_le_bytes());
        b
    }

    /// Serializes an INODE_REF item (10 + name.len() bytes).
    pub fn serialize_inode_ref(index: u64, name: &str) -> Vec<u8> {
        let mut v = Vec::with_capacity(10 + name.len());
        v.extend_from_slice(&index.to_le_bytes());
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(name.as_bytes());
        v
    }

    /// Serializes a DIR_ITEM or DIR_INDEX item (30 + name.len() bytes).
    pub fn serialize_dir_item(location: DiskKey, transid: u64, file_type: u8, name: &str) -> Vec<u8> {
        let mut v = Vec::with_capacity(30 + name.len());
        v.extend_from_slice(&location.to_bytes());
        v.extend_from_slice(&transid.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes()); // data_len = 0
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.push(file_type);
        v.extend_from_slice(name.as_bytes());
        v
    }

    /// Serializes an inline EXTENT_DATA item (21 + data.len() bytes).
    pub fn serialize_file_extent_inline(generation: u64, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(21 + data.len());
        v.extend_from_slice(&generation.to_le_bytes());
        v.extend_from_slice(&(data.len() as u64).to_le_bytes()); // ram_bytes
        v.push(0); // compression
        v.push(0); // encryption
        v.extend_from_slice(&0u16.to_le_bytes()); // other_encoding
        v.push(BTRFS_FILE_EXTENT_INLINE); // type = 0
        v.extend_from_slice(data);
        v
    }

    /// Serializes a regular EXTENT_DATA item (53 bytes).
    pub fn serialize_file_extent_reg(
        generation: u64,
        ram_bytes: u64,
        disk_bytenr: u64,
        disk_num_bytes: u64,
        offset: u64,
        num_bytes: u64,
    ) -> [u8; 53] {
        let mut b = [0u8; 53];
        b[0..8].copy_from_slice(&generation.to_le_bytes());
        b[8..16].copy_from_slice(&ram_bytes.to_le_bytes());
        b[16] = 0; // compression
        b[17] = 0; // encryption
        b[18..20].copy_from_slice(&0u16.to_le_bytes());
        b[20] = BTRFS_FILE_EXTENT_REG; // type = 1
        b[21..29].copy_from_slice(&disk_bytenr.to_le_bytes());
        b[29..37].copy_from_slice(&disk_num_bytes.to_le_bytes());
        b[37..45].copy_from_slice(&offset.to_le_bytes());
        b[45..53].copy_from_slice(&num_bytes.to_le_bytes());
        b
    }

    /// Serializes a ROOT_ITEM (439 bytes).
    pub fn serialize_root_item(
        generation: u64,
        root_dirid: u64,
        bytenr: u64,
        bytes_used: u64,
        level: u8,
        uuid: [u8; 16],
    ) -> [u8; 439] {
        let mut b = [0u8; 439];
        // 0..160: embedded inode item
        let inode = serialize_inode_item(
            generation,
            generation,
            3,
            0,
            0,
            1,
            0,
            0,
            0o040755, // S_IFDIR | 0755
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        );
        b[0..160].copy_from_slice(&inode);
        b[160..168].copy_from_slice(&generation.to_le_bytes());
        b[168..176].copy_from_slice(&root_dirid.to_le_bytes());
        b[176..184].copy_from_slice(&bytenr.to_le_bytes());
        b[184..192].copy_from_slice(&0u64.to_le_bytes()); // byte_limit
        b[192..200].copy_from_slice(&bytes_used.to_le_bytes());
        b[200..208].copy_from_slice(&0u64.to_le_bytes()); // last_snapshot
        b[208..216].copy_from_slice(&0u64.to_le_bytes()); // flags
        b[216..220].copy_from_slice(&1u32.to_le_bytes()); // refs = 1 (u32)
        // drop_progress (17 bytes: 220..237), drop_level (1 byte: 237)
        b[238] = level;
        b[239..247].copy_from_slice(&generation.to_le_bytes()); // generation_v2
        b[247..263].copy_from_slice(&uuid);
        b
    }

    /// Serializes a ROOT_REF or ROOT_BACKREF item (18 + name.len() bytes).
    pub fn serialize_root_ref(dirid: u64, sequence: u64, name: &str) -> Vec<u8> {
        let mut v = Vec::with_capacity(18 + name.len());
        v.extend_from_slice(&dirid.to_le_bytes());
        v.extend_from_slice(&sequence.to_le_bytes());
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(name.as_bytes());
        v
    }

    /// Serializes a DEV_ITEM (98 bytes).
    pub fn serialize_dev_item(
        devid: u64,
        total_bytes: u64,
        bytes_used: u64,
        uuid: [u8; 16],
        fsid: [u8; 16],
    ) -> [u8; 98] {
        let mut b = [0u8; 98];
        b[0..8].copy_from_slice(&devid.to_le_bytes());
        b[8..16].copy_from_slice(&total_bytes.to_le_bytes());
        b[16..24].copy_from_slice(&bytes_used.to_le_bytes());
        b[24..28].copy_from_slice(&BTRFS_SECTOR_SIZE.to_le_bytes()); // io_align
        b[28..32].copy_from_slice(&BTRFS_SECTOR_SIZE.to_le_bytes()); // io_width
        b[32..36].copy_from_slice(&BTRFS_SECTOR_SIZE.to_le_bytes()); // sector_size
        b[36..44].copy_from_slice(&0u64.to_le_bytes()); // type
        b[44..52].copy_from_slice(&0u64.to_le_bytes()); // generation
        b[52..60].copy_from_slice(&0u64.to_le_bytes()); // start_offset
        b[60..64].copy_from_slice(&0u32.to_le_bytes()); // dev_group
        b[64] = 0; // seek_speed
        b[65] = 0; // bandwidth
        b[66..82].copy_from_slice(&uuid);
        b[82..98].copy_from_slice(&fsid);
        b
    }

    /// Serializes a CHUNK_ITEM with 1 STRIPE (80 bytes).
    pub fn serialize_chunk_item(
        length: u64,
        owner: u64,
        chunk_type: u64,
        stripe_physical: u64,
        dev_uuid: [u8; 16],
    ) -> [u8; 80] {
        let mut b = [0u8; 80];
        b[0..8].copy_from_slice(&length.to_le_bytes());
        b[8..16].copy_from_slice(&owner.to_le_bytes());
        b[16..24].copy_from_slice(&65536u64.to_le_bytes()); // stripe_len
        b[24..32].copy_from_slice(&chunk_type.to_le_bytes());
        b[32..36].copy_from_slice(&BTRFS_SECTOR_SIZE.to_le_bytes()); // io_align
        b[36..40].copy_from_slice(&BTRFS_SECTOR_SIZE.to_le_bytes()); // io_width
        b[40..44].copy_from_slice(&BTRFS_SECTOR_SIZE.to_le_bytes()); // sector_size
        b[44..46].copy_from_slice(&1u16.to_le_bytes()); // num_stripes = 1
        b[46..48].copy_from_slice(&0u16.to_le_bytes()); // sub_stripes = 0

        // stripe 0 at 48..80
        b[48..56].copy_from_slice(&1u64.to_le_bytes()); // devid = 1
        b[56..64].copy_from_slice(&stripe_physical.to_le_bytes());
        b[64..80].copy_from_slice(&dev_uuid);
        b
    }

    /// Serializes an EXTENT_ITEM with a tree block ref (33 bytes) or data ref (45 bytes).
    pub fn serialize_extent_item_tree_block(generation: u64, root_id: u64) -> [u8; 33] {
        let mut b = [0u8; 33];
        b[0..8].copy_from_slice(&1u64.to_le_bytes()); // refs = 1
        b[8..16].copy_from_slice(&generation.to_le_bytes());
        let flags = BTRFS_EXTENT_FLAG_TREE_BLOCK;
        b[16..24].copy_from_slice(&flags.to_le_bytes());
        // inline ref: type (1 byte) + offset (8 bytes)
        b[24] = BTRFS_TREE_BLOCK_REF_KEY; // 176
        b[25..33].copy_from_slice(&root_id.to_le_bytes());
        b
    }

    pub fn serialize_extent_item_data(
        generation: u64,
        root_id: u64,
        objectid: u64,
        offset: u64,
    ) -> [u8; 53] {
        let mut b = [0u8; 53];
        b[0..8].copy_from_slice(&1u64.to_le_bytes()); // refs = 1
        b[8..16].copy_from_slice(&generation.to_le_bytes());
        b[16..24].copy_from_slice(&BTRFS_EXTENT_FLAG_DATA.to_le_bytes());
        // inline ref: type (1 byte = 178) + struct btrfs_extent_data_ref (root 8, obj 8, off 8, count 4)
        b[24] = BTRFS_EXTENT_DATA_REF_KEY; // 178
        b[25..33].copy_from_slice(&root_id.to_le_bytes());
        b[33..41].copy_from_slice(&objectid.to_le_bytes());
        b[41..49].copy_from_slice(&offset.to_le_bytes());
        b[49..53].copy_from_slice(&1u32.to_le_bytes()); // count = 1
        b
    }

    /// Serializes a BLOCK_GROUP_ITEM (24 bytes).
    pub fn serialize_block_group_item(used: u64, chunk_type: u64, flags: u64) -> [u8; 24] {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&used.to_le_bytes());
        b[8..16].copy_from_slice(&BTRFS_FIRST_CHUNK_TREE_OBJECTID.to_le_bytes());
        b[16..24].copy_from_slice(&(chunk_type | flags).to_le_bytes());
        b
    }

    /// Serializes a DEV_EXTENT (48 bytes).
    pub fn serialize_dev_extent(
        chunk_offset: u64,
        length: u64,
        chunk_tree_uuid: [u8; 16],
    ) -> [u8; 48] {
        let mut b = [0u8; 48];
        b[0..8].copy_from_slice(&BTRFS_CHUNK_TREE_OBJECTID.to_le_bytes());
        b[8..16].copy_from_slice(&BTRFS_FIRST_CHUNK_TREE_OBJECTID.to_le_bytes());
        b[16..24].copy_from_slice(&chunk_offset.to_le_bytes());
        b[24..32].copy_from_slice(&length.to_le_bytes());
        b[32..48].copy_from_slice(&chunk_tree_uuid);
        b
    }
}

/// 4096-byte Btrfs Superblock.
#[derive(Debug, Clone)]
pub struct SuperBlock {
    pub fsid: [u8; 16],
    pub bytenr: u64,
    pub flags: u64,
    pub generation: u64,
    pub root: u64,
    pub chunk_root: u64,
    pub total_bytes: u64,
    pub bytes_used: u64,
    pub sectorsize: u32,
    pub nodesize: u32,
    pub leafsize: u32,
    pub stripesize: u32,
    pub sys_chunk_array_size: u32,
    pub chunk_root_generation: u64,
    pub incompat_flags: u64,
    pub dev_item: [u8; 98],
    pub label: [u8; 256],
    pub sys_chunk_array: [u8; 2048],
}

impl SuperBlock {
    pub fn new(fsid: [u8; 16], total_bytes: u64, root: u64, chunk_root: u64) -> Self {
        let dev_item = items::serialize_dev_item(1, total_bytes, 0, fsid, fsid);
        Self {
            fsid,
            bytenr: BTRFS_SUPER_INFO_OFFSET,
            flags: 0,
            generation: 1,
            root,
            chunk_root,
            total_bytes,
            bytes_used: 0,
            sectorsize: BTRFS_SECTOR_SIZE,
            nodesize: BTRFS_NODE_SIZE as u32,
            leafsize: BTRFS_NODE_SIZE as u32,
            stripesize: BTRFS_STRIPE_SIZE,
            sys_chunk_array_size: 0,
            chunk_root_generation: 1,
            incompat_flags: DEFAULT_INCOMPAT_FLAGS,
            dev_item,
            label: [0u8; 256],
            sys_chunk_array: [0u8; 2048],
        }
    }

    pub fn set_label(&mut self, label_str: &str) {
        self.label = [0u8; 256];
        let bytes = label_str.as_bytes();
        let len = bytes.len().min(255);
        self.label[..len].copy_from_slice(&bytes[..len]);
    }

    pub fn add_sys_chunk(&mut self, logical: u64, length: u64, chunk_type: u64) {
        let key = DiskKey::new(BTRFS_FIRST_CHUNK_TREE_OBJECTID, BTRFS_CHUNK_ITEM_KEY, logical);
        let chunk_data = items::serialize_chunk_item(length, BTRFS_EXTENT_TREE_OBJECTID, chunk_type, logical, self.fsid);
        let off = self.sys_chunk_array_size as usize;
        assert!(off + 17 + 80 <= 2048, "sys_chunk_array overflow");
        self.sys_chunk_array[off..off + 17].copy_from_slice(&key.to_bytes());
        self.sys_chunk_array[off + 17..off + 97].copy_from_slice(&chunk_data);
        self.sys_chunk_array_size += 97;
    }

    /// Serializes superblock into 4096-byte array with CRC32C checksum at offset 0.
    pub fn to_bytes(&self) -> [u8; BTRFS_SUPER_INFO_SIZE] {
        let mut buf = [0u8; BTRFS_SUPER_INFO_SIZE];

        // 0..32: csum (written at end)
        // 32..48: fsid
        buf[32..48].copy_from_slice(&self.fsid);
        // 48..56: bytenr
        buf[48..56].copy_from_slice(&self.bytenr.to_le_bytes());
        // 56..64: flags
        buf[56..64].copy_from_slice(&self.flags.to_le_bytes());
        // 64..72: magic
        buf[64..72].copy_from_slice(BTRFS_MAGIC);
        // 72..80: generation
        buf[72..80].copy_from_slice(&self.generation.to_le_bytes());
        // 80..88: root
        buf[80..88].copy_from_slice(&self.root.to_le_bytes());
        // 88..96: chunk_root
        buf[88..96].copy_from_slice(&self.chunk_root.to_le_bytes());
        // 96..104: log_root (0)
        // 104..112: log_root_transid (0)
        // 112..120: total_bytes
        buf[112..120].copy_from_slice(&self.total_bytes.to_le_bytes());
        // 120..128: bytes_used
        buf[120..128].copy_from_slice(&self.bytes_used.to_le_bytes());
        // 128..136: root_dir_objectid (6)
        buf[128..136].copy_from_slice(&BTRFS_ROOT_TREE_DIR_OBJECTID.to_le_bytes());
        // 136..144: num_devices (1)
        buf[136..144].copy_from_slice(&1u64.to_le_bytes());
        // 144..148: sectorsize
        buf[144..148].copy_from_slice(&self.sectorsize.to_le_bytes());
        // 148..152: nodesize
        buf[148..152].copy_from_slice(&self.nodesize.to_le_bytes());
        // 152..156: leafsize
        buf[152..156].copy_from_slice(&self.leafsize.to_le_bytes());
        // 156..160: stripesize
        buf[156..160].copy_from_slice(&self.stripesize.to_le_bytes());
        // 160..164: sys_chunk_array_size
        buf[160..164].copy_from_slice(&self.sys_chunk_array_size.to_le_bytes());
        // 164..172: chunk_root_generation
        buf[164..172].copy_from_slice(&self.chunk_root_generation.to_le_bytes());
        // 172..180: compat_flags (0)
        // 180..188: compat_ro_flags (0)
        // 188..196: incompat_flags
        buf[188..196].copy_from_slice(&self.incompat_flags.to_le_bytes());
        // 196..198: csum_type (0 = CRC32C)
        buf[196..198].copy_from_slice(&0u16.to_le_bytes());
        // 198: root_level (0)
        // 199: chunk_root_level (0)
        // 200: log_root_level (0)
        // 201..299: dev_item (98 bytes)
        buf[201..299].copy_from_slice(&self.dev_item);
        // 299..555: label (256 bytes)
        buf[299..555].copy_from_slice(&self.label);
        // 555..563: cache_generation (0)
        // 563..571: uuid_tree_generation (0)
        // 571..587: metadata_uuid (16 bytes = fsid)
        buf[571..587].copy_from_slice(&self.fsid);
        // 587..595: nr_global_roots (0)
        // 595..603: remap_root (0)
        // 603..611: remap_root_generation (0)
        // 611: remap_root_level (0)
        // 612..811: reserved[199] (zeroed)
        // 811..2859: sys_chunk_array (2048 bytes)
        buf[811..2859].copy_from_slice(&self.sys_chunk_array);
        // 2859..3531: super_roots (backup roots, zeroed)
        // 3531..4096: padding (zeroed)

        // Compute CRC32C over bytes 32..4096
        let csum = btrfs_csum(&buf[32..4096]);
        buf[0..4].copy_from_slice(&csum.to_le_bytes());

        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_key_ordering() {
        let k1 = DiskKey::new(100, 1, 0);
        let k2 = DiskKey::new(100, 1, 5);
        let k3 = DiskKey::new(100, 2, 0);
        let k4 = DiskKey::new(101, 1, 0);

        assert!(k1 < k2);
        assert!(k2 < k3);
        assert!(k3 < k4);
    }

    #[test]
    fn test_disk_key_roundtrip() {
        let key = DiskKey::new(0x1234_5678_9ABC_DEF0, 108, 0x0FED_CBA9_8765_4321);
        let bytes = key.to_bytes();
        let parsed = DiskKey::from_bytes(&bytes);
        assert_eq!(key, parsed);
    }

    #[test]
    fn test_leaf_node_insert_and_sort() {
        let fsid = [42u8; 16];
        let mut leaf = LeafNode::new(0x100000, fsid, 1, 1);

        let k3 = DiskKey::new(256, BTRFS_INODE_REF_KEY, 256);
        let k1 = DiskKey::new(256, BTRFS_INODE_ITEM_KEY, 0);
        let k2 = DiskKey::new(256, BTRFS_DIR_ITEM_KEY, 12345);

        assert!(leaf.insert(k3, alloc::vec![3, 3, 3]));
        assert!(leaf.insert(k1, alloc::vec![1, 1, 1, 1]));
        assert!(leaf.insert(k2, alloc::vec![2, 2]));

        // Check that items are sorted in key order
        assert_eq!(leaf.items[0].key, k1);
        assert_eq!(leaf.items[1].key, k3); // INODE_REF_KEY = 12
        assert_eq!(leaf.items[2].key, k2); // DIR_ITEM_KEY = 84
    }

    #[test]
    fn test_leaf_node_serialization_and_csum() {
        let fsid = [7u8; 16];
        let mut leaf = LeafNode::new(0x200000, fsid, 1, BTRFS_FS_TREE_OBJECTID);

        let k = DiskKey::new(256, BTRFS_INODE_ITEM_KEY, 0);
        let inode_bytes = items::serialize_inode_item(
            1, 1, 0, 0, 0, 1, 0, 0, 0o040755, 0, 0, 0, 1000, 1000, 1000, 1000,
        );
        leaf.insert(k, inode_bytes.to_vec());

        let raw = leaf.to_bytes();
        assert_eq!(raw.len(), BTRFS_NODE_SIZE);

        // Header verification
        let csum = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        let expected_csum = btrfs_csum(&raw[32..BTRFS_NODE_SIZE]);
        assert_eq!(csum, expected_csum);

        assert_eq!(&raw[32..48], &fsid);
        let bytenr = u64::from_le_bytes(raw[48..56].try_into().unwrap());
        assert_eq!(bytenr, 0x200000);
        let owner = u64::from_le_bytes(raw[88..96].try_into().unwrap());
        assert_eq!(owner, BTRFS_FS_TREE_OBJECTID);
        let nritems = u32::from_le_bytes(raw[96..100].try_into().unwrap());
        assert_eq!(nritems, 1);
        assert_eq!(raw[100], 0); // level 0

        // Item 0 header
        let item_key = DiskKey::from_bytes(raw[101..118].try_into().unwrap());
        assert_eq!(item_key, k);
        let item_offset = u32::from_le_bytes(raw[118..122].try_into().unwrap());
        let item_size = u32::from_le_bytes(raw[122..126].try_into().unwrap());
        assert_eq!(item_size, 160);

        // Data payload location
        let payload_pos = BTRFS_LEAF_DATA_OFFSET + item_offset as usize;
        assert_eq!(&raw[payload_pos..payload_pos + 160], &inode_bytes[..]);
    }

    #[test]
    fn test_superblock_layout_offsets() {
        let fsid = [0x55u8; 16];
        let mut sb = SuperBlock::new(fsid, 100 * 1024 * 1024, 0x100000, 0x200000);
        sb.set_label("archroot");
        sb.add_sys_chunk(0x100000, 4 * 1024 * 1024, BTRFS_BLOCK_GROUP_SYSTEM);

        let bytes = sb.to_bytes();
        assert_eq!(bytes.len(), BTRFS_SUPER_INFO_SIZE);

        // Checksum matches
        let csum = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let expected_csum = btrfs_csum(&bytes[32..4096]);
        assert_eq!(csum, expected_csum);

        // Magic at 64 (0x40)
        assert_eq!(&bytes[64..72], b"_BHRfS_M");

        // UUID at 32 (0x20)
        assert_eq!(&bytes[32..48], &fsid);

        // bytenr at 48 (0x30)
        assert_eq!(u64::from_le_bytes(bytes[48..56].try_into().unwrap()), 65536);

        // root at 80 (0x50), chunk_root at 88 (0x58)
        assert_eq!(u64::from_le_bytes(bytes[80..88].try_into().unwrap()), 0x100000);
        assert_eq!(u64::from_le_bytes(bytes[88..96].try_into().unwrap()), 0x200000);

        // total_bytes at 112 (0x70)
        assert_eq!(u64::from_le_bytes(bytes[112..120].try_into().unwrap()), 100 * 1024 * 1024);

        // sectorsize at 144 (0x90), nodesize at 148 (0x94)
        assert_eq!(u32::from_le_bytes(bytes[144..148].try_into().unwrap()), 4096);
        assert_eq!(u32::from_le_bytes(bytes[148..152].try_into().unwrap()), 16384);

        // sys_chunk_array_size at 160 (0xa0)
        assert_eq!(u32::from_le_bytes(bytes[160..164].try_into().unwrap()), 97);

        // dev_item at 201 (0xc9)
        assert_eq!(u64::from_le_bytes(bytes[201..209].try_into().unwrap()), 1); // devid = 1

        // label at 299 (0x12b)
        assert_eq!(&bytes[299..307], b"archroot");
        assert_eq!(bytes[307], 0);

        // sys_chunk_array at 811 (0x32b)
        let chunk_key = DiskKey::from_bytes(bytes[811..828].try_into().unwrap());
        assert_eq!(chunk_key.objectid, BTRFS_FIRST_CHUNK_TREE_OBJECTID);
        assert_eq!(chunk_key.key_type, BTRFS_CHUNK_ITEM_KEY);
        assert_eq!(chunk_key.offset, 0x100000);
    }

    #[test]
    fn test_dir_item_serialization() {
        let loc = DiskKey::new(257, BTRFS_INODE_ITEM_KEY, 0);
        let raw = items::serialize_dir_item(loc, 1, BTRFS_FT_REG_FILE, "passwd");
        assert_eq!(raw.len(), 30 + 6);
        assert_eq!(&raw[30..36], b"passwd");
        assert_eq!(raw[29], BTRFS_FT_REG_FILE);
    }

    #[test]
    fn test_inline_extent_serialization() {
        let data = b"hello btrfs!";
        let raw = items::serialize_file_extent_inline(1, data);
        assert_eq!(raw.len(), 21 + 12);
        assert_eq!(raw[20], BTRFS_FILE_EXTENT_INLINE);
        assert_eq!(&raw[21..33], data);
    }

    #[test]
    fn test_leaf_node_free_space() {
        let fsid = [1u8; 16];
        let mut leaf = LeafNode::new(0x100000, fsid, 1, 1);
        let initial_free = leaf.free_space();
        assert_eq!(initial_free, BTRFS_NODE_SIZE - BTRFS_HEADER_SIZE); // 16384 - 101 = 16283

        let k = DiskKey::new(256, BTRFS_INODE_ITEM_KEY, 0);
        let payload = alloc::vec![0u8; 100];
        assert!(leaf.insert(k, payload));
        // Used: 101 (hdr) + 25 (item) + 100 (payload) = 226
        assert_eq!(leaf.free_space(), BTRFS_NODE_SIZE - 226);
    }

    #[test]
    fn test_inode_ref_serialization() {
        let raw = items::serialize_inode_ref(2, "hostname");
        assert_eq!(raw.len(), 10 + 8);
        let idx = u64::from_le_bytes(raw[0..8].try_into().unwrap());
        assert_eq!(idx, 2);
        let name_len = u16::from_le_bytes(raw[8..10].try_into().unwrap());
        assert_eq!(name_len, 8);
        assert_eq!(&raw[10..18], b"hostname");
    }

    #[test]
    fn test_file_extent_reg_serialization() {
        let raw = items::serialize_file_extent_reg(1, 10000, 0x1500000, 12288, 0, 10000);
        assert_eq!(raw.len(), 53);
        assert_eq!(raw[20], BTRFS_FILE_EXTENT_REG);
        let disk_bytenr = u64::from_le_bytes(raw[21..29].try_into().unwrap());
        assert_eq!(disk_bytenr, 0x1500000);
        let disk_num_bytes = u64::from_le_bytes(raw[29..37].try_into().unwrap());
        assert_eq!(disk_num_bytes, 12288);
        let num_bytes = u64::from_le_bytes(raw[45..53].try_into().unwrap());
        assert_eq!(num_bytes, 10000);
    }

    #[test]
    fn test_root_item_serialization() {
        let uuid = [9u8; 16];
        let raw = items::serialize_root_item(1, 256, 0x500000, 16384, 0, uuid);
        assert_eq!(raw.len(), 439);
        let root_dirid = u64::from_le_bytes(raw[168..176].try_into().unwrap());
        assert_eq!(root_dirid, 256);
        let bytenr = u64::from_le_bytes(raw[176..184].try_into().unwrap());
        assert_eq!(bytenr, 0x500000);
        let gen_v2 = u64::from_le_bytes(raw[239..247].try_into().unwrap());
        assert_eq!(gen_v2, 1);
        assert_eq!(&raw[247..263], &uuid);
    }

    #[test]
    fn test_root_ref_serialization() {
        let raw = items::serialize_root_ref(256, 2, "@home");
        assert_eq!(raw.len(), 18 + 5);
        let dirid = u64::from_le_bytes(raw[0..8].try_into().unwrap());
        assert_eq!(dirid, 256);
        let seq = u64::from_le_bytes(raw[8..16].try_into().unwrap());
        assert_eq!(seq, 2);
        let name_len = u16::from_le_bytes(raw[16..18].try_into().unwrap());
        assert_eq!(name_len, 5);
        assert_eq!(&raw[18..23], b"@home");
    }

    #[test]
    fn test_dev_item_serialization() {
        let uuid = [0xaau8; 16];
        let fsid = [0xbbu8; 16];
        let raw = items::serialize_dev_item(1, 100 * 1024 * 1024, 4096, uuid, fsid);
        assert_eq!(raw.len(), 98);
        let devid = u64::from_le_bytes(raw[0..8].try_into().unwrap());
        assert_eq!(devid, 1);
        let total = u64::from_le_bytes(raw[8..16].try_into().unwrap());
        assert_eq!(total, 100 * 1024 * 1024);
        assert_eq!(&raw[66..82], &uuid);
        assert_eq!(&raw[82..98], &fsid);
    }

    #[test]
    fn test_chunk_item_serialization() {
        let dev_uuid = [0xccu8; 16];
        let raw = items::serialize_chunk_item(4 * 1024 * 1024, 2, BTRFS_BLOCK_GROUP_SYSTEM, 0x100000, dev_uuid);
        assert_eq!(raw.len(), 80);
        let length = u64::from_le_bytes(raw[0..8].try_into().unwrap());
        assert_eq!(length, 4 * 1024 * 1024);
        let num_stripes = u16::from_le_bytes(raw[44..46].try_into().unwrap());
        assert_eq!(num_stripes, 1);
        let stripe_off = u64::from_le_bytes(raw[56..64].try_into().unwrap());
        assert_eq!(stripe_off, 0x100000);
        assert_eq!(&raw[64..80], &dev_uuid);
    }

    #[test]
    fn test_extent_items_serialization() {
        let tree_block = items::serialize_extent_item_tree_block(1, BTRFS_FS_TREE_OBJECTID);
        assert_eq!(tree_block.len(), 33);
        let flags = u64::from_le_bytes(tree_block[16..24].try_into().unwrap());
        assert_eq!(flags, BTRFS_EXTENT_FLAG_TREE_BLOCK);
        assert_eq!(tree_block[24], BTRFS_TREE_BLOCK_REF_KEY);
        let root = u64::from_le_bytes(tree_block[25..33].try_into().unwrap());
        assert_eq!(root, BTRFS_FS_TREE_OBJECTID);

        let data_ref = items::serialize_extent_item_data(1, 256, 257, 0);
        assert_eq!(data_ref.len(), 53);
        assert_eq!(data_ref[24], BTRFS_EXTENT_DATA_REF_KEY);
    }

    #[test]
    fn test_block_group_item_serialization() {
        let raw = items::serialize_block_group_item(16384, BTRFS_BLOCK_GROUP_METADATA, 0);
        assert_eq!(raw.len(), 24);
        let used = u64::from_le_bytes(raw[0..8].try_into().unwrap());
        assert_eq!(used, 16384);
        let chunk_obj = u64::from_le_bytes(raw[8..16].try_into().unwrap());
        assert_eq!(chunk_obj, BTRFS_FIRST_CHUNK_TREE_OBJECTID);
        let flags = u64::from_le_bytes(raw[16..24].try_into().unwrap());
        assert_eq!(flags, BTRFS_BLOCK_GROUP_METADATA);
    }

    #[test]
    fn test_dev_extent_serialization() {
        let uuid = [0x55u8; 16];
        let raw = items::serialize_dev_extent(0x100000, 2 * 1024 * 1024, uuid);
        assert_eq!(raw.len(), 48);
        let chunk_tree = u64::from_le_bytes(raw[0..8].try_into().unwrap());
        assert_eq!(chunk_tree, BTRFS_CHUNK_TREE_OBJECTID);
        let chunk_obj = u64::from_le_bytes(raw[8..16].try_into().unwrap());
        assert_eq!(chunk_obj, BTRFS_FIRST_CHUNK_TREE_OBJECTID);
        let off = u64::from_le_bytes(raw[16..24].try_into().unwrap());
        assert_eq!(off, 0x100000);
        let len = u64::from_le_bytes(raw[24..32].try_into().unwrap());
        assert_eq!(len, 2 * 1024 * 1024);
        assert_eq!(&raw[32..48], &uuid);
    }
}
