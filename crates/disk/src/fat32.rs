//! Write-once FAT32 with long file names. Directory trees are built in RAM; file data is written
//! as it is added; FATs and directories are written by `finish`.
use crate::region::Region;
use crate::{Error, Result};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

const RESERVED_SECTORS: u32 = 32;
const EOC: u32 = 0x0FFF_FFFF;

pub struct Options {
    /// Volume label (up to 11 characters).
    pub label: String,
    pub volume_id: u32,
    /// Sectors between the start of the disk and this partition (BPB hidden sectors).
    pub hidden_sectors: u32,
    /// Unix time for directory entry timestamps.
    pub now: u64,
}

struct Ent {
    name: String,
    first_cluster: u32,
    size: u32,
    /// Index into `dirs` for directories.
    dir: Option<usize>,
}

struct Dir {
    parent: usize,
    cluster: u32,
    entries: Vec<Ent>,
}

pub struct Fat32Writer<'r, 'd> {
    r: &'r mut Region<'d>,
    ss: u32,
    spc: u32,
    fat_sectors: u32,
    total_sectors: u32,
    clusters: u32,
    fat: Vec<u32>,
    next_cluster: u32,
    dirs: Vec<Dir>,
    opts: Options,
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (date, time) in FAT encoding.
fn fat_time(unix: u64) -> (u16, u16) {
    let days = (unix / 86400) as i64;
    let secs = (unix % 86400) as u32;
    let (y, m, d) = civil_from_days(days);
    let y = (y.clamp(1980, 2107) - 1980) as u16;
    let date = y << 9 | (m as u16) << 5 | d as u16;
    let time = ((secs / 3600) as u16) << 11 | (((secs / 60) % 60) as u16) << 5 | ((secs % 60) / 2) as u16;
    (date, time)
}

fn legal_short(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || "!#$%&'()-@^_`{}~".contains(c)
}

/// Returns the 8.3 name and whether a long name entry is also required.
fn short_name(name: &str, taken: &[[u8; 11]]) -> ([u8; 11], bool) {
    let (base, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    };
    let exact = !base.is_empty()
        && base.len() <= 8
        && ext.len() <= 3
        && base.chars().all(legal_short)
        && ext.chars().all(legal_short)
        && !name[..name.len() - if ext.is_empty() { 0 } else { ext.len() + 1 }].contains('.');
    let make = |b: &str, e: &str| {
        let mut out = [b' '; 11];
        for (i, c) in b.bytes().take(8).enumerate() {
            out[i] = c;
        }
        for (i, c) in e.bytes().take(3).enumerate() {
            out[8 + i] = c;
        }
        out
    };
    if exact {
        return (make(base, ext), false);
    }
    let clean = |s: &str| -> String { s.chars().filter(|c| *c != ' ' && *c != '.').map(|c| c.to_ascii_uppercase()).map(|c| if legal_short(c) { c } else { '_' }).collect() };
    let b = clean(base);
    let e = clean(ext);
    let e: String = e.chars().take(3).collect();
    for n in 1..1_000_000u32 {
        let tail = alloc::format!("~{n}");
        let keep = 8 - tail.len();
        let stem: String = b.chars().take(keep).collect();
        let cand = make(&alloc::format!("{stem}{tail}"), &e);
        if !taken.contains(&cand) {
            return (cand, true);
        }
    }
    (make("ERROR~", ""), true)
}

fn lfn_checksum(short: &[u8; 11]) -> u8 {
    short.iter().fold(0u8, |sum, &c| ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(c))
}

impl<'r, 'd> Fat32Writer<'r, 'd> {
    /// Formats the whole `region` as FAT32 and writes boot structures.
    pub fn format(r: &'r mut Region<'d>, opts: Options) -> Result<Self> {
        let ss = r.sector_size();
        let total = r.len() / ss as u64;
        if total > u32::MAX as u64 {
            return Err(Error::Size("FAT32 volume too large"));
        }
        let total_sectors = total as u32;
        // Largest cluster (up to 32 KiB) that still leaves at least 65525 clusters.
        let mut spc = 1;
        for cand in [64u32, 32, 16, 8, 4, 2, 1] {
            if cand * ss > 32 * 1024 {
                continue;
            }
            let approx = (total_sectors.saturating_sub(RESERVED_SECTORS + 2048)) / cand;
            if approx >= 65525 + 64 {
                spc = cand;
                break;
            }
        }
        // Prefer 4 KiB clusters on big volumes.
        let spc = spc.min((4096 / ss).max(1)).max(1);
        let mut fat_sectors = 1;
        let clusters = loop {
            let data = total_sectors - RESERVED_SECTORS - 2 * fat_sectors;
            let clusters = data / spc;
            let need = ((clusters + 2) * 4).div_ceil(ss);
            if need <= fat_sectors {
                break clusters;
            }
            fat_sectors = need;
        };
        if clusters < 65525 {
            return Err(Error::Size("volume too small for FAT32"));
        }
        let mut fat = vec![0u32; clusters as usize + 2];
        fat[0] = 0x0FFF_FFF8;
        fat[1] = EOC;
        fat[2] = EOC; // root directory
        let root = Dir { parent: 0, cluster: 2, entries: Vec::new() };
        let mut w = Fat32Writer { r, ss, spc, fat_sectors, total_sectors, clusters, fat, next_cluster: 3, dirs: vec![root], opts };
        w.write_boot_sectors()?;
        Ok(w)
    }

