//! Append a file to an existing exFAT volume (see `kernel/src/logdump.rs`).
//!
//! Free space comes from the allocation bitmap (the 0x81 root entry), not the FAT: contiguous
//! (NoFatChain) files have no FAT chain, so a FAT scan hands out clusters that are in use.
//! The stream NameHash is over the up-cased name via the 0x82 up-case table.
//!
//! Write order: file data, file FAT chain, new root clusters, bitmap, root FAT link, and the
//! directory entry set last. A crash leaks clusters or leaves the volume flagged dirty; it
//! never points a live directory entry at unwritten data.
#![no_std]

extern crate alloc;

mod bitmap;
mod upcase;

use alloc::vec;
use alloc::vec::Vec;
use bitmap::Bitmap;
use upcase::{upcase_hash, Upcase};

pub trait Storage {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Io,
    NotExfat,
    Invalid(&'static str),
    NoSpace,
    Exists,
}

pub type Result<T> = core::result::Result<T, Error>;

const VBR_JUMP: [u8; 3] = [0xeb, 0x76, 0x90];
const FS_NAME: &[u8; 8] = b"EXFAT   ";
const VBR_SIGNATURE: u16 = 0xaa55;

const ET_EOD: u8 = 0x00;
const ET_FILE: u8 = 0x85;
const ET_BITMAP: u8 = 0x81;
const ET_UPCASE: u8 = 0x82;
const ET_STREAM: u8 = 0xc0;
const ET_NAME: u8 = 0xc1;
const ATTR_ARCHIVE: u16 = 0x20;
const ATTR_DIRECTORY: u16 = 0x10;

const BS_VOLUME_FLAGS: usize = 106;
const BS_PERCENT_IN_USE: usize = 112;
const VOLUME_FLAG_DIRTY: u16 = 0x0002;

const FAT_EOC: u32 = 0xffff_ffff;
const FAT_EOC_MIN: u32 = 0xffff_fff8;
const MAX_ROOT_BYTES: usize = 4 << 20;
const MAX_BITMAP_BYTES: u64 = 32 << 20;
const MAX_UPCASE_BYTES: u64 = 0x20000;
/// No clock in the installer kernel: stamp files with the exFAT epoch (1980-01-01 00:00:00).
const EXFAT_EPOCH_TS: u32 = 0x0021_0000;

#[derive(Debug, Clone)]
struct Vbr {
    sector_size: u32,
    sectors_per_cluster: u32,
    fat_offset: u64,
    fat_length: u64,
    cluster_heap_offset: u64,
    cluster_count: u32,
    root_cluster: u32,
    has_second_fat: bool,
}

impl Vbr {
    fn cluster_bytes(&self) -> u64 {
        self.sector_size as u64 * self.sectors_per_cluster as u64
    }
    /// Callers must have validated `cluster >= 2`.
    fn cluster_offset(&self, cluster: u32) -> u64 {
        self.cluster_heap_offset + (cluster as u64 - 2) * self.cluster_bytes()
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

fn rd_u16(b: &[u8], off: usize) -> u16 {
    b.get(off..off + 2)
        .and_then(|s| <[u8; 2]>::try_from(s).ok())
        .map_or(0, u16::from_le_bytes)
}

fn rd_u32(b: &[u8], off: usize) -> u32 {
    b.get(off..off + 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map_or(0, u32::from_le_bytes)
}

fn rd_u64(b: &[u8], off: usize) -> u64 {
    b.get(off..off + 8)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
        .map_or(0, u64::from_le_bytes)
}

fn parse_vbr(sector: &[u8]) -> Result<Vbr> {
    if sector.len() < 512
        || sector.get(0..3) != Some(&VBR_JUMP)
        || sector.get(3..11) != Some(&FS_NAME[..])
    {
        return Err(Error::NotExfat);
    }
    if rd_u16(sector, 510) != VBR_SIGNATURE {
        return Err(Error::NotExfat);
    }
    let bps_shift = sector.get(108).copied().unwrap_or(0);
    let spc_shift = sector.get(109).copied().unwrap_or(0);
    if !(9..=12).contains(&bps_shift) || spc_shift > 25 - bps_shift {
        return Err(Error::Invalid("exFAT shifts out of range"));
    }
    let ss = 1u32 << bps_shift;
    let spc = 1u32 << spc_shift;
    let fat_offset = rd_u32(sector, 80) as u64 * ss as u64;
    let fat_length = rd_u32(sector, 84) as u64 * ss as u64;
    let heap = rd_u32(sector, 88) as u64 * ss as u64;
    let cluster_count = rd_u32(sector, 92);
    let root_cluster = rd_u32(sector, 96);
    if cluster_count < 1
        || root_cluster < 2
        || root_cluster >= cluster_count.saturating_add(2)
        || fat_length == 0
    {
        return Err(Error::Invalid("no cluster heap or no root dir"));
    }
    Ok(Vbr {
        sector_size: ss,
        sectors_per_cluster: spc,
        fat_offset,
        fat_length,
        cluster_heap_offset: heap,
        cluster_count,
        root_cluster,
        has_second_fat: sector.get(110).copied() == Some(2),
    })
}

/// Whether `sector` (the first 512+ bytes of a partition) is an exFAT boot sector.
pub fn is_exfat_boot_sector(sector: &[u8]) -> bool {
    parse_vbr(sector).is_ok()
}

/// Expands the (possibly compressed) up-case table into a full mapping. `0xFFFF, n` means the
/// next `n` characters map to themselves.
fn expand_upcase(raw: &[u8]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    let mut k = 0usize;
    while k + 2 <= raw.len() && out.len() <= 0xffff {
        let v = rd_u16(raw, k);
        k += 2;
        if v == 0xffff {
            if k + 2 > raw.len() {
                break;
            }
            let n = rd_u16(raw, k) as usize;
            k += 2;
            for _ in 0..n {
                let idx = out.len();
                if idx > 0xffff {
                    break;
                }
                out.push(idx as u16);
            }
        } else {
            out.push(v);
        }
    }
    out
}

fn validate_name(units: &[u16]) -> Result<()> {
    if units.is_empty() || units.len() > 255 {
        return Err(Error::Invalid("bad file name length"));
    }
    for &u in units {
        if u < 0x20
            || matches!(
                u,
                0x22 | 0x2a | 0x2f | 0x3a | 0x3c | 0x3e | 0x3f | 0x5c | 0x7c
            )
        {
            return Err(Error::Invalid("bad character in file name"));
        }
    }
    Ok(())
}

fn entry_set_checksum(set: &[u8]) -> u16 {
    let mut sum: u16 = 0;
    for (i, &b) in set.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        sum = sum.rotate_right(1).wrapping_add(b as u16);
    }
    sum
}

/// Builds the file + stream + name entry set. `up` is the up-cased name (for the NameHash).
fn build_entry_set(units: &[u16], up: &[u16], first: u32, size: u64) -> Vec<u8> {
    let name_entries = units.len().div_ceil(15);
    let mut set = vec![0u8; 32 * (2 + name_entries)];
    put_u8(&mut set, 0, ET_FILE);
    put_u8(&mut set, 1, (1 + name_entries) as u8);
    put_slice(&mut set, 4, &ATTR_ARCHIVE.to_le_bytes());
    for off in [8usize, 12, 16] {
        put_slice(&mut set, off, &EXFAT_EPOCH_TS.to_le_bytes());
    }
    put_u8(&mut set, 32, ET_STREAM);
    put_u8(&mut set, 33, if size > 0 { 0x01 } else { 0x00 }); // allocation possible, FAT chain valid
    put_u8(&mut set, 35, units.len() as u8);
    put_slice(&mut set, 36, &upcase_hash(up).to_le_bytes());
    put_slice(&mut set, 40, &size.to_le_bytes()); // ValidDataLength
    put_slice(&mut set, 52, &first.to_le_bytes());
    put_slice(&mut set, 56, &size.to_le_bytes()); // DataLength
    for (k, chunk) in units.chunks(15).enumerate() {
        let o = 64 + 32 * k;
        put_u8(&mut set, o, ET_NAME);
        for (j, &u) in chunk.iter().enumerate() {
            put_slice(&mut set, o + 2 + 2 * j, &u.to_le_bytes());
        }
    }
    let ck = entry_set_checksum(&set);
    put_slice(&mut set, 2, &ck.to_le_bytes());
    set
}

pub struct Appender<'a, S: Storage> {
    s: &'a mut S,
    vbr: Vbr,
    bitmap: Bitmap,
    bitmap_len: usize,
    upcase: Upcase,
    /// Inclusive byte range of the in-memory bitmap that differs from disk.
    bm_dirty: Option<(usize, usize)>,
}

impl<'a, S: Storage> Appender<'a, S> {
    /// Opens the exFAT volume whose boot sector is at the start of the store.
    pub fn open(s: &'a mut S) -> Result<Self> {
        let mut vbr_buf = vec![0u8; 512];
        s.read_at(0, &mut vbr_buf)?;
        let vbr = parse_vbr(&vbr_buf)?;
        let mut app = Appender {
            s,
            vbr,
            bitmap: Bitmap {
                bytes: Vec::new(),
                first_cluster: 0,
            },
            bitmap_len: 0,
            upcase: Upcase { units: Vec::new() },
            bm_dirty: None,
        };
        app.load_metadata()?;
        Ok(app)
    }

