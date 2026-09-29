//! Boots the host's Linux kernel in QEMU with our initramfs and an ext4w-built root filesystem:
//! proves tiny-init, module loading, the initramfs format and kernel-side ext4 acceptance.
use crate::{root, run, Result};
use ext4w::{Ext4Writer, Meta, Options, Storage};
use initrd::modules::{self, ModuleSource};
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct FileStorage(std::fs::File);

impl Storage for FileStorage {
    fn write_at(&mut self, off: u64, data: &[u8]) -> ext4w::Result<()> {
        self.0.seek(SeekFrom::Start(off)).map_err(|_| ext4w::Error::Io)?;
        self.0.write_all(data).map_err(|_| ext4w::Error::Io)
    }
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> ext4w::Result<()> {
        self.0.seek(SeekFrom::Start(off)).map_err(|_| ext4w::Error::Io)?;
        self.0.read_exact(buf).map_err(|_| ext4w::Error::Io)
    }
}

struct DirSource(std::collections::BTreeMap<String, PathBuf>);

impl ModuleSource for DirSource {
    fn find(&mut self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.get(name)?).ok()
    }
}

const ROOT_UUID: [u8; 16] = [0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18, 0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e, 0x8f, 0x90];
const ROOT_UUID_TEXT: &str = "a1b2c3d4-e5f6-0718-293a-4b5c6d7e8f90";

pub fn run_test() -> Result<()> {
    let release = String::from_utf8(Command::new("uname").arg("-r").output()?.stdout)?.trim().to_string();
    let moddir = PathBuf::from("/usr/lib/modules").join(&release);
    let vmlinuz = moddir.join("vmlinuz");
    if !vmlinuz.exists() {
        return Err(format!("host kernel image {} not found", vmlinuz.display()).into());
    }
    run(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root())
        .args(["build", "-p", "tiny-init", "--release", "--target", "x86_64-unknown-none"]))?;
    let bin = root().join("target/x86_64-unknown-none/release");
    let init = std::fs::read(bin.join("tiny-init"))?;
    let payload = std::fs::read(bin.join("test-payload"))?;

    // Root filesystem.
    let dir = root().join("target/linux-test");
    std::fs::create_dir_all(&dir)?;
    let img = dir.join("root.img");
    let size = 64u64 << 20;
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&img)?;
    f.set_len(size)?;
    let mut w = Ext4Writer::new(
        FileStorage(f),
        size,
        Options { label: "archroot".into(), uuid: ROOT_UUID, hash_seed: [1, 2, 3, 4], now: 1_790_000_000, reserved_percent: 1 },
    )
    .map_err(|e| format!("{e:?}"))?;
    let dirm = Meta { mode: 0o755, mtime: 1_790_000_000, ..Default::default() };
    for d in ["dev", "proc", "sys", "run", "etc", "usr/lib/systemd"] {
        w.mkdir(d, &dirm).map_err(|e| format!("{e:?}"))?;
    }
    let exe = Meta { mode: 0o755, mtime: 1_790_000_000, ..Default::default() };
    w.begin_file("usr/lib/systemd/systemd", &exe).map_err(|e| format!("{e:?}"))?;
    w.write_file(&payload).map_err(|e| format!("{e:?}"))?;
    w.end_file().map_err(|e| format!("{e:?}"))?;
    let reg = Meta { mode: 0o644, mtime: 1_790_000_000, ..Default::default() };
    w.begin_file("etc/archstaler-marker", &reg).map_err(|e| format!("{e:?}"))?;
    w.write_file(b"HELLO-FROM-EXT4W\n").map_err(|e| format!("{e:?}"))?;
    w.end_file().map_err(|e| format!("{e:?}"))?;
    w.finish().map_err(|e| format!("{e:?}"))?;

    // Initramfs with tiny-init and one loadable module to exercise init_module.
    let mut src = DirSource(Default::default());
    let find = Command::new("find").arg(moddir.join("kernel")).args(["-name", "*.ko*"]).output()?;
    for line in String::from_utf8_lossy(&find.stdout).lines() {
        if let Some(n) = modules::name_from_path(line) {
            src.0.insert(n, PathBuf::from(line));
        }
    }
    let builtin = match std::fs::read_to_string(moddir.join("modules.builtin")) {
        Ok(t) => modules::parse_builtin(&t),
        Err(_) => BTreeSet::new(),
    };
    let resolved = modules::resolve(&mut src, &[], &["dummy"], &builtin).map_err(|e| format!("{e:?}"))?;
    println!("initramfs modules: {:?}", resolved.order);
    let cpio = modules::build_initramfs(&init, &resolved);
    let initramfs = dir.join("initramfs.cpio");
    std::fs::write(&initramfs, &cpio)?;
    println!("initramfs: {} bytes", cpio.len());

    let mut child = Command::new("qemu-system-x86_64")
        .args(["-machine", "q35", "-m", "512M", "-display", "none", "-serial", "stdio", "-no-reboot"])
        .args(if std::path::Path::new("/dev/kvm").exists() { vec!["-enable-kvm", "-cpu", "host"] } else { vec![] })
        .arg("-kernel")
        .arg(&vmlinuz)
        .arg("-initrd")
        .arg(&initramfs)
        .arg("-append")
        .arg(format!("console=ttyS0 root=UUID={ROOT_UUID_TEXT} rootfstype=ext4 panic=-1 loglevel=4"))
        .arg("-drive")
        .arg(format!("if=none,id=d0,format=raw,file={}", img.display()))
        .args(["-device", "virtio-blk-pci,drive=d0"])
        .stdout(Stdio::piped())
        .spawn()?;
    let mut log = String::new();
    let mut out = child.stdout.take().unwrap();
    let start = std::time::Instant::now();
    let mut buf = [0u8; 4096];
    let ok = loop {
        let n = out.read(&mut buf)?;
        if n == 0 {
            break false;
        }
        log.push_str(&String::from_utf8_lossy(&buf[..n]));
        if let Some(i) = log.find("marker=") {
            if log[i..].contains('\n') {
                break true;
            }
        }
        if start.elapsed().as_secs() > 60 {
            break false;
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    for line in log.lines().filter(|l| l.contains("init:") || l.contains("test-payload") || l.contains("EXT4-fs") || l.contains("Kernel panic")) {
        println!("  {line}");
    }
    if !ok || !log.contains("marker=HELLO-FROM-EXT4W") {
        return Err("Linux boot test failed".into());
    }
    println!("linux-test: OK (kernel mounted the ext4w root and tiny-init switched to it)");
    Ok(())
}
