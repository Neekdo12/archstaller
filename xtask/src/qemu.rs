use crate::{root, Options, Result};
use std::path::Path;
use std::process::Command;

const OVMF_DIR: &str = "/usr/share/edk2/x64";

const PATTERN_LEN: usize = 1_000_000;

/// Serves the deterministic pattern on 127.0.0.1:8000 (`/pattern.bin` plain, `/chunked.bin` chunked).
fn spawn_test_server() -> Result<()> {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:8000")?;
    std::thread::spawn(move || {
        let body: Vec<u8> = (0..PATTERN_LEN).map(|i| (i * 7 + 3) as u8).collect();
        for conn in listener.incoming().flatten() {
            let mut conn = conn;
            let mut req = [0u8; 2048];
            let n = conn.read(&mut req).unwrap_or(0);
            let req = String::from_utf8_lossy(&req[..n]);
            if req.starts_with("GET /chunked.bin") {
                let _ = conn.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
                for c in body.chunks(7000) {
                    let _ = conn.write_all(format!("{:x}\r\n", c.len()).as_bytes());
                    let _ = conn.write_all(c);
                    let _ = conn.write_all(b"\r\n");
                }
                let _ = conn.write_all(b"0\r\n\r\n");
            } else if req.starts_with("GET /pattern.bin") {
                let _ = conn.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes());
                let _ = conn.write_all(&body);
            } else {
                let _ = conn.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        }
    });
    Ok(())
}

pub fn run_iso(iso: &Path, opts: &Options) -> Result<()> {
    if opts.selftest {
        spawn_test_server()?;
    }
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
    // Always start from a blank disk: a partition table left by an earlier run would make the
    // firmware try the disk before the ISO.
    let f = std::fs::File::create(&disk)?;
    f.set_len(1 << 30)?;
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
        "igb" => cmd.args(["-device", "igb,netdev=n0"]),
        "rtl8139" => cmd.args(["-device", "rtl8139,netdev=n0"]),
        other => return Err(format!("unknown nic type {other}").into()),
    };
    if opts.usb {
        // Generic USB device behind QEMU's xHCI, to smoke-test the installer's xHCI
        // driver (enumeration, descriptors, bulk transfers via BOT INQUIRY).
        let img = root().join("target/usb-test.img");
        if !img.exists() {
            let f = std::fs::File::create(&img)?;
            f.set_len(8 << 20)?;
        }
        cmd.args(["-device", "qemu-xhci,id=xhci"]);
        cmd.arg("-drive").arg(format!("if=none,id=u0,format=raw,file={}", img.display()));
        cmd.args(["-device", "usb-storage,bus=xhci.0,drive=u0"]);
    }
    let status = cmd.status()?;
    // isa-debug-exit turns the kernel's `out 0xf4, 0` into exit code 1.
    match status.code() {
        Some(0) | Some(1) => Ok(()),
        _ => Err(format!("{cmd:?} failed: {status}").into()),
    }
}
