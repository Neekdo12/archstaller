use crate::{keyring, lua, root, run, Options, Result};
use std::path::PathBuf;
use std::process::Command;

const TARGET: &str = "x86_64-unknown-none";
const UEFI_TARGET: &str = "x86_64-unknown-uefi";

fn le16(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap()) as usize
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// The kernel as the loader wants it: the memory image its segments form from `KERNEL_BASE`.
struct KernelImage {
    image: Vec<u8>,
    entry: u64,
    /// Bytes of memory the kernel needs, .bss included.
    mem: u64,
}

fn flatten_kernel(elf: &[u8]) -> Result<KernelImage> {
    if elf.get(..4) != Some(b"\x7fELF") {
        return Err("kernel is not an ELF file".into());
    }
    let entry = le64(elf, 0x18);
    let (phoff, phentsize, phnum) = (le64(elf, 0x20) as usize, le16(elf, 0x36), le16(elf, 0x38));
    let loads: Vec<(u64, usize, usize, u64)> = (0..phnum)
        .map(|i| phoff + i * phentsize)
        .filter(|&ph| le32(elf, ph) == 1)
        .map(|ph| (le64(elf, ph + 16), le64(elf, ph + 8) as usize, le64(elf, ph + 32) as usize, le64(elf, ph + 40)))
        .collect();
    let base = loads.iter().map(|l| l.0).min().ok_or("kernel has no loadable segments")?;
    if base != bootinfo::KERNEL_BASE {
        return Err(format!("kernel is linked at {base:#x}, the loader expects {:#x}", bootinfo::KERNEL_BASE).into());
    }
    let mem = loads.iter().map(|l| l.0 + l.3).max().unwrap() - base;
    if mem > bootinfo::KERNEL_MAX {
        return Err(format!("kernel needs {mem} bytes of memory, the loader maps {}", bootinfo::KERNEL_MAX).into());
    }
    let len = loads.iter().map(|l| (l.0 - base) as usize + l.2).max().unwrap();
    let mut image = vec![0u8; len];
    for &(vaddr, off, filesz, _) in &loads {
        let at = (vaddr - base) as usize;
        image[at..at + filesz].copy_from_slice(&elf[off..off + filesz]);
    }
    Ok(KernelImage { image, entry, mem })
}

fn deflate(data: &[u8]) -> Result<Vec<u8>> {
    let packed = miniz_oxide::deflate::compress_to_vec(data, 10);
    if miniz_oxide::inflate::decompress_to_vec(&packed).map_err(|e| format!("{e:?}"))? != data {
        return Err("compressed data does not round-trip".into());
    }
    Ok(packed)
}

/// The payload bundle (see `bootinfo`): the kernel image and the modules, each deflated when that helps.
fn make_payload(kernel: &KernelImage, modules: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let mut entries: Vec<(&str, &[u8])> = vec![("kernel", &kernel.image)];
    entries.extend_from_slice(modules);
    let head = bootinfo::PAYLOAD_HEADER + 16 + entries.len() * bootinfo::PAYLOAD_ENTRY;
    let mut out = vec![0u8; head];
    out[0..4].copy_from_slice(&bootinfo::PAYLOAD_MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    out[8..16].copy_from_slice(&kernel.entry.to_le_bytes());
    out[16..20].copy_from_slice(&(kernel.mem as u32).to_le_bytes());
    for (i, (name, data)) in entries.iter().enumerate() {
        if name.len() > 16 {
            return Err(format!("payload entry name {name} is too long").into());
        }
        let packed = deflate(data)?;
        let (stored, flags): (&[u8], u32) = if packed.len() < data.len() { (&packed, bootinfo::FLAG_DEFLATE) } else { (data, 0) };
        let at = bootinfo::PAYLOAD_HEADER + 16 + i * bootinfo::PAYLOAD_ENTRY;
        out[at..at + name.len()].copy_from_slice(name.as_bytes());
        let stored_at = out.len() as u32;
        out[at + 16..at + 20].copy_from_slice(&stored_at.to_le_bytes());
        out[at + 20..at + 24].copy_from_slice(&(stored.len() as u32).to_le_bytes());
        out[at + 24..at + 28].copy_from_slice(&(data.len() as u32).to_le_bytes());
        out[at + 28..at + 32].copy_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(stored);
        println!("payload: {name:<12} {:>8} -> {:>8} bytes", data.len(), stored.len());
    }
    Ok(out)
}

/// Builds the UEFI loader (`BOOTX64.EFI`).
fn build_uefi_loader() -> Result<Vec<u8>> {
    run(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())).current_dir(root()).args([
        "build", "-p", "boot-uefi", "--target", UEFI_TARGET, "--profile", "small",
        "-Z", "build-std=core,compiler_builtins", "-Z", "build-std-features=compiler-builtins-mem",
    ]))?;
    Ok(std::fs::read(root().join("target").join(UEFI_TARGET).join("small/boot-uefi.efi"))?)
}

