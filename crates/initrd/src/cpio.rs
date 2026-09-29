//! cpio "newc" archive writer.
use alloc::vec::Vec;

pub struct Writer {
    out: Vec<u8>,
    next_ino: u32,
}

const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFLNK: u32 = 0o120000;

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    pub fn new() -> Writer {
        Writer { out: Vec::new(), next_ino: 1 }
    }

    fn entry(&mut self, path: &str, mode: u32, rdev: (u32, u32), data: &[u8]) {
        let name = path.trim_start_matches('/');
        let namesize = name.len() + 1;
        let nlink = if mode & 0o170000 == S_IFDIR { 2 } else { 1 };
        let fields = [
            self.next_ino,
            mode,
            0,
            0,
            nlink,
            0,
            data.len() as u32,
            0,
            0,
            rdev.0,
            rdev.1,
            namesize as u32,
            0,
        ];
        self.next_ino += 1;
        self.out.extend_from_slice(b"070701");
        for f in fields {
            self.out.extend_from_slice(alloc::format!("{f:08X}").as_bytes());
        }
        self.out.extend_from_slice(name.as_bytes());
        self.out.push(0);
        self.pad();
        self.out.extend_from_slice(data);
        self.pad();
    }

    fn pad(&mut self) {
        while self.out.len() % 4 != 0 {
            self.out.push(0);
        }
    }

    pub fn dir(&mut self, path: &str, perm: u32) {
        self.entry(path, S_IFDIR | (perm & 0o7777), (0, 0), &[]);
    }

    pub fn file(&mut self, path: &str, perm: u32, data: &[u8]) {
        self.entry(path, S_IFREG | (perm & 0o7777), (0, 0), data);
    }

    pub fn symlink(&mut self, path: &str, target: &str) {
        self.entry(path, S_IFLNK | 0o777, (0, 0), target.as_bytes());
    }

    pub fn char_dev(&mut self, path: &str, perm: u32, major: u32, minor: u32) {
        self.entry(path, S_IFCHR | (perm & 0o7777), (major, minor), &[]);
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.entry("TRAILER!!!", 0, (0, 0), &[]);
        // Pad to a 512-byte boundary like cpio does.
        while self.out.len() % 512 != 0 {
            self.out.push(0);
        }
        self.out
    }
}
