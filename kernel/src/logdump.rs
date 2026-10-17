//! Writes the captured kernel log (see `logbuf.rs`) to a USB flash drive at the end of the install
//! flow, on both success and failure, immediately before the reboot.
//!
//! The scope is deliberately narrow: it only writes when the boot medium is a Ventoy stick whose data
//! partition is exFAT. In every other case it prints one line and returns without touching any disk.
//!
//! How the stick is found: the loader records a fingerprint of the disk it booted from
//! ([`bootinfo::BootMedium`]); this module reads LBA0 of every enumerated disk and matches. On the boot
//! disk it walks the partition table (GPT, or a protective/hybrid MBR), opens each partition that could
//! hold the Ventoy data, checks the exFAT boot sector, and only writes when the volume has a `ventoy`
//! directory (Ventoy's own marker; a plain exFAT stick is left alone). The log is then appended as a
//! new file so a rerun never overwrites an earlier log.
//!
//! The whole thing degrades to a one-line skip on every path: no boot medium, no boot disk among the
//! enumerated ones, no exFAT partition with Ventoy's marker, an existing log file, or an I/O error. It
//! never panics and never changes the install result.
use crate::println;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use drivers::Devices;
use hal::BlockDevice;

/// What the dump did.
pub enum Outcome {
    /// The log file was written; carries its path on the volume.
    Written(String),
    /// Nothing was written; carries a one-line reason for the console.
    Skipped(&'static str),
}

/// A byte-addressed view of one partition, for `exfatw`.
struct PartitionStorage<'a> {
    dev: &'a mut dyn BlockDevice,
    base: u64,
    len: u64,
}

impl exfatw::Storage for PartitionStorage<'_> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> exfatw::Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(exfatw::Error::Invalid("offset"))?;
        if end > self.len {
            return Err(exfatw::Error::Invalid("read beyond partition"));
        }
        let ss = self.dev.sector_size() as u64;
        let mut pos = self.base + offset;
        let mut done = 0usize;
        while done < buf.len() {
            let lba = pos / ss;
            let skip = (pos % ss) as usize;
            let n = (ss as usize - skip).min(buf.len() - done);
            let mut sector = vec![0u8; ss as usize];
            self.dev
                .read(lba, &mut sector)
                .map_err(|_| exfatw::Error::Io)?;
            buf[done..done + n].copy_from_slice(&sector[skip..skip + n]);
            done += n;
            pos += n as u64;
        }
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> exfatw::Result<()> {
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(exfatw::Error::Invalid("offset"))?;
        if end > self.len {
            return Err(exfatw::Error::Invalid("write beyond partition"));
        }
        let ss = self.dev.sector_size() as u64;
        let mut pos = self.base + offset;
        let mut done = 0usize;
        while done < data.len() {
            let lba = pos / ss;
            let skip = (pos % ss) as usize;
            let n = (ss as usize - skip).min(data.len() - done);
            let mut sector = vec![0u8; ss as usize];
            if skip != 0 || n != ss as usize {
                self.dev
                    .read(lba, &mut sector)
                    .map_err(|_| exfatw::Error::Io)?;
            }
            sector[skip..skip + n].copy_from_slice(&data[done..done + n]);
            self.dev
                .write(lba, &sector)
                .map_err(|_| exfatw::Error::Io)?;
            done += n;
            pos += n as u64;
        }
        Ok(())
    }
}

/// A partition found in the boot disk's table: byte offset and length on the disk.
struct Part {
    offset: u64,
    len: u64,
}

/// Reads `lba` (one sector) from `dev`.
fn read_lba(dev: &mut dyn BlockDevice, lba: u64) -> Option<Vec<u8>> {
    let ss = dev.sector_size() as usize;
    let mut buf = vec![0u8; ss];
    dev.read(lba, &mut buf).ok()?;
    Some(buf)
}

/// Whether `dev` is the disk the loader booted from, by the recorded fingerprint.
fn is_boot_disk(dev: &mut dyn BlockDevice, want: &bootinfo::BootMedium) -> bool {
    if dev.sector_size() != want.sector_size || dev.sector_count() != want.sectors {
        return false;
    }
    let lba0 = match read_lba(dev, 0) {
        Some(b) => b,
        None => return false,
    };
    let got = bootinfo::BootMedium::fingerprint(&lba0, dev.sector_size(), dev.sector_count());
    got == want.lba0_hash
}

