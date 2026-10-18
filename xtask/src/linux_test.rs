//! Boots the host's Linux kernel in QEMU with our initramfs and filesystems created
//! by our writers (ext4, btrfs, xfs, f2fs): proves tiny-init, module loading,
//! filesystem superblocks, on-disk formats, and Linux kernel acceptance in QEMU.
use crate::{root, run, target_dir, Result};
use disk::FsWriter;
use initrd::modules::{self, ModuleSource};
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct FileStorage(std::fs::File);

impl ext4w::Storage for FileStorage {
    fn write_at(&mut self, off: u64, data: &[u8]) -> ext4w::Result<()> {
        self.0
            .seek(SeekFrom::Start(off))
            .map_err(|_e| ext4w::Error::Io)?;
        self.0.write_all(data).map_err(|_e| ext4w::Error::Io)
    }
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> ext4w::Result<()> {
        self.0
            .seek(SeekFrom::Start(off))
            .map_err(|_e| ext4w::Error::Io)?;
        self.0.read_exact(buf).map_err(|_e| ext4w::Error::Io)
    }
}

impl disk::Storage for FileStorage {
    fn write_at(&mut self, off: u64, data: &[u8]) -> disk::Result<()> {
        self.0
            .seek(SeekFrom::Start(off))
            .map_err(|_e| disk::Error::Io)?;
        self.0.write_all(data).map_err(|_e| disk::Error::Io)
    }
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> disk::Result<()> {
        self.0
            .seek(SeekFrom::Start(off))
            .map_err(|_e| disk::Error::Io)?;
        self.0.read_exact(buf).map_err(|_e| disk::Error::Io)
    }
}

struct DirSource(std::collections::BTreeMap<String, PathBuf>);

impl ModuleSource for DirSource {
    fn find(&mut self, name: &str) -> Option<Vec<u8>> {
        let raw = std::fs::read(self.0.get(name)?).ok()?;
        modules::decompress(&raw).ok()
    }
}

const ROOT_UUID: [u8; 16] = [
    0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18, 0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e, 0x8f, 0x90,
];
const ROOT_UUID_TEXT: &str = "a1b2c3d4-e5f6-0718-293a-4b5c6d7e8f90";

fn populate_root(w: &mut dyn FsWriter, payload: &[u8], marker: &str) -> Result<()> {
    let dirm = disk::Meta {
        mode: 0o755,
        mtime: 1_790_000_000,
        ..Default::default()
    };
    for d in ["dev", "proc", "sys", "run", "etc", "usr/lib/systemd"] {
        w.mkdir(d, &dirm).map_err(|e| format!("{e:?}"))?;
    }
    let exe = disk::Meta {
        mode: 0o755,
        mtime: 1_790_000_000,
        ..Default::default()
    };
    w.begin_file("usr/lib/systemd/systemd", &exe)
        .map_err(|e| format!("{e:?}"))?;
    w.write_file(payload).map_err(|e| format!("{e:?}"))?;
    w.end_file().map_err(|e| format!("{e:?}"))?;
    let reg = disk::Meta {
        mode: 0o644,
        mtime: 1_790_000_000,
        ..Default::default()
    };
    w.begin_file("etc/archstaller-marker", &reg)
        .map_err(|e| format!("{e:?}"))?;
    w.write_file(marker.as_bytes())
        .map_err(|e| format!("{e:?}"))?;
    w.end_file().map_err(|e| format!("{e:?}"))?;
    w.finish().map_err(|e| format!("{e:?}"))?;
    Ok(())
}

