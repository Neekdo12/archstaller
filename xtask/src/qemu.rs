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
    cmd.arg("-cdrom").arg(iso);
    run(&mut cmd)
}