/// Where `needle` sits inside `hay` (sector aligned, the way `mcopy` lays out a file in a new image).
fn find_in_image(hay: &[u8], needle: &[u8]) -> Result<usize> {
    (0..hay.len().saturating_sub(needle.len() - 1))
        .step_by(512)
        .find(|&off| hay[off..off + needle.len()] == *needle)
        .ok_or_else(|| "payload.bin is not stored contiguously in efi.img".into())
}

/// Verbose tracing is on with `--debug`, and always for a dry-run (hardware test) config, whose whole
/// purpose is to show what the hardware does.
fn wants_debug(opts: &Options) -> bool {
    opts.debug
        || std::fs::read_to_string(&opts.config)
            .is_ok_and(|t| t.lines().any(|l| l.trim_start().starts_with("dry_run") && l.contains("true")))
}

fn build_kernel(opts: &Options) -> Result<PathBuf> {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root()).args(["build", "-p", "kernel", "--target", TARGET]);
    if opts.small {
        cmd.args(["--profile", "small", "-Z", "build-std=core,alloc,compiler_builtins", "-Z", "build-std-features=compiler-builtins-mem"]);
    } else {
        cmd.arg("--release");
    }
    let mut features = vec![];
    if opts.selftest {
        features.push("disk-selftest");
        features.push("net-selftest");
    }
    if opts.fault_test {
        features.push("fault-test");
    }
    if opts.usb {
        features.push("usb-selftest");
    }
    if opts.tethering || opts.nic.starts_with("usb-") {
        features.push("usb-tethering");
    }
    if wants_debug(opts) {
        features.push("debug");
    }
    cmd.args(["--features", &features.join(",")]);
    run(&mut cmd)?;
    Ok(kernel_path(opts))
}

fn kernel_path(opts: &Options) -> PathBuf {
    root().join("target").join(TARGET).join(if opts.small { "small/kernel" } else { "release/kernel" })
}

/// A FAT12 image (the ESP the firmware boots from the ISO) holding the UEFI loader and the payload.
fn make_efi_image(efi: &std::path::Path, payload: &std::path::Path, out: &std::path::Path) -> Result<()> {
    let sectors = (std::fs::metadata(efi)?.len() + std::fs::metadata(payload)?.len()).div_ceil(512) + 64;
    let _ = std::fs::remove_file(out);
    run(Command::new("mformat").args(["-C", "-T", &sectors.to_string(), "-v", "ESP", "-i"]).arg(out).arg("::"))?;
    run(Command::new("mmd").arg("-i").arg(out).args(["::/EFI", "::/EFI/BOOT"]))?;
    run(Command::new("mcopy").arg("-i").arg(out).arg(efi).arg("::/EFI/BOOT/BOOTX64.EFI"))?;
    run(Command::new("mcopy").arg("-i").arg(out).arg(payload).arg("::/payload.bin"))?;
    Ok(())
}

pub fn iso_path() -> PathBuf {
    root().join("target/archstaler.iso")
}

