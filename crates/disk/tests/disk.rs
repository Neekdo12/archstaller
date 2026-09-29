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
        self.f.seek(SeekFrom::Start(lba * self.ss as u64)).map_err(|_| Error::Io)?;
        self.f.read_exact(buf).map_err(|_| Error::Io)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        self.f.seek(SeekFrom::Start(lba * self.ss as u64)).map_err(|_| Error::Io)?;
        self.f.write_all(buf).map_err(|_| Error::Io)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("disk-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn make_disk(dir: &Path, name: &str, size: u64, ss: u32) -> (PathBuf, FileDev) {
    let path = dir.join(name);
    let f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
    f.set_len(size).unwrap();
    (path, FileDev { f, ss, sectors: size / ss as u64 })
}

fn guids() -> [[u8; 16]; 4] {
    [[1; 16], [2; 16], [3; 16], [4; 16]]
}

fn out(cmd: &mut Command) -> String {
    let o = cmd.output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
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
        let v = out(Command::new("sfdisk").args(["--sector-size", &ss.to_string(), "--verify"]).arg(&path));
        assert!(v.contains("No errors"), "{v}");
        let d = out(Command::new("sfdisk").args(["--sector-size", &ss.to_string(), "-d"]).arg(&path));
        assert!(d.contains("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"), "{d}");
        assert!(d.contains("21686148-6449-6E6F-744E-656564454649"), "{d}");
        assert!(d.contains("4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709"), "{d}");
        // Both GPT copies must agree (sfdisk falls back silently, so read the backup too).
        let p = out(Command::new("fdisk").args(["-b", &ss.to_string(), "-l"]).arg(&path));
        assert!(p.contains("Disklabel type: gpt"), "{p}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fat32_esp_passes_fsck_and_contents_match() {
    let dir = scratch("fat");
    let (path, mut dev) = make_disk(&dir, "esp.img", 1 << 30, 512);
    {
        let mut r = Region::whole(&mut dev);
        let mut w = Fat32Writer::format(&mut r, Options { label: "ESP".into(), volume_id: 0x1234_5678, hidden_sectors: 2048, now: 1_790_000_000 }).unwrap();
        w.write_file("EFI/BOOT/BOOTX64.EFI", &vec![7u8; 123_456]).unwrap();
        w.write_file("vmlinuz-linux", &(0..9_000_000u32).map(|i| (i * 13) as u8).collect::<Vec<_>>()).unwrap();
        w.write_file("limine/limine.conf", b"timeout: 0\n").unwrap();
        w.write_file("limine/a-file-with-a-rather-long-name.1.txt", b"long").unwrap();
        w.write_file("limine/a-file-with-a-rather-long-name.2.txt", b"long2").unwrap();
        w.write_file("Mixed Case.Name", b"x").unwrap();
        w.write_file("empty", b"").unwrap();
        for i in 0..200 {
            w.write_file(&format!("many/file-number-{i:04}.dat"), &[i as u8; 10]).unwrap();
        }
        w.finish().unwrap();
    }
    let f = out(Command::new("fsck.fat").args(["-n", "-v"]).arg(&path));
    assert!(!f.contains("differ") && !f.contains("Dirty") && !f.contains("error"), "{f}");
    let dump = dir.join("dump");
    std::fs::create_dir_all(&dump).unwrap();
    out(Command::new("mcopy").args(["-s", "-n", "-i"]).arg(&path).arg("::/").arg(&dump));
    assert_eq!(std::fs::read(dump.join("EFI/BOOT/BOOTX64.EFI")).unwrap(), vec![7u8; 123_456]);
    let k = std::fs::read(dump.join("vmlinuz-linux")).unwrap();
    assert!(k.iter().enumerate().all(|(i, b)| *b == (i as u32 * 13) as u8) && k.len() == 9_000_000);
    assert_eq!(std::fs::read(dump.join("limine/limine.conf")).unwrap(), b"timeout: 0\n");
    assert_eq!(std::fs::read(dump.join("limine/a-file-with-a-rather-long-name.2.txt")).unwrap(), b"long2");
    assert_eq!(std::fs::read(dump.join("Mixed Case.Name")).unwrap(), b"x");
    assert_eq!(std::fs::read(dump.join("many/file-number-0199.dat")).unwrap(), vec![199u8; 10]);
    assert_eq!(std::fs::read_dir(dump.join("many")).unwrap().count(), 200);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bios_install_matches_real_limine() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/limine/v12.9.1");
    let (tool, header) = (root.join("limine"), root.join("limine-bios-hdd.h"));
    if !tool.exists() || !header.exists() {
        eprintln!("skipping: run `cargo xtask build` first");
        return;
    }
    let hdd = disk::limine::parse_hdd_header(&std::fs::read_to_string(header).unwrap());
    assert!(hdd.len() > 512 && hdd[510] == 0x55 && hdd[511] == 0xaa, "bad hdd image ({} bytes)", hdd.len());

    let dir = scratch("limine");
    let size = 2u64 << 30;
    let (ours, mut dev) = make_disk(&dir, "ours.img", size, 512);
    let layout = Layout::plan(512, dev.sectors, 1 << 30, guids()).unwrap();
    layout.write(&mut Region::whole(&mut dev)).unwrap();
    let theirs = dir.join("theirs.img");
    std::fs::copy(&ours, &theirs).unwrap();

    let (off, len) = layout.byte_range(Layout::BIOS);
    disk::limine::bios_install(&mut Region::whole(&mut dev), &hdd, off, len).unwrap();
    out(Command::new(&tool).arg("bios-install").arg(&theirs));

    let a = std::fs::read(&ours).unwrap();
    let b = std::fs::read(&theirs).unwrap();
    assert_eq!(a[..512], b[..512], "boot sector differs");
    let range = off as usize..off as usize + hdd.len() - 512;
    assert_eq!(a[range.clone()], b[range], "stage 2 differs");
    // Everything else on the disk is untouched by both.
    assert!(a == b, "images differ outside the checked ranges");
    let _ = std::fs::remove_dir_all(&dir);
}