/// Parses the partition table of the boot disk: GPT when the protective/hybrid MBR is present, else the
/// MBR's own four entries.
fn partitions(dev: &mut dyn BlockDevice) -> Vec<Part> {
    let ss = dev.sector_size() as u64;
    let mbr = match read_lba(dev, 0) {
        Some(b) => b,
        None => return Vec::new(),
    };
    if mbr.len() < 512 {
        return Vec::new();
    }

    // GPT: LBA1 starts with "EFI PART".
    if let Some(gpt) = read_lba(dev, 1) {
        if gpt.len() >= 92 && &gpt[0..8] == b"EFI PART" {
            let entries_lba = u64::from_le_bytes(gpt[72..80].try_into().unwrap());
            let count = u32::from_le_bytes(gpt[80..84].try_into().unwrap()).min(128) as u64;
            let entry_size = u32::from_le_bytes(gpt[84..88].try_into().unwrap()).max(128) as u64;
            let mut out = Vec::new();
            for i in 0..count {
                let off = entries_lba * ss + i * entry_size;
                let mut e = vec![0u8; entry_size as usize];
                if read_at(dev, off, &mut e).is_none() {
                    break;
                }
                let type_guid = &e[0..16];
                if type_guid.iter().all(|&b| b == 0) {
                    continue;
                }
                let first = u64::from_le_bytes(e[32..40].try_into().unwrap());
                let last = u64::from_le_bytes(e[40..48].try_into().unwrap());
                if last < first {
                    continue;
                }
                out.push(Part {
                    offset: first * ss,
                    len: (last - first + 1) * ss,
                });
            }
            return out;
        }
    }

    // MBR: four 16-byte entries at 446, each with an LBA start and a sector count.
    let mut out = Vec::new();
    for i in 0..4 {
        let e = 446 + i * 16;
        let ptype = mbr[e + 4];
        if ptype == 0 {
            continue;
        }
        let start = u32::from_le_bytes(mbr[e + 8..e + 12].try_into().unwrap()) as u64;
        let sectors = u32::from_le_bytes(mbr[e + 12..e + 16].try_into().unwrap()) as u64;
        if start == 0 || sectors == 0 {
            continue;
        }
        out.push(Part {
            offset: start * ss,
            len: sectors * ss,
        });
    }
    out
}

/// Reads `buf.len()` bytes from `dev` at byte offset `off`.
fn read_at(dev: &mut dyn BlockDevice, off: u64, buf: &mut [u8]) -> Option<()> {
    let ss = dev.sector_size() as u64;
    let mut pos = off;
    let mut done = 0usize;
    while done < buf.len() {
        let lba = pos / ss;
        let skip = (pos % ss) as usize;
        let n = (ss as usize - skip).min(buf.len() - done);
        let mut sector = vec![0u8; ss as usize];
        dev.read(lba, &mut sector).ok()?;
        buf[done..done + n].copy_from_slice(&sector[skip..skip + n]);
        done += n;
        pos += n as u64;
    }
    Some(())
}

/// The log file's name on the stick. A fixed name keeps reruns from filling the stick; a second run
/// with the same name fails and the dump reports it rather than overwriting.
const LOG_NAME: &str = "archstaller-log.txt";

/// Runs the log dump. Called at the end of the install flow, on success and on failure, right before
/// the reboot. Never panics and never returns an error: a dump that cannot happen is a `Skipped`, and a
/// dump that fails is reported but does not change the install result.
pub fn run(devs: &mut Devices) -> Outcome {
    let info = match crate::boot_info() {
        Some(i) => i,
        None => return Outcome::Skipped("no boot info"),
    };
    let want = info.boot_medium;
    if !want.is_known() {
        return Outcome::Skipped("the boot medium was not identified");
    }

    let log = crate::logbuf::snapshot();

    for dev in devs.block.iter_mut() {
        let d: &mut dyn BlockDevice = dev.as_mut();
        if !is_boot_disk(d, &want) {
            continue;
        }
        for p in partitions(d) {
            if p.len < 512 {
                continue;
            }
            // Is this partition an exFAT volume with Ventoy's marker?
            let mut storage = PartitionStorage {
                dev: d,
                base: p.offset,
                len: p.len,
            };
            let mut vbr = [0u8; 512];
            if exfatw::Storage::read_at(&mut storage, 0, &mut vbr).is_err() {
                continue;
            }
            if !exfatw::is_exfat_boot_sector(&vbr) {
                continue;
            }
            let is_ventoy = match exfatw::Appender::open(&mut storage) {
                Ok(mut a) => a.root_has("ventoy").unwrap_or(false),
                Err(_) => false,
            };
            if !is_ventoy {
                continue;
            }
            let mut a = match exfatw::Appender::open(&mut storage) {
                Ok(a) => a,
                Err(_) => continue,
            };
            match a.append(LOG_NAME, &log) {
                Ok(()) => return Outcome::Written(format!("{LOG_NAME} ({} bytes)", log.len())),
                Err(exfatw::Error::Exists) => {
                    return Outcome::Skipped("the log file already exists on the stick")
                }
                Err(_) => return Outcome::Skipped("writing the log file failed"),
            }
        }
        return Outcome::Skipped("the boot disk has no Ventoy exFAT partition");
    }
    Outcome::Skipped("the boot disk is not among the enumerated disks")
}

/// Convenience for the call sites in `main.rs`: runs the dump and prints one line about what happened.
pub fn run_and_report(devs: &mut Devices) {
    match run(devs) {
        Outcome::Written(path) => println!("log dump: wrote {path}"),
        Outcome::Skipped(why) => println!("log dump skipped: {why}"),
    }
}
