//! The BIOS boot chain: building it and telling it where things lie in the finished ISO.
use crate::Result;

pub struct Bios {
    pub stage1: [u8; 512],
    pub stage2: Vec<u8>,
}

const TARGET: &str = "x86_64-unknown-none";

/// The memory image of an ELF's loadable segments from `base`.
fn flatten(elf: &[u8], base: u64) -> Result<Vec<u8>> {
    let u16le = |at: usize| -> Result<usize> {
        let bytes: [u8; 2] = elf
            .get(at..at + 2)
            .ok_or("ELF too short for u16")?
            .try_into()
            .map_err(|_e| "failed to convert u16 slice")?;
        Ok(u16::from_le_bytes(bytes) as usize)
    };
    let u32le = |at: usize| -> Result<u32> {
        let bytes: [u8; 4] = elf
            .get(at..at + 4)
            .ok_or("ELF too short for u32")?
            .try_into()
            .map_err(|_e| "failed to convert u32 slice")?;
        Ok(u32::from_le_bytes(bytes))
    };
    let u64le = |at: usize| -> Result<u64> {
        let bytes: [u8; 8] = elf
            .get(at..at + 8)
            .ok_or("ELF too short for u64")?
            .try_into()
            .map_err(|_e| "failed to convert u64 slice")?;
        Ok(u64::from_le_bytes(bytes))
    };
    let (phoff, phentsize, phnum) = (u64le(0x20)? as usize, u16le(0x36)?, u16le(0x38)?);
    let mut image = Vec::new();
    for ph in (0..phnum)
        .map(|i| phoff + i * phentsize)
        .filter(|&ph| u32le(ph).is_ok_and(|v| v == 1))
    {
        let (off, vaddr, filesz) = (
            u64le(ph + 8)? as usize,
            u64le(ph + 16)?,
            u64le(ph + 32)? as usize,
        );
        // A segment may start below `base` (it then maps the ELF headers): keep what is at or above it.
        let skip = base.saturating_sub(vaddr) as usize;
        if skip >= filesz {
            continue;
        }
        let at = (vaddr + skip as u64 - base) as usize;
        let n = filesz - skip;
        if image.len() < at + n {
            image.resize(at + n, 0);
        }
        let src = elf
            .get(off + skip..off + skip + n)
            .ok_or("ELF segment out of bounds")?;
        let dst = image
            .get_mut(at..at + n)
            .ok_or("image buffer too small")?;
        dst.copy_from_slice(src);
    }
    Ok(image)
}

fn cargo_build(package: &str) -> Result<Vec<u8>> {
    crate::run(
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .current_dir(crate::root())
            .args([
                "build",
                "-p",
                package,
                "--target",
                TARGET,
                "--profile",
                "small",
                "-Z",
                "build-std=core,compiler_builtins",
                "-Z",
                "build-std-features=compiler-builtins-mem",
            ]),
    )?;
    Ok(std::fs::read(
        crate::target_dir().join(TARGET).join("small").join(package),
    )?)
}

/// Builds stage 1 (exactly 512 bytes) and stage 2 (padded to whole sectors).
pub fn build() -> Result<Bios> {
    let s1 = flatten(&cargo_build("boot-bios-s1")?, 0x7c00)?;
    if s1.len() > 440 || s1.len() < bootinfo::bios::S1_PATCH + 12 {
        return Err(format!(
            "BIOS stage 1 is {} bytes, it must be {}..=440",
            s1.len(),
            bootinfo::bios::S1_PATCH + 12
        )
        .into());
    }
    let mut stage1 = [0u8; 512];
    if let Some(dest) = stage1.get_mut(..s1.len()) {
        dest.copy_from_slice(&s1);
    }
    stage1[510] = 0x55;
    stage1[511] = 0xaa;
    let mut stage2 = flatten(&cargo_build("boot-bios")?, bootinfo::bios::S2_BASE as u64)?;
    stage2.resize(stage2.len().next_multiple_of(512), 0);
    println!(
        "BIOS stage 1: {} of 424 bytes, stage 2: {} bytes",
        s1.iter()
            .rposition(|&b| b != 0)
            .map_or(0, |p| p + 1)
            .min(424),
        stage2.len()
    );
    Ok(Bios { stage1, stage2 })
}

