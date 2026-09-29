//! Port of `limine bios-install` for GPT disks with a BIOS boot partition.
use crate::region::Region;
use crate::{Error, Result};
use alloc::vec;

/// Installs the Limine BIOS stages.
///
/// `hdd_bin` is `limine-bios-hdd.bin` (boot sector followed by stage 2). The boot sector is
/// written to LBA 0 without touching the disk ID and the partition table (bytes 440..510) or the
/// timestamp (218..224); the remaining stage 2 goes to the start of the BIOS boot partition at
/// `stage2_offset` (bytes from the start of the disk) and its location is patched into the sector.
/// `limine-bios.sys` must additionally be present on a filesystem of the disk.
pub fn bios_install(disk: &mut Region<'_>, hdd_bin: &[u8], stage2_offset: u64, stage2_max: u64) -> Result<()> {
    if hdd_bin.len() <= 512 {
        return Err(Error::Invalid("limine-bios-hdd.bin too small"));
    }
    let stage2 = &hdd_bin[512..];
    if stage2.len() as u64 > stage2_max {
        return Err(Error::Size("BIOS boot partition too small for stage 2"));
    }
    let mut mbr = vec![0u8; 512];
    disk.read_at(0, &mut mbr)?;
    let mut timestamp = [0u8; 6];
    timestamp.copy_from_slice(&mbr[218..224]);
    let mut orig = [0u8; 70];
    orig.copy_from_slice(&mbr[440..510]);

    disk.write_at(stage2_offset, stage2)?;

    let mut boot = hdd_bin[..512].to_vec();
    boot[0x1a4..0x1ac].copy_from_slice(&stage2_offset.to_le_bytes());
    boot[218..224].copy_from_slice(&timestamp);
    boot[440..510].copy_from_slice(&orig);
    disk.write_at(0, &boot)?;
    disk.flush()
}

/// Extracts the byte array from Limine's `limine-bios-hdd.h`.
#[cfg(feature = "std")]
pub fn parse_hdd_header(text: &str) -> alloc::vec::Vec<u8> {
    text.split(|c: char| !c.is_ascii_hexdigit() && c != 'x')
        .filter_map(|t| t.strip_prefix("0x"))
        .filter_map(|h| u8::from_str_radix(h, 16).ok())
        .collect()
}
