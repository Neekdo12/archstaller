use crate::{keyring, limine, lua, root, run, Options, Result};
use std::path::PathBuf;
use std::process::Command;

const TARGET: &str = "x86_64-unknown-none";

const LIMINE_CONF: &str = "timeout: 0\n\n/archstaler\n    protocol: limine\n    path: boot():/boot/kernel\n    module_path: boot():/boot/config.bin\n    module_path: boot():/boot/keyring.bin\n    module_path: boot():/boot/tiny-init\n    module_path: boot():/boot/limine-bios-hdd.bin\n    module_path: boot():/boot/limine/limine-bios.sys\n    module_path: boot():/EFI/BOOT/BOOTX64.EFI\n";

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
    cmd.args(["--features", &features.join(",")]);
    run(&mut cmd)?;
    Ok(kernel_path(opts))
}

fn kernel_path(opts: &Options) -> PathBuf {
    root().join("target").join(TARGET).join(if opts.small { "small/kernel" } else { "release/kernel" })
}

/// A FAT12 image holding only BOOTX64.EFI, sized to fit (Limine's stock image is 3 MiB).
fn make_efi_image(efi: &std::path::Path, out: &std::path::Path) -> Result<()> {
    let sectors = std::fs::metadata(efi)?.len().div_ceil(512) + 96;
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
    let config_bin = lua::eval_config(&opts.config)?;
    let keyring_blob = keyring::build()?;
    run(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root())
        .args(["build", "-p", "tiny-init", "--bin", "tiny-init", "--release", "--target", TARGET]))?;
    let tiny_init = std::fs::read(root().join("target").join(TARGET).join("release/tiny-init"))?;
    let lim = limine::fetch()?;

    let tree = root().join("target/iso_root");
    let _ = std::fs::remove_dir_all(&tree);
    std::fs::create_dir_all(tree.join("boot/limine"))?;
    std::fs::create_dir_all(tree.join("EFI/BOOT"))?;
    std::fs::copy(&kernel, tree.join("boot/kernel"))?;
    std::fs::write(tree.join("boot/config.bin"), config_bin)?;
    std::fs::write(tree.join("boot/keyring.bin"), keyring_blob)?;
    std::fs::write(tree.join("boot/tiny-init"), tiny_init)?;
    let hdd = disk::limine::parse_hdd_header(&std::fs::read_to_string(lim.file("limine-bios-hdd.h"))?);
    std::fs::write(tree.join("boot/limine-bios-hdd.bin"), hdd)?;
    std::fs::write(tree.join("boot/limine/limine.conf"), LIMINE_CONF)?;
    for f in ["limine-bios.sys", "limine-bios-cd.bin"] {
        std::fs::copy(lim.file(f), tree.join("boot/limine").join(f))?;
    }
    std::fs::copy(lim.file("BOOTX64.EFI"), tree.join("EFI/BOOT/BOOTX64.EFI"))?;
    make_efi_image(&lim.file("BOOTX64.EFI"), &tree.join("boot/limine/efi.img"))?;

    let iso = iso_path();
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
