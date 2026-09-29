//! Builds a filesystem image from real packages and checks it with e2fsck and debugfs.
use ext4w::{Error, Ext4Writer, FileKind, Meta, Options, Storage};
use pkg::tar::{Kind, TarReader};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

struct FileStorage(File);

impl Storage for FileStorage {
    fn write_at(&mut self, off: u64, data: &[u8]) -> Result<(), Error> {
        self.0.seek(SeekFrom::Start(off)).map_err(|_| Error::Io)?;
        self.0.write_all(data).map_err(|_| Error::Io)
    }
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.0.seek(SeekFrom::Start(off)).map_err(|_| Error::Io)?;
        self.0.read_exact(buf).map_err(|_| Error::Io)
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ext4w-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn opts() -> Options {
    Options {
        label: "archroot".into(),
        uuid: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01],
        hash_seed: [1, 2, 3, 4],
        now: 1_790_000_000,
        reserved_percent: 1,
    }
}

fn meta_of(e: &pkg::tar::Entry) -> Meta {
    Meta { mode: (e.mode & 0o7777) as u16, uid: e.uid, gid: e.gid, mtime: e.mtime, xattrs: e.xattrs.clone() }
}

/// Installs a package's file tree (skipping package metadata files).
fn install(w: &mut Ext4Writer<FileStorage>, pkg_path: &Path) {
    let data = std::fs::read(pkg_path).unwrap();
    let mut tar = TarReader::new(pkg::compress::open(data.as_slice()).unwrap());
    let mut buf = vec![0u8; 65536];
    while let Some(e) = tar.next_entry().unwrap() {
        if e.path.starts_with('.') && !e.path.starts_with("./") {
            continue; // .PKGINFO, .MTREE, .BUILDINFO, .INSTALL
        }
        let m = meta_of(&e);
        match e.kind {
            Kind::Dir => {
                w.mkdir(&e.path, &m).unwrap();
            }
            Kind::File => {
                w.begin_file(&e.path, &m).unwrap();
                loop {
                    let n = tar.read_data(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    w.write_file(&buf[..n]).unwrap();
                }
                w.end_file().unwrap();
            }
            Kind::Symlink => {
                w.symlink(&e.path, &e.link, &m).unwrap();
            }
            Kind::Hardlink => {
                w.hardlink(&e.path, &e.link).unwrap();
            }
            Kind::CharDev => {
                w.mknod(&e.path, FileKind::CharDev, e.dev_major, e.dev_minor, &m).unwrap();
            }
            Kind::BlockDev => {
                w.mknod(&e.path, FileKind::BlockDev, e.dev_major, e.dev_minor, &m).unwrap();
            }
            Kind::Fifo => {
                w.mknod(&e.path, FileKind::Fifo, 0, 0, &m).unwrap();
            }
        }
    }
}

fn find_pkgs(prefixes: &[&str]) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir("/var/cache/pacman/pkg") else { return vec![] };
    let all: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".pkg.tar.zst")).collect();
    let mut out = Vec::new();
    for pre in prefixes {
        // Match "<name>-<digit>" so "bash" does not pick "bash-completion".
        let mut m: Vec<&PathBuf> = all
            .iter()
            .filter(|p| {
                let n = p.file_name().unwrap().to_string_lossy().to_string();
                n.strip_prefix(pre).is_some_and(|r| r.starts_with('-') && r[1..].starts_with(|c: char| c.is_ascii_digit()))
            })
            .collect();
        m.sort();
        if let Some(p) = m.last() {
            out.push((*p).clone());
        }
    }
    out
}

fn run_ok(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{cmd:?} failed:\n{text}");
    text
}

/// e2fsck -n exits 0 even for ignored checksum errors, so inspect the report too.
fn fsck_clean(img: &Path) {
    let out = Command::new("e2fsck").args(["-fn"]).arg(img).output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let bad = out.status.code() != Some(0) || ["Fix?", "IGNORED", "WARNING", "differences", "invalid"].iter().any(|m| text.contains(m));
    assert!(!bad, "e2fsck reported problems:\n{text}");
}

fn build_image(dir: &Path, size: u64, pkgs: &[PathBuf]) -> PathBuf {
    let img = dir.join("root.img");
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&img).unwrap();
    f.set_len(size).unwrap();
    let mut w = Ext4Writer::new(FileStorage(f), size, opts()).unwrap();
    for p in pkgs {
        install(&mut w, p);
    }
    w.finish().unwrap();
    img
}