    /// Whether a file or directory called `name` (case-insensitive) exists in the root.
    pub fn exists(&mut self, name: &str) -> Result<bool> {
        Ok(self.lookup(name)?.is_some())
    }

    /// Whether a directory called `name` (case-insensitive) exists in the root.
    pub fn root_has(&mut self, name: &str) -> Result<bool> {
        self.has_dir(name)
    }
    pub fn has_dir(&mut self, name: &str) -> Result<bool> {
        Ok(matches!(self.lookup(name)?, Some(a) if a & ATTR_DIRECTORY != 0))
    }

    /// Appends a new file to the root directory. Refuses to overwrite and refuses a dirty volume.
    pub fn append(&mut self, name: &str, data: &[u8]) -> Result<()> {
        let r = self.append_inner(name, data);
        if r.is_err() {
            // Drop allocations that never reached the disk.
            match self.load_metadata() {
                Ok(()) | Err(_) => {}
            }
        }
        r
    }

    fn lookup(&mut self, name: &str) -> Result<Option<u16>> {
        let up: Vec<u16> = name.encode_utf16().map(|u| self.upcase.fold(u)).collect();
        let (_chain, root) = self.read_root_chain()?;
        Ok(self.scan_root(&root, &up).1)
    }

    fn append_inner(&mut self, name: &str, data: &[u8]) -> Result<()> {
        let units: Vec<u16> = name.encode_utf16().collect();
        validate_name(&units)?;
        let up: Vec<u16> = units.iter().map(|&u| self.upcase.fold(u)).collect();

        let (chain, mut root) = self.read_root_chain()?;
        let (eod, found) = self.scan_root(&root, &up);
        if found.is_some() {
            return Err(Error::Exists);
        }
        if self.volume_flags()? & VOLUME_FLAG_DIRTY != 0 {
            return Err(Error::Invalid("volume is marked dirty"));
        }

        let cb = self.vbr.cluster_bytes() as usize;
        let set_len = 32 * (2 + units.len().div_ceil(15));

        // Allocate everything in memory first; nothing touches the disk until all of it fits.
        let need = data.len().div_ceil(cb);
        let mut hint = 2u32;
        let mut file_clusters: Vec<u32> = Vec::with_capacity(need);
        for _ in 0..need {
            let c = self.alloc_cluster(hint)?;
            hint = c + 1;
            file_clusters.push(c);
        }

        let end = eod + set_len;
        let mut new_root: Vec<u32> = Vec::new();
        if end > root.len() {
            let n = (end - root.len()).div_ceil(cb);
            for _ in 0..n {
                let c = self.alloc_cluster(hint)?;
                hint = c + 1;
                new_root.push(c);
            }
            root.resize(root.len() + n * cb, 0);
        }

        let first = file_clusters.first().copied().unwrap_or(0);
        let set = build_entry_set(&units, &up, first, data.len() as u64);
        put_slice(&mut root, eod, &set);
        // Whatever followed the old end-of-directory must not be parsed as entries.
        let touched_end = if end < root.len() {
            put_u8(&mut root, end, ET_EOD);
            end + 1
        } else {
            end
        };

        let mut all: Vec<u32> = chain.clone();
        all.extend_from_slice(&new_root);
        let old_n = chain.len();
        let first_ci = eod / cb;
        let last_ci = (touched_end - 1) / cb;

        self.set_dirty(true)?;

        // 1. file data
        for (k, &c) in file_clusters.iter().enumerate() {
            let mut buf = vec![0u8; cb];
            let lo = k * cb;
            let hi = (lo + cb).min(data.len());
            if let Some(src) = data.get(lo..hi) {
                put_slice(&mut buf, 0, src);
            }
            self.write_cluster(c, &buf)?;
        }
        // 2. file FAT chain
        for (k, &c) in file_clusters.iter().enumerate() {
            let next = file_clusters.get(k + 1).copied().unwrap_or(FAT_EOC);
            self.write_fat(c, next)?;
        }
        // 3. new root clusters (content, then their own chain; not linked in yet)
        for ci in old_n..all.len() {
            if let (Some(&cluster), Some(slice)) = (all.get(ci), root.get(ci * cb..(ci + 1) * cb)) {
                self.write_cluster(cluster, slice)?;
            }
        }
        for (k, &c) in new_root.iter().enumerate() {
            let next = new_root.get(k + 1).copied().unwrap_or(FAT_EOC);
            self.write_fat(c, next)?;
        }
        // 4. allocation bitmap
        self.flush_bitmap()?;
        // 5. link the new root clusters into the root chain
        if let Some(&c0) = new_root.first() {
            let last_old = *chain.last().ok_or(Error::Invalid("empty root chain"))?;
            self.write_fat(last_old, c0)?;
        }
        // 6. the existing root clusters that now hold the entry set
        for ci in first_ci..=last_ci {
            if ci < old_n {
                if let (Some(&cluster), Some(slice)) = (chain.get(ci), root.get(ci * cb..(ci + 1) * cb)) {
                    self.write_cluster(cluster, slice)?;
                }
            }
        }
        self.update_usage()?;
        self.set_dirty(false)
    }

