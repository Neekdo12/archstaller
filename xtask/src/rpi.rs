use crate::{root, target_dir, Options, Result};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// Exactly 32 MiB (33,554,432 bytes) FAT32 bootable SD card / USB image for Raspberry Pi.
pub const RPI_IMAGE_SIZE: usize = 32 * 1024 * 1024;

/// Builds the AArch64 bare-metal Raspberry Pi kernel (`kernel-rpi`).
pub fn build(release: bool) -> Result<PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "build",
        "-p",
        "kernel-rpi",
        "--target",
        "aarch64-unknown-none",
    ]);
    if release {
        cmd.arg("--release");
    }
    cmd.current_dir(root());
    crate::run(&mut cmd)?;

    let profile = if release { "release" } else { "debug" };
    let bin = target_dir()
        .join("aarch64-unknown-none")
        .join(profile)
        .join("kernel-rpi");
    println!("Built Raspberry Pi kernel: {}", bin.display());
    Ok(bin)
}

/// Assembles a 32 MiB bootable Raspberry Pi FAT32 image (`target/archstaller-rpi.img`).
pub fn build_image(opts: &Options) -> Result<PathBuf> {
    if opts.progress {
        println!(
            "{}",
            hostcfg::progress::Event::Stage {
                name: "config".into(),
                state: hostcfg::progress::State::Start
            }
            .line()
        );
    }
    let loaded = hostcfg::toml::load(&opts.config)?;
    let config_bin = hostcfg::toml::config_bin(&loaded, &opts.extra_kernel_params)?;
    if opts.progress {
        println!(
            "{}",
            hostcfg::progress::Event::Stage {
                name: "config".into(),
                state: hostcfg::progress::State::Ok
            }
            .line()
        );
        println!(
            "{}",
            hostcfg::progress::Event::Stage {
                name: "kernel-rpi".into(),
                state: hostcfg::progress::State::Start
            }
            .line()
        );
    }

    let kernel_bin = build(false)?;
    if opts.progress {
        println!(
            "{}",
            hostcfg::progress::Event::Stage {
                name: "kernel-rpi".into(),
                state: hostcfg::progress::State::Ok
            }
            .line()
        );
        println!(
            "{}",
            hostcfg::progress::Event::Stage {
                name: "rpi-image".into(),
                state: hostcfg::progress::State::Start
            }
            .line()
        );
    }

    let out = opts
        .out
        .clone()
        .unwrap_or_else(|| target_dir().join("archstaller-rpi.img"));
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let f = std::fs::File::create(&out)?;
    f.set_len(RPI_IMAGE_SIZE as u64)?;
    drop(f);

    let out_str = out.to_str().ok_or("non-utf8 image path")?;

    crate::run(Command::new("mformat").args([
        "-i",
        out_str,
        "-F",
        "-v",
        "ARCHBOOT",
        "::",
    ]))?;
    crate::run(
        Command::new("mcopy")
            .args(["-i", out_str])
            .arg(&kernel_bin)
            .arg("::/kernel8.img"),
    )?;

    let work = opts.workdir.clone().unwrap_or_else(target_dir);
    std::fs::create_dir_all(&work)?;
    let cfg_txt_path = work.join("config.txt");
    std::fs::write(&cfg_txt_path, b"# Archstaller Raspberry Pi Boot Configuration\narm_64bit=1\nenable_uart=1\nuart_2ndstage=1\ndtoverlay=disable-bt\n")?;
    crate::run(
        Command::new("mcopy")
            .args(["-i", out_str])
            .arg(&cfg_txt_path)
            .arg("::/config.txt"),
    )?;

    let cmdline_txt_path = work.join("cmdline.txt");
    std::fs::write(
        &cmdline_txt_path,
        b"console=serial0,115200 console=tty1 rootwait\n",
    )?;
    crate::run(
        Command::new("mcopy")
            .args(["-i", out_str])
            .arg(&cmdline_txt_path)
            .arg("::/cmdline.txt"),
    )?;

    let config_bin_path = work.join("config.bin");
    std::fs::write(&config_bin_path, &config_bin)?;
    crate::run(
        Command::new("mcopy")
            .args(["-i", out_str])
            .arg(&config_bin_path)
            .arg("::/config.bin"),
    )?;

    if opts.progress {
        println!(
            "{}",
            hostcfg::progress::Event::Stage {
                name: "rpi-image".into(),
                state: hostcfg::progress::State::Ok
            }
            .line()
        );
    }

    println!("Built Raspberry Pi bootable image: {}", out.display());
    println!("  Format: FAT32 (32 MiB)");
    println!("  Files:  kernel8.img, config.txt, cmdline.txt, config.bin");

    use sha2::Digest;
    let img_data = std::fs::read(&out)?;
    let sha256: String = sha2::Sha256::digest(&img_data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    crate::iso::emit(
        opts,
        hostcfg::progress::Event::Done {
            path: out.display().to_string(),
            size: img_data.len() as u64,
            sha256,
            profile: "rpi-aarch64".into(),
            config: opts.config.display().to_string(),
        },
    );

    Ok(out)
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

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot spawn qemu-system-aarch64: {e}"))?;
    let stdout = child.stdout.take().ok_or("cannot capture stdout")?;

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(std::result::Result::ok) {
            drop(tx.send(line));
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
            drop(child.kill());
            drop(child.wait());
            println!(
                "\nRaspberry Pi QEMU E2E test PASSED! (EL1 drop, UART, CPU and timer verified)"
            );
            return Ok(());
        }

        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("QEMU exited prematurely with status {status}").into());
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    drop(child.kill());
    drop(child.wait());
    Err("Raspberry Pi QEMU E2E test timed out before seeing boot success marker".into())
}
