use crate::{root, run, Options, Result};
use std::path::Path;
use std::process::Command;

const OVMF_DIR: &str = "/usr/share/edk2/x64";

pub fn run_iso(iso: &Path, opts: &Options) -> Result<()> {
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-machine", "q35", "-m", "512M", "-serial", "stdio", "-no-reboot"]);
    if Path::new("/dev/kvm").exists() {
        cmd.args(["-enable-kvm", "-cpu", "host"]);
    }
    if opts.headless {
        cmd.args(["-display", "none"]);
    }
    if opts.uefi {
        let vars = root().join("target/ovmf_vars.fd");
        std::fs::copy(format!("{OVMF_DIR}/OVMF_VARS.4m.fd"), &vars)?;
        cmd.arg("-drive")
            .arg(format!("if=pflash,format=raw,readonly=on,file={OVMF_DIR}/OVMF_CODE.4m.fd"));
        cmd.arg("-drive").arg(format!("if=pflash,format=raw,file={}", vars.display()));
    }
    cmd.args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]);
    cmd.arg("-cdrom").arg(iso);

    // Blank test disk; the serial lets the installer's confirm_serial check be exercised.
    let disk = root().join("target/test-disk.img");
    if !disk.exists() {
        let f = std::fs::File::create(&disk)?;
        f.set_len(1 << 30)?;
    }
    cmd.arg("-drive").arg(format!("if=none,id=d0,format=raw,file={}", disk.display()));
    match opts.disk.as_str() {
        "virtio" => cmd.args(["-device", "virtio-blk-pci,drive=d0,serial=TESTDISK0"]),
        "ahci" => cmd.args(["-device", "ich9-ahci,id=ahci", "-device", "ide-hd,drive=d0,bus=ahci.0,serial=TESTDISK0"]),
        "nvme" => cmd.args(["-device", "nvme,drive=d0,serial=TESTDISK0"]),
        other => return Err(format!("unknown disk type {other}").into()),
    };
    cmd.args(["-netdev", "user,id=n0"]);
    match opts.nic.as_str() {
        "virtio" => cmd.args(["-device", "virtio-net-pci,netdev=n0,disable-legacy=on"]),
        "e1000" => cmd.args(["-device", "e1000,netdev=n0"]),
        "e1000e" => cmd.args(["-device", "e1000e,netdev=n0"]),
        "rtl8139" => cmd.args(["-device", "rtl8139,netdev=n0"]),
        other => return Err(format!("unknown nic type {other}").into()),
    };
    let status = cmd.status()?;
    // isa-debug-exit turns the kernel's `out 0xf4, 0` into exit code 1.
    match status.code() {
        Some(0) | Some(1) => Ok(()),
        _ => Err(format!("{cmd:?} failed: {status}").into()),
    }
}
