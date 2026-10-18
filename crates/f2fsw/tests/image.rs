//! Integration test for F2FS filesystem generation.
//!
//! Creates an F2FS disk image with directories, files, symlinks, and device nodes,
//! verifies read-back capabilities, and inspects the binary layout of on-disk structures.

use disk::{FileKind, Meta, Storage};
use f2fsw::{
    f2fs_crc32, F2fsWriter, Options, F2FS_BLKSIZE,
    F2FS_MAJOR_VERSION, F2FS_MINOR_VERSION, F2FS_ROOT_INO, F2FS_SUPER_MAGIC, F2FS_SUPER_OFFSET,
    SB_CHKSUM_OFFSET,
};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

struct FileStorage(File);

impl Storage for FileStorage {
    fn write_at(&mut self, offset: u64, data: &[u8]) -> disk::Result<()> {
        self.0.seek(SeekFrom::Start(offset)).map_err(|_err| disk::Error::Io)?;
        self.0.write_all(data).map_err(|_err| disk::Error::Io)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> disk::Result<()> {
        self.0.seek(SeekFrom::Start(offset)).map_err(|_err| disk::Error::Io)?;
        self.0.read_exact(buf).map_err(|_err| disk::Error::Io)
    }
}

fn temp_image_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("f2fsw-test-{}-{name}.img", std::process::id()));
    drop(std::fs::remove_file(&p));
    p
}

