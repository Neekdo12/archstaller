use crate::{root, target_dir, Result};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// Builds the AArch64 bare-metal Raspberry Pi kernel (`kernel-rpi`).
pub fn build(release: bool) -> Result<PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "-p", "kernel-rpi", "--target", "aarch64-unknown-none"]);
    if release {
        cmd.arg("--release");
    }
    cmd.current_dir(root());
    crate::run(&mut cmd)?;

    let profile = if release { "release" } else { "debug" };
    let bin = target_dir().join("aarch64-unknown-none").join(profile).join("kernel-rpi");
    println!("Built Raspberry Pi kernel: {}", bin.display());
    Ok(bin)
}

/// Runs the AArch64 bare-metal kernel in QEMU (`raspi3b` by default).
pub fn run(board: &str) -> Result<()> {
    let bin = build(false)?;
    let mut cmd = Command::new("qemu-system-aarch64");
    let machine = if board == "virt" { "virt" } else { "raspi3b" };
    cmd.args(["-M", machine, "-kernel"]);
    cmd.arg(&bin);
    cmd.args(["-serial", "stdio", "-display", "none"]);
    println!("Running Raspberry Pi kernel in QEMU ({machine})...");
    cmd.status()?;
    Ok(())
}

/// Runs an automated end-to-end test in QEMU for the Raspberry Pi kernel.
pub fn test(board: &str) -> Result<()> {
    let bin = build(false)?;
    let machine = if board == "virt" { "virt" } else { "raspi3b" };
    println!("Starting QEMU E2E test for Raspberry Pi ({machine})...");

    let mut cmd = Command::new("qemu-system-aarch64");
    cmd.args(["-M", machine, "-kernel"]);
    cmd.arg(&bin);
    cmd.args(["-serial", "stdio", "-display", "none"]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("cannot spawn qemu-system-aarch64: {e}"))?;
    let stdout = child.stdout.take().ok_or("cannot capture stdout")?;

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(std::result::Result::ok) {
            let _ = tx.send(line);
        }
    });

    let timeout = Duration::from_secs(10);
    let start = Instant::now();
    let mut saw_banner = false;
    let mut saw_el1 = false;
    let mut saw_uart = false;
    let mut saw_success = false;

    while start.elapsed() < timeout {
        while let Ok(line) = rx.try_recv() {
            println!("  [qemu] {}", line);
            if line.contains("archstaller (aarch64 / bare-metal)") {
                saw_banner = true;
            }
            if line.contains("CPU: Exception Level = EL1") {
                saw_el1 = true;
            }
            if line.contains("Console: PL011 UART initialized") {
                saw_uart = true;
            }
            if line.contains("Archstaller AArch64 bare-metal kernel booted successfully!") {
                saw_success = true;
            }
        }

        if saw_banner && saw_el1 && saw_uart && saw_success {
            let _ = child.kill();
            let _ = child.wait();
            println!("\nRaspberry Pi QEMU E2E test PASSED! (EL1 drop, UART, CPU and timer verified)");
            return Ok(());
        }

        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("QEMU exited prematurely with status {status}").into());
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = child.kill();
    let _ = child.wait();
    Err("Raspberry Pi QEMU E2E test timed out before seeing boot success marker".into())
}