fn boot_qemu(
    vmlinuz: &Path,
    initramfs: &Path,
    img: &Path,
    fstype: &str,
    extra_cmdline: &str,
    expected_marker: &str,
) -> Result<()> {
    let mut child = Command::new("qemu-system-x86_64")
        .args([
            "-machine",
            "q35",
            "-m",
            "512M",
            "-display",
            "none",
            "-serial",
            "stdio",
            "-no-reboot",
        ])
        .args(if std::path::Path::new("/dev/kvm").exists() {
            vec!["-enable-kvm", "-cpu", "host"]
        } else {
            vec![]
        })
        .arg("-kernel")
        .arg(vmlinuz)
        .arg("-initrd")
        .arg(initramfs)
        .arg("-append")
        .arg(format!(
            "console=ttyS0 root=UUID={ROOT_UUID_TEXT} rootfstype={fstype} panic=-1 loglevel=7 {extra_cmdline}"
        ))
        .arg("-drive")
        .arg(format!("if=none,id=d0,format=raw,file={}", img.display()))
        .args(["-device", "virtio-blk-pci,drive=d0"])
        .stdout(Stdio::piped())
        .spawn()?;
    let mut log = String::new();
    let mut out = child.stdout.take().ok_or("cannot capture stdout")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = out.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let start = std::time::Instant::now();
    let ok = loop {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(chunk) => {
                log.push_str(&String::from_utf8_lossy(&chunk));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break false,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(i) = log.find("marker=") {
            if log.get(i..).is_some_and(|s| s.contains('\n')) {
                break true;
            }
        }
        if let Some(i) = log.find("FATAL:") {
            if log.get(i..).is_some_and(|s| s.contains('\n')) {
                break false;
            }
        }
        if log.contains("Kernel panic") {
            break false;
        }
        if start.elapsed().as_secs() > 30 {
            break false;
        }
    };
    drop(child.kill());
    drop(child.wait());
    if !ok || !log.contains(expected_marker) {
        println!("--- BOOT LOG FOR {fstype} ---");
        for line in log.lines().rev().take(60).collect::<Vec<_>>().into_iter().rev() {
            println!("  {line}");
        }
        println!("-----------------------------");
        return Err(format!("Linux boot test failed for filesystem {fstype}").into());
    }
    for line in log.lines().filter(|l| {
        l.contains("init:")
            || l.contains("test-payload")
            || l.contains("EXT4-fs")
            || l.contains("BTRFS")
            || l.contains("XFS")
            || l.contains("F2FS")
            || l.contains("Kernel panic")
    }) {
        println!("  {line}");
    }
    println!("linux-test: {fstype} OK (kernel mounted the {fstype} root and tiny-init switched to it)");
    Ok(())
}

pub fn run_test() -> Result<()> {
    let release = String::from_utf8(Command::new("uname").arg("-r").output()?.stdout)?
        .trim()
        .to_string();
    let moddir = PathBuf::from("/usr/lib/modules").join(&release);
    let vmlinuz = moddir.join("vmlinuz");
    if !vmlinuz.exists() {
        return Err(format!("host kernel image {} not found", vmlinuz.display()).into());
    }
    run(
        Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .current_dir(root())
            .args([
                "build",
                "-p",
                "tiny-init",
                "--release",
                "--target",
                "x86_64-unknown-none",
            ]),
    )?;
    let bin = target_dir().join("x86_64-unknown-none/release");
    let init = std::fs::read(bin.join("tiny-init"))?;
    let payload = std::fs::read(bin.join("test-payload"))?;

    let dir = target_dir().join("linux-test");
    std::fs::create_dir_all(&dir)?;

    // Initramfs with tiny-init and modules.
    let mut src = DirSource(Default::default());
    let find = Command::new("find")
        .arg(moddir.join("kernel"))
        .args(["-name", "*.ko*"])
        .output()?;
    for line in String::from_utf8_lossy(&find.stdout).lines() {
        if let Some(n) = modules::name_from_path(line) {
            src.0.insert(n, PathBuf::from(line));
        }
    }
    let builtin = match std::fs::read_to_string(moddir.join("modules.builtin")) {
        Ok(t) => modules::parse_builtin(&t),
        Err(_) => BTreeSet::new(),
    };
    let resolved = modules::resolve(
        &mut src,
        &[],
        &["dummy", "xfs", "f2fs", "crc32c_generic", "crc32c_intel"],
        &builtin,
    )
    .map_err(|e| format!("{e:?}"))?;
    println!("initramfs modules: {:?}", resolved.order);
    let cpio = modules::build_initramfs(&init, &resolved);
    let initramfs = dir.join("initramfs.cpio");
    std::fs::write(&initramfs, &cpio)?;
    println!("initramfs: {} bytes", cpio.len());

    let size = 64u64 << 20;

    // 1. Test ext4
    {
        let img = dir.join("root-ext4.img");
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&img)?;
        f.set_len(size)?;
        let mut w = ext4w::Ext4Writer::new(
            FileStorage(f),
            size,
            ext4w::Options {
                label: "archroot".into(),
                uuid: ROOT_UUID,
                hash_seed: [1, 2, 3, 4],
                now: 1_790_000_000,
                reserved_percent: 1,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        populate_root(&mut w, &payload, "HELLO-FROM-EXT4W\n")?;
        boot_qemu(
            &vmlinuz,
            &initramfs,
            &img,
            "ext4",
            "",
            "marker=HELLO-FROM-EXT4W",
        )?;
    }

    // 2. Test btrfs
    {
        let img = dir.join("root-btrfs.img");
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&img)?;
        f.set_len(size)?;
        let mut w = btrfsw::BtrfsWriter::new(
            FileStorage(f),
            size,
            btrfsw::Options {
                label: "archroot".into(),
                uuid: ROOT_UUID,
                now: 1_790_000_000,
                subvolumes: vec![
                    btrfsw::SubvolConfig {
                        name: "@".into(),
                        mount: "/".into(),
                    },
                    btrfsw::SubvolConfig {
                        name: "@home".into(),
                        mount: "/home".into(),
                    },
                ],
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        populate_root(&mut w, &payload, "HELLO-FROM-BTRFSW\n")?;
        boot_qemu(
            &vmlinuz,
            &initramfs,
            &img,
            "btrfs",
            "rootflags=subvol=@",
            "marker=HELLO-FROM-BTRFSW",
        )?;
    }

    // 3. Test xfs
    {
        let img = dir.join("root-xfs.img");
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&img)?;
        f.set_len(size)?;
        let mut w = xfsw::XfsWriter::new(
            FileStorage(f),
            size,
            xfsw::Options {
                label: "archroot".into(),
                uuid: ROOT_UUID,
                now: 1_790_000_000,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        populate_root(&mut w, &payload, "HELLO-FROM-XFSW\n")?;
        boot_qemu(&vmlinuz, &initramfs, &img, "xfs", "", "marker=HELLO-FROM-XFSW")?;
    }

    // 4. Test f2fs
    {
        let img = dir.join("root-f2fs.img");
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&img)?;
        f.set_len(size)?;
        let mut w = f2fsw::F2fsWriter::new(
            FileStorage(f),
            size,
            f2fsw::Options {
                label: "archroot".into(),
                uuid: ROOT_UUID,
                now: 1_790_000_000,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        populate_root(&mut w, &payload, "HELLO-FROM-F2FSW\n")?;
        boot_qemu(&vmlinuz, &initramfs, &img, "f2fs", "", "marker=HELLO-FROM-F2FSW")?;
    }

    println!("All filesystems (ext4, btrfs, xfs, f2fs) passed QEMU boot test!");
    Ok(())
}
