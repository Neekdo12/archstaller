//! Generic write-once filesystem interface (`FsWriter`).
//!
//! Provides a unified trait for bare-metal streaming filesystem generators
//! (`ext4w`, `btrfsw`, `xfsw`, `f2fsw`), decoupling the installer kernel from
//! filesystem-specific on-disk formats.

use crate::Result;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Regular,
    Dir,
    Symlink,
    CharDev,
    BlockDev,
    Fifo,
}

/// Ownership, permissions, and extended attributes of a filesystem entry.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    /// Permission bits (0o7777).
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
    pub xattrs: Vec<(String, Vec<u8>)>,
}

/// Byte-addressed backing store (e.g. a partition or disk region).
pub trait Storage {
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()>;
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;
}

/// Generic streaming filesystem writer contract.
pub trait FsWriter {
    /// Creates a directory at `path` with permissions `meta`.
    fn mkdir(&mut self, path: &str, meta: &Meta) -> Result<u64>;

    /// Starts writing a new regular file at `path`.
    fn begin_file(&mut self, path: &str, meta: &Meta) -> Result<u64>;

    /// Streams sequential data into the currently open file.
    fn write_file(&mut self, data: &[u8]) -> Result<()>;

    /// Finalizes the currently open file and returns its inode/object ID.
    fn end_file(&mut self) -> Result<u64>;

    /// Creates a symbolic link at `path` pointing to `target`.
    fn symlink(&mut self, path: &str, target: &str, meta: &Meta) -> Result<u64>;

    /// Creates a hard link at `path` pointing to existing file `existing`.
    fn hardlink(&mut self, path: &str, existing: &str) -> Result<u64>;

    /// Creates a device node (char or block) or a FIFO.
    fn mknod(&mut self, path: &str, kind: FileKind, major: u32, minor: u32, meta: &Meta) -> Result<u64>;

    /// Looks up an existing entry by path and returns its inode/object ID.
    fn lookup(&self, path: &str) -> Option<u64>;

    /// Returns the size in bytes of the file identified by `id`.
    fn file_size(&self, id: u64) -> u64;