#[test]
fn test_full_arch_root_f2fs_image() {
    let img_path = temp_image_path("arch-root");
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&img_path)
        .expect("create image file");

    let size_bytes = 64 * 1024 * 1024; // 64 MiB
    file.set_len(size_bytes).expect("set image length");

    let opts = Options {
        label: String::from("ARCH_ROOT"),
        uuid: [
            0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0, 0xE0,
            0xF0, 0xFF,
        ],
        now: 1710000000,
    };

    let mut fs = F2fsWriter::new(FileStorage(file), size_bytes, opts).expect("create f2fs writer");

    let dir_meta = Meta {
        mode: 0o755,
        uid: 0,
        gid: 0,
        mtime: 1710000000,
        xattrs: Vec::new(),
    };

    let file_meta = Meta {
        mode: 0o644,
        uid: 0,
        gid: 0,
        mtime: 1710000000,
        xattrs: Vec::new(),
    };

    // 1. Create directory structure
    let etc_id = fs.mkdir("etc", &dir_meta).unwrap();
    let _usr_id = fs.mkdir("usr", &dir_meta).unwrap();
    let bin_id = fs.mkdir("usr/bin", &dir_meta).unwrap();
    let lib_id = fs.mkdir("usr/lib", &dir_meta).unwrap();
    let _var_id = fs.mkdir("var", &dir_meta).unwrap();
    let _cache_id = fs.mkdir("var/cache", &dir_meta).unwrap();
    let _pacman_id = fs.mkdir("var/cache/pacman", &dir_meta).unwrap();
    let pkg_id = fs.mkdir("var/cache/pacman/pkg", &dir_meta).unwrap();
    let dev_id = fs.mkdir("dev", &dir_meta).unwrap();

    assert_eq!(fs.lookup("etc"), Some(etc_id));
    assert_eq!(fs.lookup("usr/bin"), Some(bin_id));
    assert_eq!(fs.lookup("usr/lib"), Some(lib_id));
    assert_eq!(fs.lookup("var/cache/pacman/pkg"), Some(pkg_id));
    assert_eq!(fs.lookup("dev"), Some(dev_id));

    // 2. Populate /etc files
    fs.begin_file("etc/hostname", &file_meta).unwrap();
    fs.write_file(b"archstaler-box\n").unwrap();
    let hostname_id = fs.end_file().unwrap();
    assert_eq!(fs.file_size(hostname_id), 15);

    let mut hostname_buf = [0u8; 15];
    fs.read_file(hostname_id, 0, &mut hostname_buf).unwrap();
    assert_eq!(&hostname_buf, b"archstaler-box\n");

    fs.begin_file("etc/os-release", &file_meta).unwrap();
    let os_release_content = b"NAME=\"Arch Linux\"\nID=arch\nPRETTY_NAME=\"Arch Linux\"\n";
    fs.write_file(os_release_content).unwrap();
    let os_rel_id = fs.end_file().unwrap();
    assert_eq!(fs.file_size(os_rel_id), os_release_content.len() as u64);

    // 3. Write a large binary file (> 32 KiB, multiple blocks)
    let mut large_bin = Vec::with_capacity(36000);
    for i in 0..36000 {
        large_bin.push(u8::try_from(i % 251).unwrap());
    }
    fs.begin_file("usr/bin/archstaler-bin", &file_meta).unwrap();
    fs.write_file(&large_bin[..10000]).unwrap();
    fs.write_file(&large_bin[10000..25000]).unwrap();
    fs.write_file(&large_bin[25000..]).unwrap();
    let bin_exec_id = fs.end_file().unwrap();
    assert_eq!(fs.file_size(bin_exec_id), 36000);

    let mut read_bin = vec![0u8; 36000];
    let n = fs.read_file(bin_exec_id, 0, &mut read_bin).unwrap();
    assert_eq!(n, 36000);
    assert_eq!(read_bin, large_bin);

    // Partial read from middle of second block
    let mut mid_buf = [0u8; 100];
    fs.read_file(bin_exec_id, 5000, &mut mid_buf).unwrap();
    assert_eq!(&mid_buf[..], &large_bin[5000..5100]);

    // 4. Create symlinks
    let bin_sym_id = fs.symlink("bin", "usr/bin", &dir_meta).unwrap();
    let lib_sym_id = fs.symlink("lib", "usr/lib", &dir_meta).unwrap();
    assert_eq!(fs.lookup("bin"), Some(bin_sym_id));
    assert_eq!(fs.lookup("lib"), Some(lib_sym_id));

    // 5. Create hardlinks
    let hard_id = fs.hardlink("etc/hostname.copy", "etc/hostname").unwrap();
    assert_eq!(hard_id, hostname_id);
    assert_eq!(fs.lookup("etc/hostname.copy"), Some(hostname_id));

    // 6. Create device nodes
    let null_id = fs.mknod("dev/null", FileKind::CharDev, 1, 3, &file_meta).unwrap();
    let zero_id = fs.mknod("dev/zero", FileKind::CharDev, 1, 5, &file_meta).unwrap();
    let sda_id = fs.mknod("dev/sda", FileKind::BlockDev, 8, 0, &file_meta).unwrap();
    assert_eq!(fs.lookup("dev/null"), Some(null_id));
    assert_eq!(fs.lookup("dev/zero"), Some(zero_id));
    assert_eq!(fs.lookup("dev/sda"), Some(sda_id));

    // 7. Finish filesystem
    fs.finish().expect("finish filesystem generation");

    // 8. Re-open and verify binary on-disk image
    let mut file = File::open(&img_path).expect("reopen image file");

    // Superblock 0 (Offset 1024)
    file.seek(SeekFrom::Start(F2FS_SUPER_OFFSET as u64)).unwrap();
    let mut sb0 = [0u8; 3072];
    file.read_exact(&mut sb0).unwrap();

    let magic = u32::from_le_bytes(sb0[0..4].try_into().unwrap());
    assert_eq!(magic, F2FS_SUPER_MAGIC);
    assert_eq!(&sb0[0..4], &[0x10, 0x20, 0xF5, 0xF2]);

    let major = u16::from_le_bytes(sb0[4..6].try_into().unwrap());
    let minor = u16::from_le_bytes(sb0[6..8].try_into().unwrap());
    assert_eq!(major, F2FS_MAJOR_VERSION);
    assert_eq!(minor, F2FS_MINOR_VERSION);

    let root_ino = u32::from_le_bytes(sb0[96..100].try_into().unwrap());
    assert_eq!(root_ino, F2FS_ROOT_INO);

    let crc0 = u32::from_le_bytes(sb0[SB_CHKSUM_OFFSET..SB_CHKSUM_OFFSET + 4].try_into().unwrap());
    let expected_crc0 = f2fs_crc32(F2FS_SUPER_MAGIC, &sb0[..SB_CHKSUM_OFFSET]);
    assert_eq!(crc0, expected_crc0);

    // Superblock 1 (Offset 1024 + 4096 = 5120)
    file.seek(SeekFrom::Start((F2FS_SUPER_OFFSET + F2FS_BLKSIZE) as u64)).unwrap();
    let mut sb1 = [0u8; 3072];
    file.read_exact(&mut sb1).unwrap();
    assert_eq!(sb0, sb1);

    // Checkpoint Pack 1 (Segment 1 = block 512 = 2 MiB)
    file.seek(SeekFrom::Start(512 * 4096)).unwrap();
    let mut cp1 = [0u8; 4096];
    file.read_exact(&mut cp1).unwrap();

    let cp_ver = u64::from_le_bytes(cp1[0..8].try_into().unwrap());
    assert_eq!(cp_ver, 1);

    let cp_crc = u32::from_le_bytes(cp1[4092..4096].try_into().unwrap());
    let expected_cp_crc = f2fs_crc32(F2FS_SUPER_MAGIC, &cp1[..4092]);
    assert_eq!(cp_crc, expected_cp_crc);

    // Cleanup temp file
    drop(std::fs::remove_file(&img_path));
}
