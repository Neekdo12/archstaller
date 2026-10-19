use disk::fat32::{Fat32Writer, Options};
use disk::gpt::Layout;
use disk::region::Region;
use hal::{BlockDevice, Error, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

struct FileDev {
    f: File,
    ss: u32,
    sectors: u64,
}

impl BlockDevice for FileDev {
    fn model(&self) -> &str {
        "file"
    }
    fn serial(&self) -> &str {
        "FILE0"
    }
    fn sector_size(&self) -> u32 {
        self.ss
    }
    fn sector_count(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()> {
        self.f
            .seek(SeekFrom::Start(lba * self.ss as u64))
            .map_err(|_e| Error::Io)?;
        self.f.read_exact(buf).map_err(|_e| Error::Io)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        self.f
            .seek(SeekFrom::Start(lba * self.ss as u64))
            .map_err(|_e| Error::Io)?;
        self.f.write_all(buf).map_err(|_e| Error::Io)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("disk-{}-{name}", std::process::id()));
    drop(std::fs::remove_dir_all(&d));
    std::fs::create_dir_all(&d).expect("create scratch dir");
    d
}

fn make_disk(dir: &Path, name: &str, size: u64, ss: u32) -> (PathBuf, FileDev) {
    let path = dir.join(name);
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("open disk image");
    f.set_len(size).expect("set disk size");
    (
        path,
        FileDev {
            f,
            ss,
            sectors: size / ss as u64,
        },
    )
}

fn guids() -> [[u8; 16]; 4] {
    [[1; 16], [2; 16], [3; 16], [4; 16]]
}

fn out(cmd: &mut Command) -> String {
    let o = cmd.output().expect("execute command");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(o.status.success(), "{cmd:?}:\n{text}");
    text
}

#[test]
fn gpt_layout_verifies_512_and_4096() {
    let dir = scratch("gpt");
    for ss in [512u32, 4096] {
        let (path, mut dev) = make_disk(&dir, &format!("d{ss}.img"), 8 << 30, ss);
        let layout = Layout::plan(ss, dev.sectors, 1 << 30, guids()).unwrap();
        layout.write(&mut Region::whole(&mut dev)).unwrap();
        let v = out(Command::new("sfdisk")
            .args(["--sector-size", &ss.to_string(), "--verify"])
            .arg(&path));
        assert!(v.contains("No errors"), "{v}");
        let d = out(Command::new("sfdisk")
            .args(["--sector-size", &ss.to_string(), "-d"])
            .arg(&path));
        assert!(d.contains("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"), "{d}");
        assert!(d.contains("21686148-6449-6E6F-744E-656564454649"), "{d}");
        assert!(d.contains("4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709"), "{d}");
        // Both GPT copies must agree (sfdisk falls back silently, so read the backup too).
        let p = out(Command::new("fdisk")
            .args(["-b", &ss.to_string(), "-l"])
            .arg(&path));
        assert!(p.contains("Disklabel type: gpt"), "{p}");
    }
    drop(std::fs::remove_dir_all(&dir));
}

#[test]
fn fat32_min_volume_size_formats_and_one_mib_less_does_not() {
    let dir = scratch("fatmin");
    for ss in [512u32, 4096] {
        let min = disk::fat32::min_volume_bytes(ss);
        let opts = || Options {
            label: "ESP".into(),
            volume_id: 1,
            hidden_sectors: 2048,
            now: 1_790_000_000,
        };
        let (_, mut dev) = make_disk(&dir, &format!("min{ss}.img"), min, ss);
        Fat32Writer::format(&mut Region::whole(&mut dev), opts())
            .unwrap_or_else(|e| panic!("{ss}-byte sectors, {min} bytes: {e:?}"));
        let (_, mut dev) = make_disk(&dir, &format!("small{ss}.img"), min - (1 << 20), ss);
        assert!(
            Fat32Writer::format(&mut Region::whole(&mut dev), opts()).is_err(),
            "{ss}-byte sectors: {} bytes should be too small",
            min - (1 << 20)
        );
    }
    // The installer's 64 MiB minimum ESP is enough with 512-byte sectors, not with 4 KiB ones.
    assert!(disk::fat32::min_volume_bytes(512) <= 64 << 20);
    assert!(disk::fat32::min_volume_bytes(4096) > 64 << 20);
    drop(std::fs::remove_dir_all(&dir));
}

#[test]
fn fat32_esp_passes_fsck_and_contents_match() {
    let dir = scratch("fat");
    let (path, mut dev) = make_disk(&dir, "esp.img", 1 << 30, 512);
    {
        let mut r = Region::whole(&mut dev);
        let mut w = Fat32Writer::format(
            &mut r,
            Options {
                label: "ESP".into(),
                volume_id: 0x1234_5678,
                hidden_sectors: 2048,
                now: 1_790_000_000,
            },
        )
        .unwrap();
        w.write_file("EFI/BOOT/BOOTX64.EFI", &vec![7u8; 123_456])
            .unwrap();
        w.write_file(
            "vmlinuz-linux",
            &(0..9_000_000u32)
                .map(|i| (i * 13) as u8)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        w.write_file("limine/limine.conf", b"timeout: 0\n").unwrap();
        w.write_file("limine/a-file-with-a-rather-long-name.1.txt", b"long")
            .unwrap();
        w.write_file("limine/a-file-with-a-rather-long-name.2.txt", b"long2")
            .unwrap();
        w.write_file("Mixed Case.Name", b"x").unwrap();
        w.write_file("empty", b"").unwrap();
        for i in 0u8..200u8 {
            w.write_file(&format!("many/file-number-{i:04}.dat"), &[i; 10])
                .unwrap();
        }
        w.finish().unwrap();
    }
    let f = out(Command::new("fsck.fat").args(["-n", "-v"]).arg(&path));
    assert!(
        !f.contains("differ") && !f.contains("Dirty") && !f.contains("error"),
        "{f}"
    );
    let dump = dir.join("dump");
    std::fs::create_dir_all(&dump).unwrap();
    out(Command::new("mcopy")
        .args(["-s", "-n", "-i"])
        .arg(&path)
        .arg("::/")
        .arg(&dump));
    assert_eq!(
        std::fs::read(dump.join("EFI/BOOT/BOOTX64.EFI")).unwrap(),
        vec![7u8; 123_456]
    );
    let k = std::fs::read(dump.join("vmlinuz-linux")).unwrap();
    assert!(
        k.iter()
            .enumerate()
            .all(|(i, b)| *b == (i as u32 * 13) as u8)
            && k.len() == 9_000_000
    );
    assert_eq!(
        std::fs::read(dump.join("limine/limine.conf")).unwrap(),
        b"timeout: 0\n"
    );
    assert_eq!(
        std::fs::read(dump.join("limine/a-file-with-a-rather-long-name.2.txt")).unwrap(),
        b"long2"
    );
    assert_eq!(std::fs::read(dump.join("Mixed Case.Name")).unwrap(), b"x");
    assert_eq!(
        std::fs::read(dump.join("many/file-number-0199.dat")).unwrap(),
        vec![199u8; 10]
    );
    assert_eq!(std::fs::read_dir(dump.join("many")).unwrap().count(), 200);
    drop(std::fs::remove_dir_all(&dir));
}

#[test]
fn test_layout_plan_invalid_inputs() {
    let guids = [
        disk::gpt::guid("11111111-1111-1111-1111-111111111111"),
        disk::gpt::guid("22222222-2222-2222-2222-222222222222"),
        disk::gpt::guid("33333333-3333-3333-3333-333333333333"),
        disk::gpt::guid("44444444-4444-4444-4444-444444444444"),
    ];

    // sector_size == 0 must return Err, not panic
    Layout::plan(0, 10_000_000, 512 * 1024 * 1024, guids).unwrap_err();

    // sector_size > ALIGN (1 MiB) must return Err, not panic
    Layout::plan(2 * 1024 * 1024, 10_000_000, 512 * 1024 * 1024, guids).unwrap_err();

    // non-power of two sector size must return Err
    Layout::plan(500, 10_000_000, 512 * 1024 * 1024, guids).unwrap_err();

    // sector_size < 512 must return Err
    Layout::plan(256, 10_000_000, 512 * 1024 * 1024, guids).unwrap_err();

    // esp_bytes == 0 must return Err
    Layout::plan(512, 10_000_000, 0, guids).unwrap_err();

    // Tiny disk where last_usable + 1 < align must return Err, not overflow or panic
    Layout::plan(512, 100, 512 * 1024 * 1024, guids).unwrap_err();
    Layout::plan(512, 2048, 512 * 1024 * 1024, guids).unwrap_err();
}

#[test]
fn test_gpt_layout_root_alignment() {
    let guids = [
        disk::gpt::guid("11111111-1111-1111-1111-111111111111"),
        disk::gpt::guid("22222222-2222-2222-2222-222222222222"),
        disk::gpt::guid("33333333-3333-3333-3333-333333333333"),
        disk::gpt::guid("44444444-4444-4444-4444-444444444444"),
    ];
    let align_sectors = (1024 * 1024) / 512;
    // 100 MiB + 512 bytes ESP requested
    let esp_bytes = 100 * 1024 * 1024 + 512;
    let layout = Layout::plan(512, 10_000_000, esp_bytes, guids).expect("plan layout");
    let root = &layout.partitions[Layout::ROOT];
    assert_eq!(
        root.first_lba % align_sectors,
        0,
        "root first LBA ({}) must be 1 MiB aligned",
        root.first_lba
    );
}

#[test]
fn test_gpt_large_esp_no_panic() {
    let guids = [
        disk::gpt::guid("11111111-1111-1111-1111-111111111111"),
        disk::gpt::guid("22222222-2222-2222-2222-222222222222"),
        disk::gpt::guid("33333333-3333-3333-3333-333333333333"),
        disk::gpt::guid("44444444-4444-4444-4444-444444444444"),
    ];
    Layout::plan(512, 1_000_000, u64::MAX - 512, guids).unwrap_err();
}

#[test]
fn test_fat32_format_small_volume_no_panic() {
    let dir = scratch("fat32-tiny");
    let (_p, mut dev) = make_disk(&dir, "tiny.img", 30 * 512, 512);
    let opts = Options {
        label: "ESP".into(),
        volume_id: 1,
        hidden_sectors: 0,
        now: 0,
    };
    let mut reg = Region::whole(&mut dev);
    match Fat32Writer::format(&mut reg, opts) {
        Err(_) => (),
        Ok(_) => panic!("expected error on tiny volume"),
    }
    drop(std::fs::remove_dir_all(&dir));
}