    /// Loads the allocation bitmap (0x81) and up-case table (0x82) from the root.
    fn load_metadata(&mut self) -> Result<()> {
        self.bitmap = Bitmap {
            bytes: Vec::new(),
            first_cluster: 0,
        };
        self.bitmap_len = 0;
        self.upcase = Upcase { units: Vec::new() };
        self.bm_dirty = None;

        let (_chain, root) = self.read_root_chain()?;
        let cc = self.vbr.cluster_count;
        let mut i = 0usize;
        while i + 32 <= root.len() {
            let t = root.get(i).copied().unwrap_or(ET_EOD);
            if t == ET_EOD {
                break;
            }
            if t == ET_BITMAP && self.bitmap.bytes.is_empty() {
                let first = rd_u32(&root, i + 20);
                let len64 = rd_u64(&root, i + 24);
                if first < 2
                    || first >= cc + 2
                    || len64 == 0
                    || len64 > MAX_BITMAP_BYTES
                    || len64 * 8 < cc as u64
                {
                    return Err(Error::Invalid("bad bitmap"));
                }
                let len = len64 as usize;
                let mut bytes = vec![0u8; len];
                let off = self.vbr.cluster_offset(first);
                self.s.read_at(off, &mut bytes)?; // contiguous, like every real implementation
                self.bitmap = Bitmap {
                    bytes,
                    first_cluster: first,
                };
                self.bitmap_len = len;
            } else if t == ET_UPCASE && self.upcase.units.is_empty() {
                let stored = rd_u32(&root, i + 4);
                let first = rd_u32(&root, i + 20);
                let len64 = rd_u64(&root, i + 24);
                if first < 2 || first >= cc + 2 || !(2..=MAX_UPCASE_BYTES).contains(&len64) {
                    return Err(Error::Invalid("bad upcase"));
                }
                let mut raw = vec![0u8; len64 as usize];
                let off = self.vbr.cluster_offset(first);
                self.s.read_at(off, &mut raw)?;
                if Upcase::checksum(&raw) != stored {
                    return Err(Error::Invalid("up-case checksum mismatch"));
                }
                self.upcase = Upcase {
                    units: expand_upcase(&raw),
                };
            }
            i += 32;
        }
        if self.bitmap.bytes.is_empty() {
            return Err(Error::Invalid("no bitmap"));
        }
        if self.upcase.units.is_empty() {
            return Err(Error::Invalid("no upcase"));
        }
        Ok(())
    }