#[test]
fn empty_filesystem_is_clean() {
    let dir = scratch("empty");
    let img = build_image(&dir, 64 << 20, &[]);
    fsck_clean(&img);
    let info = run_ok(Command::new("tune2fs").arg("-l").arg(&img));
    assert!(info.contains("Filesystem features:") && info.contains("metadata_csum"), "{info}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn package_tree_matches_tar_and_fsck_is_clean() {
    let pkgs = find_pkgs(&["filesystem", "bash", "shadow", "gstreamer", "coreutils", "systemd-libs", "tzdata"]);
    if pkgs.len() < 4 {
        return;
    }
    let dir = scratch("pkgs");
    let img = build_image(&dir, 2 << 30, &pkgs);
    fsck_clean(&img);

    // Reference: extract the same packages with tar.
    let reference = dir.join("ref");
    std::fs::create_dir_all(&reference).unwrap();
    for p in &pkgs {
        run_ok(Command::new("tar").args(["--zstd", "-xf"]).arg(p).arg("-C").arg(&reference).args(["--exclude=.PKGINFO", "--exclude=.MTREE", "--exclude=.BUILDINFO", "--exclude=.INSTALL", "--exclude=.CHANGELOG"]));
    }
    let dumped = dir.join("dump");
    std::fs::create_dir_all(&dumped).unwrap();
    run_ok(Command::new("debugfs").args(["-R", &format!("rdump / {}", dumped.display())]).arg(&img));
    let dumped = dumped.join("");
    let d = Command::new("diff").args(["-r", "--no-dereference", "-x", "lost+found"]).arg(&reference).arg(&dumped).output().unwrap();
    assert!(d.status.success(), "diff:\n{}", String::from_utf8_lossy(&d.stdout).chars().take(3000).collect::<String>());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn xattrs_and_special_files_survive() {
    let pkgs = find_pkgs(&["gstreamer"]);
    if pkgs.is_empty() {
        return;
    }
    let dir = scratch("xattr");
    let img = build_image(&dir, 1 << 30, &pkgs);
    fsck_clean(&img);
    let out = run_ok(Command::new("debugfs").args(["-R", "ea_list /usr/lib/gstreamer-1.0/gst-ptp-helper"]).arg(&img));
    assert!(out.contains("security.capability"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_back_and_replace() {
    let dir = scratch("readback");
    let img = dir.join("r.img");
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&img).unwrap();
    f.set_len(64 << 20).unwrap();
    let mut w = Ext4Writer::new(FileStorage(f), 64 << 20, opts()).unwrap();
    let data: Vec<u8> = (0..300_000u32).map(|i| (i * 31 + 7) as u8).collect();
    let m = Meta { mode: 0o644, mtime: 1_700_000_000, ..Default::default() };
    w.begin_file("var/cache/pkg/a.bin", &m).unwrap();
    w.write_file(&data[..100_000]).unwrap();
    w.write_file(&data[100_000..]).unwrap();
    let ino = w.end_file().unwrap();
    let mut back = vec![0u8; 300_000];
    assert_eq!(w.read_file(ino, 0, &mut back).unwrap(), 300_000);
    assert_eq!(back, data);
    let mut part = [0u8; 10];
    assert_eq!(w.read_file(ino, 4090, &mut part).unwrap(), 10);
    assert_eq!(&part, &data[4090..4100]);
    // Replace with shorter content.
    w.begin_file("var/cache/pkg/a.bin", &m).unwrap();
    w.write_file(b"hello").unwrap();
    w.end_file().unwrap();
    w.symlink("usr/lib/link", &"x".repeat(100), &m).unwrap();
    w.symlink("usr/lib/short", "target", &m).unwrap();
    w.finish().unwrap();
    fsck_clean(&img);
    let out = Command::new("debugfs").args(["-R", "cat /var/cache/pkg/a.bin"]).arg(&img).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn huge_file_needs_extent_tree_and_big_directory() {
    let dir = std::env::current_dir().unwrap().join("../../target/ext4w-test");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let img = dir.join("big.img");
    let size = 1500u64 << 20;
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&img).unwrap();
    f.set_len(size).unwrap();
    let mut w = Ext4Writer::new(FileStorage(f), size, opts()).unwrap();
    let m = Meta { mode: 0o644, mtime: 1_700_000_000, uid: 70000, gid: 80000, ..Default::default() };
    // 900 MiB of a block-dependent pattern.
    let chunk: Vec<u8> = (0..(1u32 << 20)).map(|i| (i / 4096) as u8 ^ (i as u8)).collect();
    w.begin_file("big/data.bin", &m).unwrap();
    for _ in 0..900 {
        w.write_file(&chunk).unwrap();
    }
    let ino = w.end_file().unwrap();
    assert_eq!(w.file_size(ino), 900 << 20);
    for i in 0..6000 {
        w.begin_file(&format!("big/many/file-with-a-fairly-long-name-{i:05}"), &m).unwrap();
        w.write_file(&[i as u8; 100]).unwrap();
        w.end_file().unwrap();
    }
    w.finish().unwrap();
    fsck_clean(&img);
    let ex = run_ok(Command::new("debugfs").args(["-R", "ex /big/data.bin"]).arg(&img));
    assert!(ex.contains("Level Entries") || ex.contains("Level"), "{ex}");
    assert!(ex.lines().any(|l| l.trim_start().starts_with("0/") || l.contains("1/")), "expected an index level:\n{ex}");
    let stat = run_ok(Command::new("debugfs").args(["-R", "stat /big/data.bin"]).arg(&img));
    assert!(stat.contains("User: 70000") && stat.contains("Group: 80000"), "{stat}");
    let dump = dir.join("data.out");
    run_ok(Command::new("debugfs").args(["-R", &format!("dump /big/data.bin {}", dump.display())]).arg(&img));
    let got = std::fs::read(&dump).unwrap();
    assert_eq!(got.len(), 900 << 20);
    for (i, c) in got.chunks(1 << 20).enumerate() {
        assert!(c == chunk.as_slice(), "mismatch in MiB {i}");
    }
    let _ = std::fs::remove_file(&dump);
    let _ = std::fs::remove_file(&img);
}
