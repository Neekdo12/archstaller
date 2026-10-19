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
        _ => 0,
    }
}

const fn read_byte(p: *const u8, off: usize) -> u8 {
    // SAFETY: Caller ensures off is within the valid buffer bounds.
    let ptr = unsafe { p.add(off) };
    // SAFETY: ptr is non-null, aligned, and valid for reading.
    unsafe { *ptr }
}

const fn write_byte(p: *mut u8, off: usize, val: u8) {
    // SAFETY: Caller ensures off is within the valid buffer bounds.
    let ptr = unsafe { p.add(off) };
    // SAFETY: ptr is non-null, aligned, and valid for writing.
    unsafe { *ptr = val };
}

/// Parses "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" into the mixed-endian on-disk form.
pub const fn guid(s: &str) -> [u8; 16] {
    let b = s.as_bytes();
    let mut raw = [0u8; 16];
    let mut n = 0;
    let mut i = 0;
    while i < b.len() && n < 16 {
        let c = read_byte(b.as_ptr(), i);
        if c != b'-' {
            if i + 1 < b.len() {
                let next = read_byte(b.as_ptr(), i + 1);
                write_byte(raw.as_mut_ptr(), n, hex(c) << 4 | hex(next));
                n += 1;
                i += 2;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    let p = raw.as_ptr();
    [
        read_byte(p, 3),
        read_byte(p, 2),
        read_byte(p, 1),
        read_byte(p, 0),
        read_byte(p, 5),
        read_byte(p, 4),
        read_byte(p, 7),
        read_byte(p, 6),
        read_byte(p, 8),
        read_byte(p, 9),
        read_byte(p, 10),
        read_byte(p, 11),
        read_byte(p, 12),
        read_byte(p, 13),
        read_byte(p, 14),
        read_byte(p, 15),
    ]
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
    pub fn plan(
        sector_size: u32,
        total_sectors: u64,
        esp_bytes: u64,
        guids: [[u8; 16]; 4],
    ) -> Result<Layout> {
        let ss = sector_size as u64;
        if sector_size < 512 || !sector_size.is_power_of_two() || ss > ALIGN {
            return Err(Error::Size("invalid sector size"));
        }
        let align = ALIGN / ss;
        let raw_esp_sectors = esp_bytes.div_ceil(ss);
        if raw_esp_sectors == 0 {
            return Err(Error::Size("esp too small"));
        }
        let esp_sectors = raw_esp_sectors
            .checked_add(align)
            .and_then(|x| x.checked_sub(1))
            .map(|x| x / align)
            .and_then(|x| x.checked_mul(align))
            .ok_or(Error::Size("esp too large"))?;
        let table_sectors = (ENTRIES * ENTRY_SIZE) as u64 / ss;
        let first_usable = 2 + table_sectors;
        let last_usable = total_sectors
            .checked_sub(1 + table_sectors + 1)
            .ok_or(Error::Size("disk too small"))?;
        if last_usable + 1 < align {
            return Err(Error::Size("disk too small for the layout"));
        }
        let bios_first = align;
        let bios_last = bios_first
            .checked_add(align)
            .and_then(|x| x.checked_sub(1))
            .ok_or(Error::Size("disk too small for the layout"))?;
        let esp_first = bios_last
            .checked_add(1)
            .ok_or(Error::Size("disk too small for the layout"))?;
        let esp_last = esp_first
            .checked_add(esp_sectors)
            .and_then(|x| x.checked_sub(1))
            .ok_or(Error::Size("disk too small for the layout"))?;
        let root_first = esp_last
            .checked_add(1)
            .ok_or(Error::Size("disk too small for the layout"))?;
        let root_last = ((last_usable + 1) / align * align)
            .checked_sub(1)
            .ok_or(Error::Size("disk too small for the layout"))?;
        if bios_first < first_usable || root_last <= root_first.saturating_add(align) {
            return Err(Error::Size("disk too small for the layout"));
        }
        let [disk_guid, g1, g2, g3] = guids;
        let mk = |type_guid, unique_guid, first_lba, last_lba, name| Partition {
            type_guid,
            unique_guid,
            first_lba,
            last_lba,
            name,
        };
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
        if let Some(p) = self.partitions.get(index) {
            (
                p.first_lba * self.sector_size as u64,
                p.sectors() * self.sector_size as u64,
            )
        } else {
            (0, 0)
        }
    }

    /// Writes protective MBR, both headers and both entry arrays.
    pub fn write(&self, dev: &mut Region<'_>) -> Result<()> {
        let ss = self.sector_size as usize;
        let table_sectors = (ENTRIES * ENTRY_SIZE) as u64 / ss as u64;
        let last_lba = self.total_sectors - 1;

        let mut entries = vec![0u8; (ENTRIES * ENTRY_SIZE) as usize];
        for (i, p) in self.partitions.iter().enumerate() {
            let base = i * ENTRY_SIZE as usize;
            put_slice(&mut entries, base, &p.type_guid);
            put_slice(&mut entries, base + 16, &p.unique_guid);
            put_slice(&mut entries, base + 32, &p.first_lba.to_le_bytes());
            put_slice(&mut entries, base + 40, &p.last_lba.to_le_bytes());
            for (j, c) in p.name.encode_utf16().take(36).enumerate() {
                put_slice(&mut entries, base + 56 + 2 * j, &c.to_le_bytes());
            }
        }
        let entries_crc = crc32(&entries);

        // Protective MBR.
        let mut mbr = vec![0u8; ss];
        put_u8(&mut mbr, 446, 0x80); // marked bootable for firmware that insists on an active entry
        put_slice(&mut mbr, 447, &[0, 2, 0]); // CHS of LBA 1
        put_u8(&mut mbr, 450, 0xee);
        put_slice(&mut mbr, 451, &[0xff, 0xff, 0xff]);
        put_slice(&mut mbr, 454, &1u32.to_le_bytes());
        put_slice(&mut mbr, 458, &(last_lba.min(u32::MAX as u64) as u32).to_le_bytes());
        put_u8(&mut mbr, 510, 0x55);
        put_u8(&mut mbr, 511, 0xaa);

        let header = |current: u64, backup: u64, entries_lba: u64| -> Vec<u8> {
            let mut h = vec![0u8; ss];
            put_slice(&mut h, 0, b"EFI PART");
            put_slice(&mut h, 8, &0x0001_0000u32.to_le_bytes());
            put_slice(&mut h, 12, &92u32.to_le_bytes());
            put_slice(&mut h, 24, &current.to_le_bytes());
            put_slice(&mut h, 32, &backup.to_le_bytes());
            put_slice(&mut h, 40, &(2 + table_sectors).to_le_bytes());
            put_slice(&mut h, 48, &(last_lba - 1 - table_sectors).to_le_bytes());
            put_slice(&mut h, 56, &self.disk_guid);
            put_slice(&mut h, 72, &entries_lba.to_le_bytes());
            put_slice(&mut h, 80, &ENTRIES.to_le_bytes());
            put_slice(&mut h, 84, &ENTRY_SIZE.to_le_bytes());
            put_slice(&mut h, 88, &entries_crc.to_le_bytes());
            let c = crc32(h.get(..92).unwrap_or(&[]));
            put_slice(&mut h, 16, &c.to_le_bytes());
            h
        };

        let backup_entries_lba = last_lba - table_sectors;
        dev.write_at(0, &mbr)?;
        dev.write_at(ss as u64, &header(1, last_lba, 2))?;
        dev.write_at(2 * ss as u64, &entries)?;
        dev.write_at(backup_entries_lba * ss as u64, &entries)?;
        dev.write_at(
            last_lba * ss as u64,
            &header(last_lba, 1, backup_entries_lba),
        )?;
        dev.flush()
    }
}