    /// The root directory is always a FAT chain. Returns its clusters and its bytes.
    fn read_root_chain(&mut self) -> Result<(Vec<u32>, Vec<u8>)> {
        let mut chain: Vec<u32> = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut c = self.vbr.root_cluster;
        loop {
            if c < 2 || c >= self.vbr.cluster_count + 2 {
                return Err(Error::Invalid("root chain out of range"));
            }
            if chain.len() as u32 > self.vbr.cluster_count || buf.len() > MAX_ROOT_BYTES {
                return Err(Error::Invalid("root chain too long or looping"));
            }
            chain.push(c);
            let data = self.read_cluster(c)?;
            buf.extend_from_slice(&data);
            let next = self.read_fat(c)?;
            if next >= FAT_EOC_MIN {
                break;
            }
            c = next;
        }
        Ok((chain, buf))
    }

    /// Walks the root up to the end-of-directory marker. Returns the offset of that marker and,
    /// if an entry set named `want_up` (up-cased) exists, its file attributes.
    fn scan_root(&self, root: &[u8], want_up: &[u16]) -> (usize, Option<u16>) {
        let mut i = 0usize;
        let mut found: Option<u16> = None;
        while i + 32 <= root.len() {
            let t = root.get(i).copied().unwrap_or(ET_EOD);
            if t == ET_EOD {
                return (i, found);
            }
            if t == ET_FILE {
                let n = root.get(i + 1).copied().unwrap_or(0) as usize;
                if found.is_none() {
                    if let Some(up) = self.set_name_upcased(root, i) {
                        if up.as_slice() == want_up {
                            found = Some(rd_u16(root, i + 4));
                        }
                    }
                }
                i += 32 * (n + 1);
            } else {
                i += 32;
            }
        }
        (root.len(), found)
    }

