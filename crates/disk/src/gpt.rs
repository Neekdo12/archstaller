//! GUID partition table with protective MBR.
use crate::crc32::crc32;
use crate::region::Region;
use crate::{Error, Result};
use alloc::vec;
use alloc::vec::Vec;

pub const BIOS_BOOT: [u8; 16] = guid("21686148-6449-6E6F-744E-656564454649");
pub const ESP: [u8; 16] = guid("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
pub const LINUX_ROOT_X86_64: [u8; 16] = guid("4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709");

const ENTRIES: u32 = 128;
const ENTRY_SIZE: u32 = 128;
const ALIGN: u64 = 1 << 20;

const fn hex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("bad hex digit"),
    }
}

/// Parses "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" into the mixed-endian on-disk form.
pub const fn guid(s: &str) -> [u8; 16] {
    let b = s.as_bytes();
    let mut raw = [0u8; 16];
    let mut n = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'-' {
            raw[n] = hex(b[i]) << 4 | hex(b[i + 1]);
            n += 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    [
        raw[3], raw[2], raw[1], raw[0], raw[5], raw[4], raw[7], raw[6], raw[8], raw[9], raw[10], raw[11], raw[12],
        raw[13], raw[14], raw[15],
    ]
}

#[derive(Debug, Clone)]
pub struct Partition {
    pub type_guid: [u8; 16],
    pub unique_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
    pub name: &'static str,
}

impl Partition {
    pub fn sectors(&self) -> u64 {
        self.last_lba - self.first_lba + 1
    }
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub disk_guid: [u8; 16],
    pub sector_size: u32,
    pub total_sectors: u64,
    pub partitions: Vec<Partition>,
}

impl Layout {
    pub const BIOS: usize = 0;
    pub const ESP: usize = 1;
    pub const ROOT: usize = 2;

    /// BIOS boot (1 MiB), ESP (`esp_bytes`), root (rest), all 1 MiB aligned.
    /// `guids` supplies the disk GUID and the three partition GUIDs.
    pub fn plan(sector_size: u32, total_sectors: u64, esp_bytes: u64, guids: [[u8; 16]; 4]) -> Result<Layout> {
        let ss = sector_size as u64;
        let align = ALIGN / ss;
        let table_sectors = (ENTRIES * ENTRY_SIZE) as u64 / ss;
        let first_usable = 2 + table_sectors;
        let last_usable = total_sectors.checked_sub(1 + table_sectors + 1).ok_or(Error::Size("disk too small"))?;
        let bios_first = align;
        let bios_last = bios_first + align - 1;
        let esp_first = bios_last + 1;
        let esp_last = esp_first + esp_bytes / ss - 1;
        let root_first = esp_last + 1;
        let root_last = (last_usable + 1) / align * align - 1;
        if bios_first < first_usable || root_last <= root_first + align {
            return Err(Error::Size("disk too small for the layout"));
        }
        let [disk_guid, g1, g2, g3] = guids;
        let mk = |type_guid, unique_guid, first_lba, last_lba, name| Partition { type_guid, unique_guid, first_lba, last_lba, name };
        Ok(Layout {
            disk_guid,
            sector_size,
            total_sectors,
            partitions: vec![
                mk(BIOS_BOOT, g1, bios_first, bios_last, "BIOS boot"),
                mk(ESP, g2, esp_first, esp_last, "ESP"),
                mk(LINUX_ROOT_X86_64, g3, root_first, root_last, "root"),
            ],
        })
    }

    pub fn byte_range(&self, index: usize) -> (u64, u64) {
        let p = &self.partitions[index];
        (p.first_lba * self.sector_size as u64, p.sectors() * self.sector_size as u64)
    }

    /// Writes protective MBR, both headers and both entry arrays.
    pub fn write(&self, dev: &mut Region<'_>) -> Result<()> {
        let ss = self.sector_size as usize;
        let table_sectors = (ENTRIES * ENTRY_SIZE) as u64 / ss as u64;
        let last_lba = self.total_sectors - 1;

        let mut entries = vec![0u8; (ENTRIES * ENTRY_SIZE) as usize];
        for (i, p) in self.partitions.iter().enumerate() {
            let e = &mut entries[i * ENTRY_SIZE as usize..(i + 1) * ENTRY_SIZE as usize];
            e[0..16].copy_from_slice(&p.type_guid);
            e[16..32].copy_from_slice(&p.unique_guid);
            e[32..40].copy_from_slice(&p.first_lba.to_le_bytes());
            e[40..48].copy_from_slice(&p.last_lba.to_le_bytes());
            for (j, c) in p.name.encode_utf16().take(36).enumerate() {
                e[56 + 2 * j..58 + 2 * j].copy_from_slice(&c.to_le_bytes());
            }
        }
        let entries_crc = crc32(&entries);

        // Protective MBR.
        let mut mbr = vec![0u8; ss];
        mbr[446] = 0x80; // marked bootable for firmware that insists on an active entry
        mbr[447..450].copy_from_slice(&[0, 2, 0]); // CHS of LBA 1
        mbr[450] = 0xee;
        mbr[451..454].copy_from_slice(&[0xff, 0xff, 0xff]);
        mbr[454..458].copy_from_slice(&1u32.to_le_bytes());
        mbr[458..462].copy_from_slice(&(last_lba.min(u32::MAX as u64) as u32).to_le_bytes());
        mbr[510] = 0x55;
        mbr[511] = 0xaa;

        let header = |current: u64, backup: u64, entries_lba: u64| -> Vec<u8> {
            let mut h = vec![0u8; ss];
            h[0..8].copy_from_slice(b"EFI PART");
            h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
            h[12..16].copy_from_slice(&92u32.to_le_bytes());
            h[24..32].copy_from_slice(&current.to_le_bytes());
            h[32..40].copy_from_slice(&backup.to_le_bytes());
            h[40..48].copy_from_slice(&(2 + table_sectors).to_le_bytes());
            h[48..56].copy_from_slice(&(last_lba - 1 - table_sectors).to_le_bytes());
            h[56..72].copy_from_slice(&self.disk_guid);
            h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
            h[80..84].copy_from_slice(&ENTRIES.to_le_bytes());
            h[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
            h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
            let c = crc32(&h[..92]);
            h[16..20].copy_from_slice(&c.to_le_bytes());
            h
        };

        let backup_entries_lba = last_lba - table_sectors;
        dev.write_at(0, &mbr)?;
        dev.write_at(ss as u64, &header(1, last_lba, 2))?;
        dev.write_at(2 * ss as u64, &entries)?;
        dev.write_at(backup_entries_lba * ss as u64, &entries)?;
        dev.write_at(last_lba * ss as u64, &header(last_lba, 1, backup_entries_lba))?;
        dev.flush()
    }
}
