//! Generates and boots 1.44 MB floppy disk images.
use crate::{iso, target_dir, Options, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Exactly 1,474,560 bytes (2880 sectors of 512 bytes, standard 3.5" HD floppy diskette).
pub const FLOPPY_SIZE: usize = 1_474_560;

/// Assembles a 1.44 MB bootable floppy image (`target/archstaller-floppy.img`).
pub fn build(opts: &Options) -> Result<PathBuf> {
    let components = iso::build_components(opts)?;
    let bios = components.bios;
    let payload = components.payload;

    let mut s1 = bios.stage1;
    let mut s2 = bios.stage2;

    let s2_off = 512u64;
    let s2_len = s2.len() as u32;
    let payload_off = (s2_off + s2_len as u64).next_multiple_of(512);
    let payload_len = payload.len() as u64;

    let total_len = payload_off + payload_len;
    if total_len > FLOPPY_SIZE as u64 {
        return Err(format!("Floppy image exceeds 1.44 MB ({total_len} > {FLOPPY_SIZE})").into());
    }

    bootinfo::bios::patch_stage1(&mut s1, s2_off, s2_len);
    bootinfo::bios::patch_stage2(&mut s2, bootinfo::bios::MODE_PAYLOAD, &[(payload_off, payload_len)], "")?;

    let mut img = vec![0u8; FLOPPY_SIZE];
    img[..512].copy_from_slice(&s1);
    img[s2_off as usize..s2_off as usize + s2.len()].copy_from_slice(&s2);
    img[payload_off as usize..payload_off as usize + payload.len()].copy_from_slice(&payload);

    let out = opts.out.clone().unwrap_or_else(|| target_dir().join("archstaller-floppy.img"));
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out, &img)?;

    println!("Built 1.44 MB bootable floppy image: {}", out.display());
    println!("  Stage 1 (boot sector): 512 bytes (LBA 0)");
    println!("  Stage 2 (loader):      {} bytes (LBA 1..{})", s2_len, s2_off / 512 + (s2_len as u64 / 512) - 1);
    println!("  Payload:               {} bytes (LBA {}..{})", payload_len, payload_off / 512, (payload_off + payload_len) / 512 - 1);
    println!("  Total Used:            {} / {} bytes ({:.1}%)", total_len, FLOPPY_SIZE, (total_len as f64 / FLOPPY_SIZE as f64) * 100.0);
    println!("  Free Space:            {} bytes ({:.1}%)", FLOPPY_SIZE as u64 - total_len, ((FLOPPY_SIZE as u64 - total_len) as f64 / FLOPPY_SIZE as f64) * 100.0);

    use sha2::Digest;
    let sha256: String = sha2::Sha256::digest(&img).iter().map(|b| format!("{b:02x}")).collect();
    iso::emit(
        opts,
        hostcfg::progress::Event::Done {
            path: out.display().to_string(),
            size: img.len() as u64,
            sha256,
            profile: components.resolved.profile.name().into(),
            config: opts.config.display().to_string(),
        },
    );

    Ok(out)
}

/// Runs the 1.44 MB floppy image in QEMU.
pub fn run(opts: &Options) -> Result<()> {
    let img = build(opts)?;
    println!("Booting floppy disk image in QEMU...");
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-machine", if opts.disk == "ide" { "pc" } else { "q35" }, "-m", "512M", "-no-reboot"]);
    if std::path::Path::new("/dev/kvm").exists() {
        cmd.args(["-enable-kvm", "-cpu", "host"]);
    }
    cmd.args(["-fda"]).arg(&img).args(["-boot", "a", "-serial", "stdio"]);
    if opts.headless {
        cmd.args(["-display", "none"]);
    }
    cmd.status()?;
    Ok(())
}

/// Automated test: boots the floppy image in QEMU and verifies stage 1, stage 2, and kernel banner.
pub fn test(opts: &Options) -> Result<()> {
    let img = build(opts)?;
    println!("Starting automated QEMU floppy boot test...");
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-machine", if opts.disk == "ide" { "pc" } else { "q35" }, "-m", "512M", "-no-reboot", "-display", "none"]);
    if std::path::Path::new("/dev/kvm").exists() {
        cmd.args(["-enable-kvm", "-cpu", "host"]);
    }
    cmd.args(["-fda"]).arg(&img).args(["-boot", "a", "-serial", "stdio"]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("cannot run QEMU: {e}"))?;
    let stdout = child.stdout.take().ok_or("cannot capture stdout")?;

    use std::io::{BufRead, BufReader};
    use std::sync::mpsc::channel;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(std::result::Result::ok) {
            let _ = tx.send(line);
        }
    });

    let timeout = Duration::from_secs(10);
    let start = Instant::now();
    let mut saw_s2 = false;
    let mut saw_long = false;
    let mut saw_kernel = false;

    while start.elapsed() < timeout {
        while let Ok(line) = rx.try_recv() {
            println!("  [floppy] {line}");
            if line.contains("boot: BIOS loader") {
                saw_s2 = true;
            }
            if line.contains("boot: long mode") {
                saw_long = true;
            }
            if line.contains("archstaller") || line.contains("ARCHSTALLER") {
                saw_kernel = true;
            }
        }

        if saw_s2 && saw_long && saw_kernel {
            let _ = child.kill();
            let _ = child.wait();
            println!("\nFloppy boot test PASSED! (Stage 1 CHS read, Stage 2 CHS payload load, long mode, kernel entry verified)");
            return Ok(());
        }

        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("QEMU exited prematurely with status {status}").into());
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = child.kill();
    let _ = child.wait();
    Err("Floppy boot test timed out before seeing kernel banner".into())
}