    /// Reads data from a file previously written (used for package cache reads).
    fn read_file(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize>;

    /// Flushes all cached metadata, superblocks, and journals to disk.
    fn finish(&mut self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use alloc::collections::BTreeMap;
    use alloc::vec;

    struct MockFs {
        files: BTreeMap<String, (u64, Vec<u8>)>,
        current: Option<(String, Vec<u8>)>,
        next_id: u64,
        finished: bool,
    }

    impl MockFs {
        fn new() -> Self {
            Self {
                files: BTreeMap::new(),
                current: None,
                next_id: 1,
                finished: false,
            }
        }
    }

    impl FsWriter for MockFs {
        fn mkdir(&mut self, path: &str, _meta: &Meta) -> Result<u64> {
            let id = self.next_id;
            self.next_id += 1;
            self.files.insert(path.into(), (id, Vec::new()));
            Ok(id)
        }

        fn begin_file(&mut self, path: &str, _meta: &Meta) -> Result<u64> {
            if self.current.is_some() {
                return Err(Error::Invalid("file already open"));
            }
            self.current = Some((path.into(), Vec::new()));
            Ok(self.next_id)
        }

        fn write_file(&mut self, data: &[u8]) -> Result<()> {
            let Some((_, ref mut buf)) = self.current else {
                return Err(Error::Invalid("no open file"));
            };
            buf.extend_from_slice(data);
            Ok(())
        }

        fn end_file(&mut self) -> Result<u64> {
            let Some((path, buf)) = self.current.take() else {
                return Err(Error::Invalid("no open file"));
            };
            let id = self.next_id;
            self.next_id += 1;
            self.files.insert(path, (id, buf));
            Ok(id)
        }

        fn symlink(&mut self, path: &str, target: &str, _meta: &Meta) -> Result<u64> {
            let id = self.next_id;
            self.next_id += 1;
            self.files.insert(path.into(), (id, target.as_bytes().to_vec()));
            Ok(id)
        }

        fn hardlink(&mut self, path: &str, existing: &str) -> Result<u64> {
            let Some(&(id, ref buf)) = self.files.get(existing) else {
                return Err(Error::NotFound);
            };
            let buf_clone = buf.clone();
            self.files.insert(path.into(), (id, buf_clone));
            Ok(id)
        }

        fn mknod(&mut self, path: &str, _kind: FileKind, _major: u32, _minor: u32, _meta: &Meta) -> Result<u64> {
            let id = self.next_id;
            self.next_id += 1;
            self.files.insert(path.into(), (id, Vec::new()));
            Ok(id)
        }

        fn lookup(&self, path: &str) -> Option<u64> {
            self.files.get(path).map(|(id, _)| *id)
        }

        fn file_size(&self, id: u64) -> u64 {
            self.files
                .values()
                .find(|(i, _)| *i == id)
                .map_or(0, |(_, b)| b.len() as u64)
        }

        fn read_file(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize> {
            let Some((_, data)) = self.files.values().find(|(i, _)| *i == id) else {
                return Err(Error::NotFound);
            };
            if offset >= data.len() as u64 {
                return Ok(0);
            }
            let off = offset as usize;
            let n = (data.len() - off).min(buf.len());
            let Some(src) = data.get(off..off + n) else {
                return Ok(0);
            };
            if let Some(dst) = buf.get_mut(..n) {
                dst.copy_from_slice(src);
            }
            Ok(n)
        }

        fn finish(&mut self) -> Result<()> {
            self.finished = true;
            Ok(())
        }
    }

    #[test]
    fn fs_writer_workflow_and_edge_cases() {
        let mut fs = MockFs::new();
        let meta = Meta {
            mode: 0o755,
            uid: 0,
            gid: 0,
            mtime: 1234567,
            xattrs: vec![],
        };

        // 1. mkdir
        let dir_id = fs.mkdir("etc", &meta).unwrap();
        assert_eq!(dir_id, 1);
        assert_eq!(fs.lookup("etc"), Some(1));

        // 2. begin_file -> write_file -> end_file
        let file_meta = Meta {
            mode: 0o644,
            ..meta.clone()
        };
        fs.begin_file("etc/hostname", &file_meta).unwrap();
        fs.write_file(b"arch").unwrap();
        fs.write_file(b"box\n").unwrap();
        let file_id = fs.end_file().unwrap();
        assert_eq!(file_id, 2);
        assert_eq!(fs.file_size(file_id), 8);
        assert_eq!(fs.lookup("etc/hostname"), Some(2));

        // 3. read back
        let mut buf = [0u8; 8];
        let n = fs.read_file(file_id, 0, &mut buf).unwrap();
        assert_eq!(n, 8);
        assert_eq!(&buf, b"archbox\n");

        // Edge case: read at offset past EOF
        let mut eof_buf = [0u8; 4];
        let eof_n = fs.read_file(file_id, 100, &mut eof_buf).unwrap();
        assert_eq!(eof_n, 0);

        // Edge case: partial read
        let mut partial = [0u8; 3];
        let pn = fs.read_file(file_id, 4, &mut partial).unwrap();
        assert_eq!(pn, 3);
        assert_eq!(&partial, b"box");

        // 4. symlink
        let sym_id = fs.symlink("etc/localtime", "../usr/share/zoneinfo/UTC", &meta).unwrap();
        assert_eq!(fs.lookup("etc/localtime"), Some(sym_id));

        // 5. hardlink
        let hard_id = fs.hardlink("etc/hostname.hard", "etc/hostname").unwrap();
        assert_eq!(hard_id, file_id);
        assert_eq!(fs.lookup("etc/hostname.hard"), Some(file_id));

        // Edge case: hardlink to nonexistent file
        assert_eq!(fs.hardlink("bad", "nonexistent"), Err(Error::NotFound));

        // 6. finish
        assert!(!fs.finished);
        fs.finish().unwrap();
        assert!(fs.finished);
    }
}