    fn write_boot_sectors(&mut self) -> Result<()> {
        let ss = self.ss as usize;
        let mut b = vec![0u8; ss];
        b[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
        b[3..11].copy_from_slice(b"ARCHSTLR");
        b[11..13].copy_from_slice(&(ss as u16).to_le_bytes());
        b[13] = self.spc as u8;
        b[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
        b[16] = 2;
        b[21] = 0xf8;
        b[24..26].copy_from_slice(&63u16.to_le_bytes());
        b[26..28].copy_from_slice(&255u16.to_le_bytes());
        b[28..32].copy_from_slice(&self.opts.hidden_sectors.to_le_bytes());
        b[32..36].copy_from_slice(&self.total_sectors.to_le_bytes());
        b[36..40].copy_from_slice(&self.fat_sectors.to_le_bytes());
        b[44..48].copy_from_slice(&2u32.to_le_bytes()); // root cluster
        b[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo
        b[50..52].copy_from_slice(&6u16.to_le_bytes()); // backup boot sector
        b[64] = 0x80;
        b[66] = 0x29;
        b[67..71].copy_from_slice(&self.opts.volume_id.to_le_bytes());
        let mut label = [b' '; 11];
        for (i, c) in self.opts.label.to_ascii_uppercase().bytes().take(11).enumerate() {
            label[i] = c;
        }
        b[71..82].copy_from_slice(&label);
        b[82..90].copy_from_slice(b"FAT32   ");
        b[510] = 0x55;
        b[511] = 0xaa;
        self.r.write_at(0, &b)?;
        self.r.write_at(6 * ss as u64, &b)?;
        Ok(())
    }

    fn write_fsinfo(&mut self) -> Result<()> {
        let ss = self.ss as usize;
        let mut b = vec![0u8; ss];
        b[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
        b[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
        let free = self.clusters + 2 - self.next_cluster;
        b[488..492].copy_from_slice(&free.to_le_bytes());
        b[492..496].copy_from_slice(&self.next_cluster.to_le_bytes());
        b[508..512].copy_from_slice(&[0, 0, 0x55, 0xaa]);
        self.r.write_at(ss as u64, &b)?;
        self.r.write_at(7 * ss as u64, &b)
    }

    fn cluster_bytes(&self) -> u64 {
        self.spc as u64 * self.ss as u64
    }

    fn cluster_offset(&self, cluster: u32) -> u64 {
        (RESERVED_SECTORS as u64 + 2 * self.fat_sectors as u64) * self.ss as u64 + (cluster as u64 - 2) * self.cluster_bytes()
    }

    /// Reserves `count` contiguous clusters and links them into a chain.
    fn alloc_chain(&mut self, count: u32) -> Result<u32> {
        let first = self.next_cluster;
        if first as u64 + count as u64 > self.clusters as u64 + 2 {
            return Err(Error::NoSpace);
        }
        for i in 0..count {
            self.fat[(first + i) as usize] = if i + 1 == count { EOC } else { first + i + 1 };
        }
        self.next_cluster += count;
        Ok(first)
    }

    fn find_dir(&self, dir: usize, name: &str) -> Option<&Ent> {
        self.dirs[dir].entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// Returns the directory index for `path` (created as needed).
    pub fn mkdir_all(&mut self, path: &str) -> Result<usize> {
        let mut cur = 0;
        for c in path.split('/').filter(|c| !c.is_empty()) {
            cur = match self.find_dir(cur, c) {
                Some(Ent { dir: Some(d), .. }) => *d,
                Some(_) => return Err(Error::Exists),
                None => {
                    let idx = self.dirs.len();
                    self.dirs.push(Dir { parent: cur, cluster: 0, entries: Vec::new() });
                    self.dirs[cur].entries.push(Ent { name: c.into(), first_cluster: 0, size: 0, dir: Some(idx) });
                    idx
                }
            };
        }
        Ok(cur)
    }

    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<()> {
        let (dir_path, name) = path.trim_matches('/').rsplit_once('/').unwrap_or(("", path.trim_matches('/')));
        if name.is_empty() || name.len() > 255 {
            return Err(Error::Invalid("bad file name"));
        }
        let dir = self.mkdir_all(dir_path)?;
        if self.find_dir(dir, name).is_some() {
            return Err(Error::Exists);
        }
        if data.len() as u64 > u32::MAX as u64 {
            return Err(Error::Size("file over 4 GiB"));
        }
        let mut first = 0;
        if !data.is_empty() {
            let clusters = (data.len() as u64).div_ceil(self.cluster_bytes()) as u32;
            first = self.alloc_chain(clusters)?;
            let off = self.cluster_offset(first);
            let padded = (clusters as u64 * self.cluster_bytes()) as usize;
            if padded == data.len() {
                self.r.write_at(off, data)?;
            } else {
                let full = data.len() / self.ss as usize * self.ss as usize;
                self.r.write_at(off, &data[..full])?;
                let mut tail = vec![0u8; self.ss as usize];
                tail[..data.len() - full].copy_from_slice(&data[full..]);
                self.r.write_at(off + full as u64, &tail)?;
            }
        }
        self.dirs[dir].entries.push(Ent { name: name.into(), first_cluster: first, size: data.len() as u32, dir: None });
        Ok(())
    }

    fn serialize_dir(&self, idx: usize) -> Vec<u8> {
        let d = &self.dirs[idx];
        let (date, time) = fat_time(self.opts.now);
        let mut out: Vec<u8> = Vec::new();
        let put_short = |out: &mut Vec<u8>, short: &[u8; 11], attr: u8, cluster: u32, size: u32| {
            let mut e = [0u8; 32];
            e[0..11].copy_from_slice(short);
            e[11] = attr;
            e[14..16].copy_from_slice(&time.to_le_bytes());
            e[16..18].copy_from_slice(&date.to_le_bytes());
            e[18..20].copy_from_slice(&date.to_le_bytes());
            e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
            e[22..24].copy_from_slice(&time.to_le_bytes());
            e[24..26].copy_from_slice(&date.to_le_bytes());
            e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
            e[28..32].copy_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&e);
        };
        if idx == 0 {
            let mut label = [b' '; 11];
            for (i, c) in self.opts.label.to_ascii_uppercase().bytes().take(11).enumerate() {
                label[i] = c;
            }
            put_short(&mut out, &label, 0x08, 0, 0);
        } else {
            let parent_cluster = if d.parent == 0 { 0 } else { self.dirs[d.parent].cluster };
            put_short(&mut out, b".          ", 0x10, d.cluster, 0);
            put_short(&mut out, b"..         ", 0x10, parent_cluster, 0);
        }
        let mut taken: Vec<[u8; 11]> = Vec::new();
        for e in &d.entries {
            let (short, needs_lfn) = short_name(&e.name, &taken);
            taken.push(short);
            if needs_lfn {
                let units: Vec<u16> = e.name.encode_utf16().collect();
                let sum = lfn_checksum(&short);
                let n = units.len().div_ceil(13);
                for seq in (1..=n).rev() {
                    let mut le = [0u8; 32];
                    le[0] = seq as u8 | if seq == n { 0x40 } else { 0 };
                    le[11] = 0x0f;
                    le[13] = sum;
                    let mut chars = [0xffffu16; 13];
                    for (i, c) in chars.iter_mut().enumerate() {
                        let pos = (seq - 1) * 13 + i;
                        if pos < units.len() {
                            *c = units[pos];
                        } else if pos == units.len() {
                            *c = 0;
                        }
                    }
                    let offs = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
                    for (c, o) in chars.iter().zip(offs) {
                        le[o..o + 2].copy_from_slice(&c.to_le_bytes());
                    }
                    out.extend_from_slice(&le);
                }
            }
            let cluster = match e.dir {
                Some(di) => self.dirs[di].cluster,
                None => e.first_cluster,
            };
            put_short(&mut out, &short, if e.dir.is_some() { 0x10 } else { 0x20 }, cluster, e.size);
        }
        out
    }

    /// Allocates directory clusters, writes directories, both FATs and the FSInfo sector.
    pub fn finish(mut self) -> Result<()> {
        // Cluster counts depend only on entry counts, so allocate all directories first
        // (their cluster numbers appear in child entries).
        for idx in 1..self.dirs.len() {
            let bytes = self.serialize_dir(idx).len() as u64 + 32;
            let clusters = bytes.div_ceil(self.cluster_bytes()).max(1) as u32;
            let c = self.alloc_chain(clusters)?;
            self.dirs[idx].cluster = c;
        }
        for idx in 0..self.dirs.len() {
            let mut data = self.serialize_dir(idx);
            let clusters = ((data.len() as u64 + 32).div_ceil(self.cluster_bytes())).max(1);
            data.resize((clusters * self.cluster_bytes()) as usize, 0);
            if idx == 0 {
                // The root's chain must be long enough as well.
                let extra = clusters as u32 - 1;
                if extra > 0 {
                    let more = self.alloc_chain(extra)?;
                    self.fat[2] = more;
                    // Root clusters are not contiguous; write them one by one.
                    let cb = self.cluster_bytes() as usize;
                    self.r.write_at(self.cluster_offset(2), &data[..cb])?;
                    let off = self.cluster_offset(more);
                    self.r.write_at(off, &data[cb..])?;
                    continue;
                }
            }
            let start = self.dirs[idx].cluster;
            self.r.write_at(self.cluster_offset(start), &data)?;
        }
        let mut bytes = vec![0u8; self.fat_sectors as usize * self.ss as usize];
        for (i, e) in self.fat.iter().enumerate() {
            bytes[4 * i..4 * i + 4].copy_from_slice(&e.to_le_bytes());
        }
        let fat0 = RESERVED_SECTORS as u64 * self.ss as u64;
        self.r.write_at(fat0, &bytes)?;
        self.r.write_at(fat0 + bytes.len() as u64, &bytes)?;
        self.write_fsinfo()?;
        self.r.flush()
    }
}