pub fn build(opts: &Options) -> Result<PathBuf> {
    let kernel = flatten_kernel(&std::fs::read(build_kernel(opts)?)?)?;
    let config_bin = lua::eval_config(&opts.config, &opts.extra_kernel_params)?;
    let keyring_blob = keyring::build()?;
    run(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root())
        .args(["build", "-p", "tiny-init", "--bin", "tiny-init", "--release", "--target", TARGET]))?;
    let tiny_init = std::fs::read(root().join("target").join(TARGET).join("release/tiny-init"))?;
    let efi = build_uefi_loader()?;
    let bios = crate::bios::build()?;
    let mut bios_boot = bios.stage1.to_vec();
    bios_boot.extend_from_slice(&bios.stage2);

    let payload = make_payload(
        &kernel,
        &[("config.bin", &config_bin), ("keyring.bin", &keyring_blob), ("tiny-init", &tiny_init), ("bootx64.efi", &efi), ("bios-boot", &bios_boot)],
    )?;

    let tree = root().join("target/iso_root");
    let _ = std::fs::remove_dir_all(&tree);
    std::fs::create_dir_all(tree.join("boot"))?;
    let (efi_file, payload_file) = (root().join("target/BOOTX64.EFI"), root().join("target/payload.bin"));
    std::fs::write(&efi_file, &efi)?;
    std::fs::write(&payload_file, &payload)?;
    let efi_img = tree.join("boot/efi.img");
    make_efi_image(&efi_file, &payload_file, &efi_img)?;
    let payload_in_efi_img = find_in_image(&std::fs::read(&efi_img)?, &payload)?;
    let mut bios_img = bios.stage1.to_vec();
    bios_img.resize(bootinfo::bios::S2_ALIGN, 0);
    bios_img.extend_from_slice(&bios.stage2);
    std::fs::write(tree.join("boot/bios.img"), bios_img)?;

    let iso = opts.out.clone().unwrap_or_else(iso_path);
    if let Some(dir) = iso.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // libvirt may have taken ownership of a previous ISO; unlinking works, overwriting does not.
    let _ = std::fs::remove_file(&iso);
    run(Command::new("xorriso")
        .args(["-as", "mkisofs", "-R", "-r", "-no-pad"])
        .args(["-b", "boot/bios.img", "-no-emul-boot", "-boot-load-size", "4"])
        .args(["--efi-boot", "boot/efi.img"])
        .args(["-efi-boot-part", "--efi-boot-image", "--protective-msdos-label"])
        .arg(&tree)
        .arg("-o")
        .arg(&iso))?;

    // Tell the BIOS stages where things lie in the finished image.
    let mut img = std::fs::read(&iso)?;
    let extent = |path: &str| crate::bios::iso_extent(&img, path).ok_or_else(|| format!("{path} is not in the ISO"));
    let (bios_img, efi_extent) = (extent("boot/bios.img")?, extent("boot/efi.img")?);
    crate::bios::patch_iso(&mut img, &bios, bios_img.0, efi_extent.0 + payload_in_efi_img as u64, payload.len() as u64)?;
    std::fs::write(&iso, &img)?;
    println!("built {}", iso.display());
    Ok(iso)
}

pub fn size(opts: &Options) -> Result<()> {
    let iso = build(opts)?;
    let tree = root().join("target/iso_root");
    let mut files: Vec<(u64, String)> = Vec::new();
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<(u64, String)>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                walk(&p, base, out)?;
            } else {
                out.push((std::fs::metadata(&p)?.len(), p.strip_prefix(base).unwrap().display().to_string()));
            }
        }
        Ok(())
    }
    walk(&tree, &tree, &mut files)?;
    files.sort_by(|a, b| b.0.cmp(&a.0));
    println!("{:>10}  file (ISO contents)", "bytes");
    for (n, name) in &files {
        println!("{n:>10}  {name}");
    }
    let total = std::fs::metadata(&iso)?.len();
    let payload: u64 = files.iter().map(|f| f.0).sum();
    println!("{payload:>10}  payload total");
    println!("{total:>10}  ISO ({} bytes of image overhead)", total - payload.min(total));
    if total > opts.limit {
        return Err(format!("ISO is {total} bytes, over the limit of {} (use --limit to change)", opts.limit).into());
    }
    println!("within the limit of {} bytes", opts.limit);
    Ok(())
}
