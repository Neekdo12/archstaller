use crate::{keyring, limine, lua, root, run, Options, Result};
use std::path::PathBuf;
use std::process::Command;

const TARGET: &str = "x86_64-unknown-none";

const LIMINE_CONF: &str = "timeout: 0\n\n/archstaler\n    protocol: limine\n    path: boot():/boot/kernel\n    module_path: boot():/boot/config.bin\n    module_path: boot():/boot/keyring.bin\n    module_path: boot():/boot/tiny-init\n    module_path: boot():/boot/limine-bios-hdd.bin\n    module_path: boot():/boot/limine/limine-bios.sys\n    module_path: boot():/EFI/BOOT/BOOTX64.EFI\n";

/// `--super-small`: `boot/kernel` is the `kstub` loader, which unpacks the module `kernel.z`; the
/// kernel finds BOOTX64.EFI inside the `efi.img` module (its cmdline is `offset:length`).
fn super_small_conf(efi_range: (usize, usize)) -> String {
    format!(
        "timeout: 0\n\n/archstaler\n    protocol: limine\n    path: boot():/boot/kernel\n    module_path: boot():/boot/kernel.z\n    module_path: boot():/boot/config.bin\n    module_path: boot():/boot/keyring.bin\n    module_path: boot():/boot/tiny-init\n    module_path: boot():/boot/limine-bios-hdd.bin\n    module_path: boot():/boot/limine/limine-bios.sys\n    module_path: boot():/boot/limine/efi.img\n    module_cmdline: {}:{}\n",
        efi_range.0, efi_range.1
    )
}

/// Must equal PAYLOAD_SIZE in kstub/linker.ld and kstub/src/main.rs.
const PAYLOAD_SIZE: u64 = 4 << 20;

fn le16(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap()) as usize
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Flattens the kernel ELF into the memory image its segments would form, compresses it, and
/// prepends what `kstub` needs: `[entry][image length][raw deflate]`.
fn kernel_blob(elf: &[u8]) -> Result<Vec<u8>> {
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
    if base != 0xffff_ffff_8000_0000 {
        return Err(format!("kernel is linked at {base:#x}, kstub expects 0xffffffff80000000").into());
    }
    let mem_end = loads.iter().map(|l| l.0 + l.3).max().unwrap() - base;
    if mem_end > PAYLOAD_SIZE {
        return Err(format!("kernel needs {mem_end} bytes of memory, kstub reserves {PAYLOAD_SIZE}").into());
    }
    let len = loads.iter().map(|l| (l.0 - base) as usize + l.2).max().unwrap();
    let mut image = vec![0u8; len];
    for &(vaddr, off, filesz, _) in &loads {
        let at = (vaddr - base) as usize;
        image[at..at + filesz].copy_from_slice(&elf[off..off + filesz]);
    }
    let packed = miniz_oxide::deflate::compress_to_vec(&image, 10);
    if miniz_oxide::inflate::decompress_to_vec(&packed).map_err(|e| format!("{e:?}"))? != image {
        return Err("compressed kernel does not round-trip".into());
    }
    let mut blob = Vec::with_capacity(16 + packed.len());
    blob.extend_from_slice(&entry.to_le_bytes());
    blob.extend_from_slice(&(len as u64).to_le_bytes());
    blob.extend_from_slice(&packed);
    println!("kernel image {len} bytes -> kernel.z {} bytes", blob.len());
    Ok(blob)
}

/// Builds the loader stub that goes in place of the kernel in `--super-small` ISOs.
fn build_stub() -> Result<PathBuf> {
    run(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())).current_dir(root()).args([
        "build", "-p", "kstub", "--target", TARGET, "--profile", "small",
        "-Z", "build-std=core,compiler_builtins", "-Z", "build-std-features=compiler-builtins-mem",
    ]))?;
    Ok(root().join("target").join(TARGET).join("small/kstub"))
}