    /// The up-cased name of the entry set starting at `i`, or None if the set is malformed.
    fn set_name_upcased(&self, root: &[u8], i: usize) -> Option<Vec<u16>> {
        let n = root.get(i + 1).copied().unwrap_or(0) as usize;
        let end = i + 32 * (n + 1);
        if n < 2 || end > root.len() || root.get(i + 32).copied() != Some(ET_STREAM) {
            return None;
        }
        let nlen = root.get(i + 35).copied().unwrap_or(0) as usize;
        let mut out: Vec<u16> = Vec::with_capacity(nlen);
        let mut k = i + 64;
        while k < end && out.len() < nlen {
            if root.get(k).copied() != Some(ET_NAME) {
                return None;
            }
            for u in 0..15 {
                if out.len() == nlen {
                    break;
                }
                out.push(self.upcase.fold(rd_u16(root, k + 2 + u * 2)));
            }
            k += 32;
        }
        if out.len() == nlen {
            Some(out)
        } else {
            None
        }
    }

    fn check_cluster(&self, cluster: u32) -> Result<()> {
        if cluster < 2 || cluster >= self.vbr.cluster_count + 2 {
            return Err(Error::Invalid("cluster out of range"));
        }
        Ok(())
    }

    fn fat_entry_offset(&self, cluster: u32) -> Result<u64> {
        let rel = cluster as u64 * 4;
        if rel + 4 > self.vbr.fat_length {
            return Err(Error::Invalid("cluster outside FAT"));
        }
        Ok(self.vbr.fat_offset + rel)
    }

    fn read_fat(&mut self, cluster: u32) -> Result<u32> {
        let off = self.fat_entry_offset(cluster)?;
        let mut b = [0u8; 4];
        self.s.read_at(off, &mut b)?;
        Ok(rd_u32(&b, 0))
    }