fn le32(b: &[u8], at: usize) -> u32 {
    let bytes: [u8; 4] = b
        .get(at..at + 4)
        .and_then(|s| s.try_into().ok())
        .unwrap_or([0; 4]);
    u32::from_le_bytes(bytes)
}

/// Finds a file in the ISO 9660 tree: (byte offset of its data, length).
pub fn iso_extent(img: &[u8], path: &str) -> Option<(u64, u64)> {
    const SECTOR: usize = 2048;
    // Primary volume descriptor at sector 16; the root directory record is at offset 156.
    let pvd = img.get(16 * SECTOR..17 * SECTOR)?;
    if pvd.get(1..6) != Some(b"CD001".as_slice()) {
        return None;
    }
    let (mut lba, mut len) = (le32(pvd, 156 + 2) as usize, le32(pvd, 156 + 10) as usize);
    let parts: Vec<&str> = path.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        let dir = img.get(lba * SECTOR..lba * SECTOR + len)?;
        let mut at = 0;
        let mut found = None;
        while at < dir.len() {
            let rec = *dir.get(at)? as usize;
            if rec == 0 {
                at = (at / SECTOR + 1) * SECTOR; // records do not cross sector boundaries
                continue;
            }
            let name_len = *dir.get(at + 32)? as usize;
            let raw_name = dir.get(at + 33..at + 33 + name_len)?;
            let name =
                String::from_utf8_lossy(raw_name).to_ascii_lowercase();
            let name = name
                .split(';')
                .next()?
                .trim_end_matches('.')
                .to_string();
            if name == *part {
                found = Some((le32(dir, at + 2) as usize, le32(dir, at + 10) as usize));
                break;
            }
            at += rec;
        }
        let (l, n) = found?;
        if i + 1 == parts.len() {
            return Some((l as u64 * SECTOR as u64, n as u64));
        }
        lba = l;
        len = n;
    }
    None
}

/// Patches the finished ISO: stage 1 into the El Torito image and the hybrid MBR, stage 2's header
/// with the payload's location. `bios_img` is where `boot/bios.img` (stage 1 followed by stage 2) lies.
pub fn patch_iso(
    img: &mut [u8],
    bios: &Bios,
    bios_img: u64,
    payload_off: u64,
    payload_len: u64,
) -> Result<()> {
    let stage2_off = bios_img + bootinfo::bios::S2_ALIGN as u64;
    let mut s1 = bios.stage1;
    bootinfo::bios::patch_stage1(&mut s1, stage2_off, bios.stage2.len() as u32);
    let mut s2 = bios.stage2.clone();
    if s2.len() >= bootinfo::bios::S2_HEADER + bootinfo::bios::S2_HEADER_LEN {
        bootinfo::bios::patch_stage2(
            &mut s2,
            bootinfo::bios::MODE_PAYLOAD,
            &[(payload_off, payload_len)],
            "",
        )?;
    }
    let dest_bios = img
        .get_mut(bios_img as usize..bios_img as usize + 512)
        .ok_or("ISO too small for BIOS stage 1")?;
    dest_bios.copy_from_slice(&s1);
    let dest_s2 = img
        .get_mut(stage2_off as usize..stage2_off as usize + s2.len())
        .ok_or("ISO too small for BIOS stage 2")?;
    dest_s2.copy_from_slice(&s2);
    // Hybrid image (dd to a USB stick): the same code in the MBR, which xorriso filled with a protective label.
    let s1_mbr = s1.get(..440).ok_or("stage 1 too short for MBR")?;
    let mbr = img.get_mut(..440).ok_or("ISO too small for MBR")?;
    mbr.copy_from_slice(s1_mbr);
    Ok(())
}