/// Where `needle` sits inside `hay` (sector aligned, the way `mcopy` lays out a file in a new image).
fn find_in_image(hay: &[u8], needle: &[u8]) -> Result<usize> {
    (0..hay.len().saturating_sub(needle.len() - 1))
        .step_by(512)
        .find(|&off| hay[off..off + needle.len()] == *needle)
        .ok_or_else(|| "BOOTX64.EFI is not stored contiguously in efi.img".into())
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

/// A FAT12 image holding only BOOTX64.EFI, sized to fit (Limine's stock image is 3 MiB).
fn make_efi_image(efi: &std::path::Path, out: &std::path::Path, slack_sectors: u64) -> Result<()> {
    let sectors = std::fs::metadata(efi)?.len().div_ceil(512) + slack_sectors;
    let _ = std::fs::remove_file(out);
    run(Command::new("mformat").args(["-C", "-T", &sectors.to_string(), "-v", "ESP", "-i"]).arg(out).arg("::"))?;
    run(Command::new("mmd").arg("-i").arg(out).args(["::/EFI", "::/EFI/BOOT"]))?;
    run(Command::new("mcopy").arg("-i").arg(out).arg(efi).arg("::/EFI/BOOT/BOOTX64.EFI"))?;
    Ok(())
}

pub fn iso_path() -> PathBuf {
    root().join("target/archstaler.iso")
}

pub fn build(opts: &Options) -> Result<PathBuf> {
    let kernel = build_kernel(opts)?;
    let config_bin = lua::eval_config(&opts.config, &opts.extra_kernel_params)?;
    let keyring_blob = keyring::build()?;
    run(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root())
        .args(["build", "-p", "tiny-init", "--bin", "tiny-init", "--release", "--target", TARGET]))?;
    let tiny_init = std::fs::read(root().join("target").join(TARGET).join("release/tiny-init"))?;
    let lim = limine::fetch()?;

    let tree = root().join("target/iso_root");
    let _ = std::fs::remove_dir_all(&tree);
    std::fs::create_dir_all(tree.join("boot/limine"))?;
    if opts.super_small {
        std::fs::copy(build_stub()?, tree.join("boot/kernel"))?;
        std::fs::write(tree.join("boot/kernel.z"), kernel_blob(&std::fs::read(&kernel)?)?)?;
    } else {
        std::fs::copy(&kernel, tree.join("boot/kernel"))?;
    }
    std::fs::write(tree.join("boot/config.bin"), config_bin)?;
    std::fs::write(tree.join("boot/keyring.bin"), keyring_blob)?;
    std::fs::write(tree.join("boot/tiny-init"), tiny_init)?;
    let hdd = disk::limine::parse_hdd_header(&std::fs::read_to_string(lim.file("limine-bios-hdd.h"))?);
    std::fs::write(tree.join("boot/limine-bios-hdd.bin"), hdd)?;
    for f in ["limine-bios.sys", "limine-bios-cd.bin"] {
        std::fs::copy(lim.file(f), tree.join("boot/limine").join(f))?;
    }
    // Normally BOOTX64.EFI is on the ISO twice: inside efi.img (for the firmware) and loose (a module
    // the kernel copies to the target's ESP). --super-small keeps only the copy in efi.img.
    let efi_img = tree.join("boot/limine/efi.img");
    let conf = if opts.super_small {
        make_efi_image(&lim.file("BOOTX64.EFI"), &efi_img, 40)?;
        let efi = std::fs::read(lim.file("BOOTX64.EFI"))?;
        super_small_conf((find_in_image(&std::fs::read(&efi_img)?, &efi)?, efi.len()))
    } else {
        std::fs::create_dir_all(tree.join("EFI/BOOT"))?;
        std::fs::copy(lim.file("BOOTX64.EFI"), tree.join("EFI/BOOT/BOOTX64.EFI"))?;
        make_efi_image(&lim.file("BOOTX64.EFI"), &efi_img, 96)?;
        LIMINE_CONF.to_string()
    };
    std::fs::write(tree.join("boot/limine/limine.conf"), conf)?;

    let iso = opts.out.clone().unwrap_or_else(iso_path);
    if let Some(dir) = iso.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // libvirt may have taken ownership of a previous ISO; unlinking works, overwriting does not.
    let _ = std::fs::remove_file(&iso);
    run(Command::new("xorriso")
        .args(["-as", "mkisofs", "-R", "-r", "-no-pad"])
        .args(["-b", "boot/limine/limine-bios-cd.bin"])
        .args(["-no-emul-boot", "-boot-load-size", "4", "-boot-info-table"])
        .args(["--efi-boot", "boot/limine/efi.img"])
        .args(["-efi-boot-part", "--efi-boot-image", "--protective-msdos-label"])
        .arg(&tree)
        .arg("-o")
        .arg(&iso))?;
    run(Command::new(lim.tool()).arg("bios-install").arg(&iso))?;
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