    fn write_fat(&mut self, cluster: u32, value: u32) -> Result<()> {
        let ent = self.fat_entry_offset(cluster)?;
        let val = value.to_le_bytes();
        self.s.write_at(ent, &val)?;
        if self.vbr.has_second_fat {
            // fat_length is already in bytes; the second FAT follows the first.
            let second = self.vbr.fat_length;
            self.s.write_at(ent + second, &val)?;
        }
        Ok(())
    }

    /// Allocates the first free cluster at or after `from`, per the allocation bitmap.
    /// Only the in-memory bitmap changes; `flush_bitmap` writes it out.
    fn alloc_cluster(&mut self, from: u32) -> Result<u32> {
        let end = self.vbr.cluster_count + 2;
        let mut c = from.max(2);
        while c < end {
            if (c - 2).is_multiple_of(8) {
                if let Some(&b) = self.bitmap.bytes.get(((c - 2) / 8) as usize) {
                    if b == 0xff {
                        c += 8;
                        continue;
                    }
                }
            }
            if !self.bitmap.used(c) {
                let byte = self.bitmap.set(c).ok_or(Error::NoSpace)?;
                self.bm_dirty = Some(match self.bm_dirty {
                    Some((lo, hi)) => (lo.min(byte), hi.max(byte)),
                    None => (byte, byte),
                });
                return Ok(c);
            }
            c += 1;
        }
        Err(Error::NoSpace)
    }

    fn flush_bitmap(&mut self) -> Result<()> {
        if let Some((lo, hi)) = self.bm_dirty {
            let base = self.vbr.cluster_offset(self.bitmap.first_cluster) + lo as u64;
            if let Some(slice) = self.bitmap.bytes.get(lo..=hi) {
                self.s.write_at(base, slice)?;
            }
            self.bm_dirty = None;
        }
        Ok(())
    }

    fn read_cluster(&mut self, cluster: u32) -> Result<Vec<u8>> {
        self.check_cluster(cluster)?;
        let cb = self.vbr.cluster_bytes() as usize;
        let off = self.vbr.cluster_offset(cluster);
        let mut buf = vec![0u8; cb];
        self.s.read_at(off, &mut buf)?;
        Ok(buf)
    }

    fn write_cluster(&mut self, cluster: u32, data: &[u8]) -> Result<()> {
        self.check_cluster(cluster)?;
        let off = self.vbr.cluster_offset(cluster);
        self.s.write_at(off, data)
    }

    fn volume_flags(&mut self) -> Result<u16> {
        let mut b = [0u8; 2];
        self.s.read_at(BS_VOLUME_FLAGS as u64, &mut b)?;
        Ok(u16::from_le_bytes(b))
    }

    fn set_dirty(&mut self, dirty: bool) -> Result<()> {
        let mut f = self.volume_flags()?;
        if dirty {
            f |= VOLUME_FLAG_DIRTY;
        } else {
            f &= !VOLUME_FLAG_DIRTY;
        }
        self.s.write_at(BS_VOLUME_FLAGS as u64, &f.to_le_bytes())
    }

    fn percent_in_use(&self) -> u8 {
        let cc = self.vbr.cluster_count as usize;
        let full = cc / 8;
        let rem = cc % 8;
        let mut used = 0usize;
        let slice = self.bitmap.bytes.get(..full).unwrap_or(&self.bitmap.bytes);
        for b in slice {
            used += b.count_ones() as usize;
        }
        if rem > 0 {
            if let Some(&b) = self.bitmap.bytes.get(full) {
                used += (b & ((1u8 << rem) - 1)).count_ones() as usize;
            }
        }
        (used * 100 / cc.max(1)) as u8
    }

    /// Refreshes PercentInUse (not covered by the boot checksum). 0xFF ("unknown") is left alone.
    fn update_usage(&mut self) -> Result<()> {
        let mut cur = [0u8; 1];
        self.s.read_at(BS_PERCENT_IN_USE as u64, &mut cur)?;
        if cur.first() == Some(&0xff) {
            return Ok(());
        }
        let p = self.percent_in_use();
        if cur.first() != Some(&p) {
            self.s.write_at(BS_PERCENT_IN_USE as u64, &[p])?;
        }
        Ok(())
    }
}
