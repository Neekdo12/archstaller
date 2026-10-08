//! End-to-end test: install onto a blank disk in QEMU, then boot the result twice.
use crate::{iso, root, Options, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const OVMF_DIR: &str = "/usr/share/edk2/x64";

fn qemu(opts: &Options, disk: &Path, cdrom: Option<&Path>, log: &Path) -> Result<Child> {
    // QEMU's user-mode network forwards DNS to the nameservers of the host's /etc/resolv.conf. A host that
    // resolves through systemd-resolved with an empty resolv.conf gives the guest no DNS at all, so run QEMU
    // in a mount namespace whose resolv.conf names a public resolver.
    let no_dns = !std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default().lines().any(|l| l.trim_start().starts_with("nameserver"));
    let mut cmd = if no_dns && Command::new("bwrap").arg("--version").output().is_ok() {
        let resolv = root().join("target/e2e/resolv.conf");
        std::fs::write(&resolv, "nameserver 8.8.8.8\nnameserver 1.1.1.1\n")?;
        let mut c = Command::new("bwrap");
        c.args(["--dev-bind", "/", "/", "--ro-bind"]).arg(&resolv).args(["/etc/resolv.conf", "qemu-system-x86_64"]);
        c
    } else {
        Command::new("qemu-system-x86_64")
    };
    // The "ide" disk needs the legacy i440fx board: q35 has only AHCI.
    cmd.args(["-machine", if opts.disk == "ide" { "pc" } else { "q35" }, "-m", "1536M", "-no-reboot", "-display", "none", "-monitor", "none"]);
    cmd.arg("-serial").arg(format!("file:{}", log.display()));
    if Path::new("/dev/kvm").exists() {
        cmd.args(["-enable-kvm", "-cpu", "host"]);
    }
    if opts.uefi {
        let vars = root().join("target/e2e/ovmf_vars.fd");
        if !vars.exists() {
            std::fs::copy(format!("{OVMF_DIR}/OVMF_VARS.4m.fd"), &vars)?;
        }
        cmd.arg("-drive").arg(format!("if=pflash,format=raw,readonly=on,file={OVMF_DIR}/OVMF_CODE.4m.fd"));
        cmd.arg("-drive").arg(format!("if=pflash,format=raw,file={}", vars.display()));
    }
    if let Some(iso) = cdrom {
        cmd.arg("-cdrom").arg(iso);
    }
    cmd.arg("-drive").arg(format!("if=none,id=d0,format=raw,file={}", disk.display()));
    match opts.disk.as_str() {
        "virtio" => cmd.args(["-device", "virtio-blk-pci,drive=d0,serial=TESTDISK0"]),
        "ahci" => cmd.args(["-device", "ich9-ahci,id=ahci", "-device", "ide-hd,drive=d0,bus=ahci.0,serial=TESTDISK0"]),
        "nvme" => cmd.args(["-device", "nvme,drive=d0,serial=TESTDISK0"]),
        "ide" => cmd.args(["-device", "ide-hd,drive=d0,bus=ide.0,serial=TESTDISK0"]),
        other => return Err(format!("unknown disk type {other}").into()),
    };
    cmd.args(["-netdev", "user,id=n0"]);
    match opts.nic.as_str() {
        "virtio" => cmd.args(["-device", "virtio-net-pci,netdev=n0,disable-legacy=on"]),
        "e1000" => cmd.args(["-device", "e1000,netdev=n0"]),
        "e1000e" => cmd.args(["-device", "e1000e,netdev=n0"]),
        "igb" => cmd.args(["-device", "igb,netdev=n0"]),
        // Any other QEMU PCI NIC model (vmxnet3, pcnet, i82559er, ne2k_pci, tulip, ...): `--nic <model>`.
        model if model.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') => cmd.args(["-device", &format!("{model},netdev=n0")]),
        other => return Err(format!("unknown nic type {other}").into()),
    };
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    Ok(cmd.spawn()?)
}

/// Waits until `marker` shows up in the log; fails on `abort` markers, exit or timeout.
fn wait_for(child: &mut Child, log: &Path, marker: &str, abort: &[&str], timeout: Duration) -> Result<String> {
    let start = Instant::now();
    let mut shown = 0;
    loop {
        let text = String::from_utf8_lossy(&std::fs::read(log).unwrap_or_default()).replace('\r', "");
        // Echo interesting new lines as they arrive.
        for line in text.lines().skip(shown) {
            if line.contains("archstaller") || line.starts_with('[') && line.contains('/') || line.contains("INSTALL") || line.contains("login:") || line.contains("Kernel panic") || line.contains("FAILED") || line.contains("FATAL") {
                println!("  | {line}");
            }
        }
        shown = text.lines().count();
        if text.contains(marker) {
            return Ok(text);
        }
        if let Some(a) = abort.iter().find(|a| text.contains(**a)) {
            let _ = child.kill();
            return Err(format!("aborting, saw '{a}' (log: {})", log.display()).into());
        }
        if let Ok(Some(status)) = child.try_wait() {
            // Final read after exit.
            let text = String::from_utf8_lossy(&std::fs::read(log).unwrap_or_default()).to_string();
            if text.contains(marker) {
                return Ok(text);
            }
            return Err(format!("QEMU exited ({status}) before '{marker}' (log: {})", log.display()).into());
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            return Err(format!("timeout waiting for '{marker}' (log: {})", log.display()).into());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

pub fn run_e2e(opts: &Options) -> Result<()> {
    let mode = &format!("{}-{}-{}", if opts.uefi { "uefi" } else { "bios" }, opts.disk, opts.nic);
    let dir = root().join("target/e2e");
    std::fs::create_dir_all(&dir)?;
    let default_cfg = root().join("configs/config.lua");
    // A preset can be tested with --config; otherwise use the serial-locked e2e config.
    let config = if opts.config != default_cfg { opts.config.clone() } else { root().join("configs/e2e.lua") };
    let o = Options { config: config.clone(), fault_test: false, selftest: false, profile: opts.profile, legacy_small: opts.legacy_small, limit: opts.limit, out: Some(dir.join("e2e.iso")), extra_kernel_params: vec!["console=ttyS0,115200".into()], uefi: opts.uefi, headless: true, disk: opts.disk.clone(), nic: opts.nic.clone(), usb: false, tethering: false, debug: opts.debug, progress: false, workdir: None, disk_gib: opts.disk_gib };
    let iso_path = iso::build(&o)?;

    let disk = dir.join(format!("disk-{mode}.img"));
    let _ = std::fs::remove_file(&disk);
    std::fs::File::create(&disk)?.set_len(opts.disk_gib << 30)?;
    let _ = std::fs::remove_file(dir.join("ovmf_vars.fd"));

    println!("== [{mode}] installing (downloads packages from the mirror)");
    let log: PathBuf = dir.join(format!("install-{mode}.log"));
    let _ = std::fs::remove_file(&log);
    let mut child = qemu(&o, &disk, Some(&iso_path), &log)?;
    let install = wait_for(&mut child, &log, "installation finished", &["INSTALLATION FAILED", "EXCEPTION", "KERNEL PANIC"], Duration::from_secs(45 * 60))?;
    let cfg = crate::lua::load_config(&config)?;
    for a in &cfg.user_archives {
        let fetched = install.contains(&format!("user archive {} -> ~/", a.url));
        let skipped = install.contains(&format!("warning: skipping {}", a.url));
        if !fetched && !skipped {
            return Err(format!("installer log says nothing about user archive {}", a.url).into());
        }
        println!("  user archive {}: {}", a.url, if fetched { "fetched" } else { "skipped" });
    }
    // Optional user files: reachable ones are fetched, unreachable ones only warn.
    for (i, f) in cfg.user_files.iter().enumerate() {
        let fetched = install.contains(&format!("user file {} -> ~/{}", f.url, f.dest));
        let skipped = install.contains(&format!("warning: skipping {}", f.url));
        if !fetched && !skipped {
            return Err(format!("installer log says nothing about user file #{i} {}", f.url).into());
        }
        println!("  user file {}: {}", f.dest, if fetched { "fetched" } else { "skipped (server unreachable)" });
    }
    let _ = child.wait(); // the installer resets the machine; -no-reboot ends QEMU

    println!("== [{mode}] first boot");
    let log = dir.join(format!("firstboot-{mode}.log"));
    let _ = std::fs::remove_file(&log);
    let mut child = qemu(&o, &disk, None, &log)?;
    wait_for(&mut child, &log, "archstaller-firstboot: done", &["archstaller-firstboot: FAILED", "Kernel panic", "FATAL"], Duration::from_secs(30 * 60))?;
    let _ = child.wait();
    let fb = String::from_utf8_lossy(&std::fs::read(&log).unwrap_or_default()).to_string();
    let mut wants: Vec<String> = cfg.users.iter().map(|u| format!("user {} created", u.name)).collect();
    wants.extend(cfg.services.iter().map(|s| format!("service {s}: enabled")));
    for (i, a) in cfg.user_archives.iter().enumerate() {
        if install.contains(&format!("user archive {} -> ~/", a.url)) {
            for u in &cfg.users {
                wants.push(format!("user archive #{i} extracted into ~ for {}", u.name));
            }
        }
    }
    for f in &cfg.user_files {
        if install.contains(&format!("user file {} -> ~/{}", f.url, f.dest)) {
            for u in &cfg.users {
                wants.push(format!("user file ~/{} installed for {}", f.dest, u.name));
            }
        }
    }
    for want in &wants {
        if !fb.contains(want.as_str()) {
            return Err(format!("first boot log lacks '{want}' (log: {})", log.display()).into());
        }
    }

    println!("== [{mode}] second boot");
    let log = dir.join(format!("secondboot-{mode}.log"));
    let _ = std::fs::remove_file(&log);
    let mut child = qemu(&o, &disk, None, &log)?;
    // AUR packages are built by archstaller-aur.service on this boot (plans-implement/aur.md); wait for it first.
    let aur_log;
    if !cfg.aur.is_empty() {
        println!("== [{mode}] AUR build on the installed system");
        let r = wait_for(&mut child, &log, "archstaller-aur: done", &["archstaller-aur: REFUSED", "archstaller-aur: warning", "archstaller-aur: FAILED", "Kernel panic", "FATAL"], Duration::from_secs(60 * 60));
        match r {
            Ok(t) => aur_log = t,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        }
        for a in &cfg.aur {
            if !aur_log.contains(&format!("archstaller-aur: package {}: installed", a.name)) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the AUR service did not report {} installed (log: {})", a.name, log.display()).into());
            }
            println!("  AUR package {}: built and installed", a.name);
        }
    }
    let res = wait_for(&mut child, &log, &format!("{} login:", cfg.hostname), &["Kernel panic", "FATAL"], Duration::from_secs(10 * 60));
    let _ = child.kill();
    let _ = child.wait();
    let text = res?;
    // Units that failed to start on the installed system are a defect worth failing on.
    let failed: Vec<&str> = text.lines().filter(|l| l.contains("FAILED") && l.contains("Failed to start")).collect();
    if !failed.is_empty() {
        return Err(format!("units failed on the second boot: {failed:?}").into());
    }
    println!("e2e [{mode}]: OK");
    Ok(())
}
