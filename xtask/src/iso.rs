use crate::{keyring, limine, lua, root, run, Options, Result};
use std::path::PathBuf;
use std::process::Command;

const TARGET: &str = "x86_64-unknown-none";

const LIMINE_CONF: &str = "timeout: 0\n\n/archstaler\n    protocol: limine\n    path: boot():/boot/kernel\n    module_path: boot():/boot/config.bin\n    module_path: boot():/boot/keyring.bin\n    module_path: boot():/boot/tiny-init\n    module_path: boot():/boot/limine-bios-hdd.bin\n    module_path: boot():/boot/limine/limine-bios.sys\n    module_path: boot():/EFI/BOOT/BOOTX64.EFI\n";

fn build_kernel(opts: &Options) -> Result<PathBuf> {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root()).args(["build", "-p", "kernel", "--release", "--target", TARGET]);
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
    Ok(root().join("target").join(TARGET).join("release/kernel"))
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
    for f in ["limine-bios.sys", "limine-bios-cd.bin", "limine-uefi-cd.bin"] {
        std::fs::copy(lim.file(f), tree.join("boot/limine").join(f))?;
    }
    std::fs::copy(lim.file("BOOTX64.EFI"), tree.join("EFI/BOOT/BOOTX64.EFI"))?;

    let iso = iso_path();
    run(Command::new("xorriso")
        .args(["-as", "mkisofs", "-R", "-r", "-no-pad"])
        .args(["-b", "boot/limine/limine-bios-cd.bin"])
        .args(["-no-emul-boot", "-boot-load-size", "4", "-boot-info-table"])
        .args(["--efi-boot", "boot/limine/limine-uefi-cd.bin"])
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
    let kernel = root().join("target").join(TARGET).join("release/kernel");
    println!("kernel: {} bytes", std::fs::metadata(kernel)?.len());
    println!("iso:    {} bytes", std::fs::metadata(iso)?.len());
    Ok(())
}
