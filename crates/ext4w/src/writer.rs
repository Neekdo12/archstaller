use crate::crc::crc32c;
use crate::{Error, Result, Storage};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

const BS: usize = 4096;
const BPG: u64 = 32768;
const ISIZE: usize = 256;
const EXTRA_ISIZE: u16 = 32;
const FIRST_INO: u32 = 11;
const ROOT_INO: u32 = 2;
const JOURNAL_INO: u32 = 8;
const LPF_INO: u32 = 11;
const LPF_BLOCKS: usize = 4;
const MAX_EXTENT_LEN: u64 = 32768;
const FLUSH_BYTES: usize = 1 << 20;
const DIR_TAIL: usize = 12;

const FL_EXTENTS: u32 = 0x8_0000;

const S_IFREG: u16 = 0o100000;
const S_IFDIR: u16 = 0o040000;
const S_IFLNK: u16 = 0o120000;
const S_IFCHR: u16 = 0o020000;
const S_IFBLK: u16 = 0o060000;
const S_IFIFO: u16 = 0o010000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Regular,
    Dir,
    Symlink,
    CharDev,
    BlockDev,
    Fifo,
}

impl FileKind {
    fn mode_bits(self) -> u16 {
        match self {
            FileKind::Regular => S_IFREG,
            FileKind::Dir => S_IFDIR,
            FileKind::Symlink => S_IFLNK,
            FileKind::CharDev => S_IFCHR,
            FileKind::BlockDev => S_IFBLK,
            FileKind::Fifo => S_IFIFO,
        }
    }
    fn dirent_type(self) -> u8 {
        match self {
            FileKind::Regular => 1,
            FileKind::Dir => 2,
            FileKind::CharDev => 3,
            FileKind::BlockDev => 4,
            FileKind::Fifo => 5,
            FileKind::Symlink => 7,
        }
    }
}

/// Ownership, permissions and attributes of a new entry.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    /// Permission bits (0o7777).
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
    pub xattrs: Vec<(String, Vec<u8>)>,
}

pub struct Options {
    pub label: String,
    pub uuid: [u8; 16],
    pub hash_seed: [u32; 4],
    /// Unix time used for mkfs and creation timestamps.
    pub now: u64,
    /// Percent of blocks reserved for root.
    pub reserved_percent: u64,
}

#[derive(Clone, Copy, Debug)]
struct Extent {
    lblk: u32,
    len: u32,
    pblk: u64,
}

struct Inode {
    kind: FileKind,
    mode: u16,
    uid: u32,
    gid: u32,
    size: u64,
    mtime: u64,
    nlink: u32,
    extents: Vec<Extent>,
    rdev: (u32, u32),
    fast_symlink: Vec<u8>,
    xattrs: Vec<(String, Vec<u8>)>,
    children: BTreeMap<Vec<u8>, (u32, FileKind)>,
    parent: u32,
    min_dir_blocks: usize,
    // Filled in by `finish`.
    i_block: [u8; 60],
    xattr_block: u64,
    meta_blocks: u64,
}

impl Inode {
    fn new(kind: FileKind, meta: &Meta) -> Inode {
        Inode {
            kind,
            mode: kind.mode_bits() | (meta.mode & 0o7777),
            uid: meta.uid,
            gid: meta.gid,
            size: 0,
            mtime: meta.mtime,
            nlink: 1,
            extents: Vec::new(),
            rdev: (0, 0),
            fast_symlink: Vec::new(),
            xattrs: meta.xattrs.clone(),
            children: BTreeMap::new(),
            parent: 0,
            min_dir_blocks: 1,
            i_block: [0; 60],
            xattr_block: 0,
            meta_blocks: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct GroupLayout {
    start: u64,
    block_bitmap: u64,
    inode_bitmap: u64,
    inode_table: u64,
    data_start: u64,
    end: u64,
}

struct OpenFile {
    ino: u32,
    buf: Vec<u8>,
    size: u64,
    next_lblk: u32,
}

pub struct Ext4Writer<S: Storage> {
    st: S,
    opts: Options,
    blocks: u64,
    groups: u32,
    ipg: u32,
    itable_blocks: u64,
    gdt_blocks: u64,
    csum_seed: u32,
    inodes: Vec<Option<Inode>>,
    next_ino: u32,
    cur_group: u32,
    cursor: u64,
    /// Block bitmaps per group (metadata and out-of-range padding bits already set).
    bitmaps: Vec<Vec<u8>>,
    free_blocks: Vec<u64>,
    open: Option<OpenFile>,
}

fn put16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// Journal size in blocks for a filesystem of `blocks` 4 KiB blocks, as `mke2fs` chooses it (capped at
/// 64 MiB so the journal fits at most a few extents). Filesystems under 32 MiB get none.
fn journal_blocks(blocks: u64) -> u64 {
    match blocks {
        0..=8191 => 0,
        8192..=32767 => 1024,
        32768..=262143 => 4096,
        262144..=524287 => 8192,
        _ => 16384,
    }
}

fn put_be32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_be_bytes());
}

fn is_power_of(mut n: u32, base: u32) -> bool {
    while n % base == 0 {
        n /= base;
    }
    n == 1
}

fn has_backup(g: u32) -> bool {
    g == 0 || g == 1 || is_power_of(g, 3) || is_power_of(g, 5) || is_power_of(g, 7)
}

fn round_up(v: u64, to: u64) -> u64 {
    v.div_ceil(to) * to
}

fn components(path: &str) -> Result<Vec<&str>> {
    let mut out = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => return Err(Error::Invalid("'..' in path")),
            c => {
                if c.len() > 255 {
                    return Err(Error::Invalid("name too long"));
                }
                out.push(c)
            }
        }
    }
    Ok(out)
}

impl<S: Storage> Ext4Writer<S> {
    /// Lays out a filesystem of `size_bytes` (rounded down to whole blocks) and creates the
    /// root directory and `lost+found`.
    pub fn new(st: S, size_bytes: u64, opts: Options) -> Result<Self> {
        let mut blocks = size_bytes / BS as u64;
        if blocks < 2048 || blocks >= u32::MAX as u64 {
            return Err(Error::Invalid("unsupported filesystem size"));
        }
        let mut groups = blocks.div_ceil(BPG) as u32;
        // Inodes per group: one per 16 KiB overall, capped at what fits a bitmap block.
        let want = ((blocks * BS as u64 / 16384).div_ceil(groups as u64)).clamp(128, 8192);
        let ipg = round_up(want, 16) as u32;
        let itable_blocks = ipg as u64 * ISIZE as u64 / BS as u64;
        let gdt_blocks = (groups as u64 * 32).div_ceil(BS as u64);
        // Drop a runt last group that cannot even hold its own metadata.
        let last_len = blocks - (groups as u64 - 1) * BPG;
        let min_last = 1 + gdt_blocks + 2 + itable_blocks + 64;
        if groups > 1 && last_len < min_last {
            groups -= 1;
            blocks = groups as u64 * BPG;
        }
        let gdt_blocks = (groups as u64 * 32).div_ceil(BS as u64);
        let csum_seed = crc32c(!0, &opts.uuid);
        let mut w = Ext4Writer {
            st,
            opts,
            blocks,
            groups,
            ipg,
            itable_blocks,
            gdt_blocks,
            csum_seed,
            inodes: Vec::new(),
            next_ino: FIRST_INO + 1,
            cur_group: 0,
            cursor: 0,
            bitmaps: Vec::new(),
            free_blocks: Vec::new(),
            open: None,
        };
        for g in 0..groups {
            let l = w.layout(g);
            let mut bm = vec![0u8; BS];
            let group_blocks = l.end - l.start;
            for bit in 0..(l.data_start - l.start) as usize {
                bm[bit / 8] |= 1 << (bit % 8);
            }
            for bit in group_blocks as usize..BPG as usize {
                bm[bit / 8] |= 1 << (bit % 8);
            }
            w.free_blocks.push(group_blocks - (l.data_start - l.start));
            w.bitmaps.push(bm);
        }
        w.cursor = w.layout(0).data_start;
        w.inodes.resize_with(FIRST_INO as usize + 1, || None);

        let now = w.opts.now;
        let mut root = Inode::new(FileKind::Dir, &Meta { mode: 0o755, mtime: now, ..Default::default() });
        root.parent = ROOT_INO;
        root.nlink = 2;
        let mut lpf = Inode::new(FileKind::Dir, &Meta { mode: 0o700, mtime: now, ..Default::default() });
        lpf.parent = ROOT_INO;
        lpf.nlink = 2;
        lpf.min_dir_blocks = LPF_BLOCKS;
        root.children.insert(b"lost+found".to_vec(), (LPF_INO, FileKind::Dir));
        root.nlink += 1;
        w.inodes[ROOT_INO as usize] = Some(root);
        w.inodes[LPF_INO as usize] = Some(lpf);
        let jb = journal_blocks(w.blocks);
        if jb > 0 {
            w.add_journal(jb)?;
        }
        Ok(w)
    }

    /// Creates the internal journal: contiguous blocks mapped by inode 8 and a version 2 jbd2
    /// superblock in its first block (empty journal: `s_start` 0), as `mke2fs -j` writes it. The
    /// filesystem superblock gets the journal feature and a backup of the inode's block map.
    fn add_journal(&mut self, jblocks: u64) -> Result<()> {
        let mut extents = Vec::new();
        let mut done = 0u64;
        while done < jblocks {
            let (pblk, len) = self.alloc_run(jblocks - done)?;
            extents.push(Extent { lblk: done as u32, len: len as u32, pblk });
            done += len;
        }
        if extents.len() > 4 {
            return Err(Error::Invalid("journal too fragmented"));
        }
        let mut j = Inode::new(FileKind::Regular, &Meta { mode: 0o600, mtime: self.opts.now, ..Default::default() });
        j.size = jblocks * BS as u64;
        j.extents = extents.clone();
        self.inodes[JOURNAL_INO as usize] = Some(j);
        self.build_extent_root(JOURNAL_INO)?;

        let mut jsb = vec![0u8; BS];
        put_be32(&mut jsb, 0, 0xC03B_3998); // JBD2 magic
        put_be32(&mut jsb, 4, 4); // superblock, version 2
        put_be32(&mut jsb, 12, BS as u32); // block size
        put_be32(&mut jsb, 16, jblocks as u32); // length of the journal in blocks
        put_be32(&mut jsb, 20, 1); // first log block
        put_be32(&mut jsb, 24, 1); // first expected commit sequence
        // s_start (28) stays 0: the journal is empty and needs no recovery.
        jsb[48..64].copy_from_slice(&self.opts.uuid);
        put_be32(&mut jsb, 64, 1); // users
        self.st.write_at(extents[0].pblk * BS as u64, &jsb)?;
        Ok(())
    }


    pub fn into_storage(self) -> S {
        self.st
    }

    pub fn total_blocks(&self) -> u64 {
        self.blocks
    }

    // ---- geometry ----

    fn layout(&self, g: u32) -> GroupLayout {
        let start = g as u64 * BPG;
        let sb_blocks = if has_backup(g) { 1 + self.gdt_blocks } else { 0 };
        let block_bitmap = start + sb_blocks;
        let inode_bitmap = block_bitmap + 1;
        let inode_table = inode_bitmap + 1;
        GroupLayout {
            start,
            block_bitmap,
            inode_bitmap,
            inode_table,
            data_start: inode_table + self.itable_blocks,
            end: (start + BPG).min(self.blocks),
        }
    }

    /// Next run of at most `want` contiguous free blocks, first fit from the allocation cursor.
    fn alloc_run(&mut self, want: u64) -> Result<(u64, u64)> {
        loop {
            let g = self.cur_group;
            let l = self.layout(g);
            let group_blocks = (l.end - l.start) as usize;
            let mut rel = (self.cursor.max(l.start) - l.start) as usize;
            let bm = &mut self.bitmaps[g as usize];
            // Skip used blocks.
            while rel < group_blocks {
                if rel % 8 == 0 && bm[rel / 8] == 0xff {
                    rel += 8;
                } else if bm[rel / 8] & (1 << (rel % 8)) != 0 {
                    rel += 1;
                } else {
                    break;
                }
            }
            if rel >= group_blocks {
                if self.cur_group + 1 >= self.groups {
                    return Err(Error::NoSpace);
                }
                self.cur_group += 1;
                self.cursor = self.layout(self.cur_group).start;
                continue;
            }
            let start = rel;
            let max = want.min(MAX_EXTENT_LEN) as usize;
            let mut len = 0;
            while len < max && start + len < group_blocks && bm[(start + len) / 8] & (1 << ((start + len) % 8)) == 0 {
                bm[(start + len) / 8] |= 1 << ((start + len) % 8);
                len += 1;
            }
            self.free_blocks[g as usize] -= len as u64;
            self.cursor = l.start + (start + len) as u64;
            return Ok((l.start + start as u64, len as u64));
        }
    }

    fn free_extents(&mut self, extents: &[Extent]) {
        for e in extents {
            for b in e.pblk..e.pblk + e.len as u64 {
                let g = (b / BPG) as usize;
                let rel = (b % BPG) as usize;
                self.bitmaps[g][rel / 8] &= !(1 << (rel % 8));
                self.free_blocks[g] += 1;
            }
            if e.pblk < self.cursor {
                self.cursor = e.pblk;
                self.cur_group = (e.pblk / BPG) as u32;
            }
        }
    }

    fn alloc_one(&mut self) -> Result<u64> {
        Ok(self.alloc_run(1)?.0)
    }

    fn group_of_ino(&self, ino: u32) -> u32 {
        (ino - 1) / self.ipg
    }

    // ---- namespace ----

    fn dir(&self, ino: u32) -> &Inode {
        self.inodes[ino as usize].as_ref().unwrap()
    }

    fn dir_mut(&mut self, ino: u32) -> &mut Inode {
        self.inodes[ino as usize].as_mut().unwrap()
    }

    fn lookup_in(&self, dir: u32, name: &str) -> Option<(u32, FileKind)> {
        self.dir(dir).children.get(name.as_bytes()).copied()
    }

    fn alloc_ino(&mut self) -> Result<u32> {
        let ino = self.next_ino;
        if ino as u64 > self.ipg as u64 * self.groups as u64 {
            return Err(Error::NoInodes);
        }
        self.next_ino += 1;
        if self.inodes.len() <= ino as usize {
            self.inodes.resize_with(ino as usize + 1, || None);
        }
        Ok(ino)
    }

    /// Resolves the parent directory of `path`, creating missing directories.
    fn parent_of<'p>(&mut self, path: &'p str) -> Result<(u32, &'p str)> {
        let comps = components(path)?;
        let (name, dirs) = comps.split_last().ok_or(Error::Invalid("empty path"))?;
        let mut cur = ROOT_INO;
        for d in dirs {
            cur = match self.lookup_in(cur, d) {
                Some((ino, FileKind::Dir)) => ino,
                Some(_) => return Err(Error::NotDir),
                None => self.new_dir(cur, d, &Meta { mode: 0o755, mtime: self.opts.now, ..Default::default() })?,
            };
        }
        Ok((cur, name))
    }

    fn new_dir(&mut self, parent: u32, name: &str, meta: &Meta) -> Result<u32> {
        let ino = self.alloc_ino()?;
        let mut d = Inode::new(FileKind::Dir, meta);
        d.parent = parent;
        d.nlink = 2;
        self.inodes[ino as usize] = Some(d);
        let p = self.dir_mut(parent);
        p.children.insert(name.as_bytes().to_vec(), (ino, FileKind::Dir));
        p.nlink += 1;
        Ok(ino)
    }

    fn link_new(&mut self, parent: u32, name: &str, kind: FileKind, ino: u32) {
        self.dir_mut(parent).children.insert(name.as_bytes().to_vec(), (ino, kind));
    }

    /// Creates `path` as a directory, or updates the attributes of an existing one.
    pub fn mkdir(&mut self, path: &str, meta: &Meta) -> Result<u32> {
        if components(path)?.is_empty() {
            self.apply_meta(ROOT_INO, meta);
            return Ok(ROOT_INO);
        }
        let (parent, name) = self.parent_of(path)?;
        match self.lookup_in(parent, name) {
            Some((ino, FileKind::Dir)) => {
                self.apply_meta(ino, meta);
                Ok(ino)
            }
            Some(_) => Err(Error::Exists),
            None => self.new_dir(parent, name, meta),
        }
    }

    fn apply_meta(&mut self, ino: u32, meta: &Meta) {
        let n = self.dir_mut(ino);
        n.mode = (n.mode & 0o170000) | (meta.mode & 0o7777);
        n.uid = meta.uid;
        n.gid = meta.gid;
        n.mtime = meta.mtime;
        n.xattrs = meta.xattrs.clone();
    }

    /// Removes an existing non-directory entry so it can be replaced (its blocks stay allocated).
    fn replace_slot(&mut self, parent: u32, name: &str) -> Result<()> {
        match self.lookup_in(parent, name) {
            Some((_, FileKind::Dir)) => Err(Error::IsDir),
            Some((ino, _)) => {
                self.dir_mut(parent).children.remove(name.as_bytes());
                let n = self.dir_mut(ino);
                n.nlink = n.nlink.saturating_sub(1);
                if n.nlink == 0 {
                    let gone = self.inodes[ino as usize].take().unwrap();
                    self.free_extents(&gone.extents);
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Starts a regular file; follow with [`write_file`](Self::write_file) and
    /// [`end_file`](Self::end_file). Only one file can be open at a time.
    pub fn begin_file(&mut self, path: &str, meta: &Meta) -> Result<u32> {
        if self.open.is_some() {
            return Err(Error::Invalid("a file is already open"));
        }
        let (parent, name) = self.parent_of(path)?;
        self.replace_slot(parent, name)?;
        let ino = self.alloc_ino()?;
        self.inodes[ino as usize] = Some(Inode::new(FileKind::Regular, meta));
        self.link_new(parent, name, FileKind::Regular, ino);
        self.open = Some(OpenFile { ino, buf: Vec::new(), size: 0, next_lblk: 0 });
        Ok(ino)
    }

    pub fn write_file(&mut self, data: &[u8]) -> Result<()> {
        let of = self.open.as_mut().ok_or(Error::Invalid("no open file"))?;
        of.buf.extend_from_slice(data);
        of.size += data.len() as u64;
        if of.buf.len() >= FLUSH_BYTES {
            self.flush_open(false)?;
        }
        Ok(())
    }

    fn flush_open(&mut self, last: bool) -> Result<()> {
        let mut of = self.open.take().unwrap();
        let mut buf = core::mem::take(&mut of.buf);
        if last && buf.len() % BS != 0 {
            buf.resize(round_up(buf.len() as u64, BS as u64) as usize, 0);
        }
        let full = buf.len() / BS * BS;
        let mut done = 0;
        let mut result = Ok(());
        while done < full {
            let want = ((full - done) / BS) as u64;
            match self.alloc_run(want) {
                Ok((pblk, len)) => {
                    let bytes = len as usize * BS;
                    if let Err(e) = self.st.write_at(pblk * BS as u64, &buf[done..done + bytes]) {
                        result = Err(e);
                        break;
                    }
                    let ino = of.ino;
                    let lblk = of.next_lblk;
                    let n = self.dir_mut(ino);
                    match n.extents.last_mut() {
                        Some(e) if e.pblk + e.len as u64 == pblk && e.lblk + e.len == lblk && e.len as u64 + len <= MAX_EXTENT_LEN => {
                            e.len += len as u32
                        }
                        _ => n.extents.push(Extent { lblk, len: len as u32, pblk }),
                    }
                    of.next_lblk += len as u32;
                    done += bytes;
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        buf.drain(..done);
        of.buf = buf;
        self.open = Some(of);
        result
    }

    /// Finishes the open file and returns its inode number.
    pub fn end_file(&mut self) -> Result<u32> {
        if self.open.is_none() {
            return Err(Error::Invalid("no open file"));
        }
        self.flush_open(true)?;
        let of = self.open.take().unwrap();
        self.dir_mut(of.ino).size = of.size;
        Ok(of.ino)
    }

    pub fn symlink(&mut self, path: &str, target: &str, meta: &Meta) -> Result<u32> {
        let (parent, name) = self.parent_of(path)?;
        self.replace_slot(parent, name)?;
        let ino = self.alloc_ino()?;
        let mut n = Inode::new(FileKind::Symlink, meta);
        n.mode = S_IFLNK | 0o777;
        n.size = target.len() as u64;
        if target.len() < 60 {
            n.fast_symlink = target.as_bytes().to_vec();
        } else if target.len() < BS {
            let b = self.alloc_one()?;
            let mut blk = vec![0u8; BS];
            blk[..target.len()].copy_from_slice(target.as_bytes());
            self.st.write_at(b * BS as u64, &blk)?;
            n.extents.push(Extent { lblk: 0, len: 1, pblk: b });
        } else {
            return Err(Error::Invalid("symlink target too long"));
        }
        self.inodes[ino as usize] = Some(n);
        self.link_new(parent, name, FileKind::Symlink, ino);
        Ok(ino)
    }

    pub fn mknod(&mut self, path: &str, kind: FileKind, major: u32, minor: u32, meta: &Meta) -> Result<u32> {
        if !matches!(kind, FileKind::CharDev | FileKind::BlockDev | FileKind::Fifo) {
            return Err(Error::Invalid("not a special file kind"));
        }
        let (parent, name) = self.parent_of(path)?;
        self.replace_slot(parent, name)?;
        let ino = self.alloc_ino()?;
        let mut n = Inode::new(kind, meta);
        n.rdev = (major, minor);
        self.inodes[ino as usize] = Some(n);
        self.link_new(parent, name, kind, ino);
        Ok(ino)
    }

    /// Adds `path` as another name for the existing non-directory `existing`.
    pub fn hardlink(&mut self, path: &str, existing: &str) -> Result<u32> {
        let ino = self.lookup(existing).ok_or(Error::NotFound)?;
        let kind = self.dir(ino).kind;
        if kind == FileKind::Dir {
            return Err(Error::IsDir);
        }
        let (parent, name) = self.parent_of(path)?;
        self.replace_slot(parent, name)?;
        self.link_new(parent, name, kind, ino);
        self.dir_mut(ino).nlink += 1;
        Ok(ino)
    }

    pub fn lookup(&self, path: &str) -> Option<u32> {
        let mut cur = ROOT_INO;
        for c in components(path).ok()? {
            cur = self.lookup_in(cur, c)?.0;
        }
        Some(cur)
    }

    pub fn file_size(&self, ino: u32) -> u64 {
        self.dir(ino).size
    }

    /// Reads back a completed regular file.
    pub fn read_file(&mut self, ino: u32, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let n = self.dir(ino);
        if n.kind != FileKind::Regular {
            return Err(Error::Invalid("not a regular file"));
        }
        let size = n.size;
        if offset >= size {
            return Ok(0);
        }
        let len = (buf.len() as u64).min(size - offset) as usize;
        let extents = n.extents.clone();
        let mut done = 0;
        while done < len {
            let pos = offset + done as u64;
            let lblk = (pos / BS as u64) as u32;
            let e = extents.iter().find(|e| lblk >= e.lblk && lblk < e.lblk + e.len).ok_or(Error::Io)?;
            let in_ext = (lblk - e.lblk) as u64 * BS as u64 + pos % BS as u64;
            let avail = e.len as u64 * BS as u64 - in_ext;
            let take = (len - done).min(avail as usize);
            self.st.read_at(e.pblk * BS as u64 + in_ext, &mut buf[done..done + take])?;
            done += take;
        }
        Ok(len)
    }

    // ---- finishing ----

    fn ext_csum(&self, ino: u32) -> u32 {
        // Inodes are created with generation 0.
        crc32c(crc32c(self.csum_seed, &ino.to_le_bytes()), &0u32.to_le_bytes())
    }

    fn build_dir_blocks(&self, ino: u32) -> Vec<u8> {
        let d = self.dir(ino);
        let cap = BS - DIR_TAIL;
        let mut entries: Vec<(u32, u8, Vec<u8>)> = Vec::new();
        entries.push((ino, 2, b".".to_vec()));
        entries.push((d.parent, 2, b"..".to_vec()));
        for (name, (i, k)) in &d.children {
            entries.push((*i, k.dirent_type(), name.clone()));
        }
        let mut blocks: Vec<Vec<(u32, u8, Vec<u8>)>> = alloc::vec![Vec::new()];
        let mut used = 0;
        for e in entries {
            let sz = (8 + e.2.len()).div_ceil(4) * 4;
            if used + sz > cap {
                blocks.push(Vec::new());
                used = 0;
            }
            used += sz;
            blocks.last_mut().unwrap().push(e);
        }
        while blocks.len() < d.min_dir_blocks {
            blocks.push(Vec::new());
        }
        let mut out = vec![0u8; blocks.len() * BS];
        for (bi, ents) in blocks.iter().enumerate() {
            let b = &mut out[bi * BS..(bi + 1) * BS];
            let mut off = 0;
            if ents.is_empty() {
                put32(b, 0, 0);
                put16(b, 4, cap as u16);
            }
            for (i, (child, ft, name)) in ents.iter().enumerate() {
                let sz = (8 + name.len()).div_ceil(4) * 4;
                let rec_len = if i + 1 == ents.len() { cap - off } else { sz };
                put32(b, off, *child);
                put16(b, off + 4, rec_len as u16);
                b[off + 6] = name.len() as u8;
                b[off + 7] = *ft;
                b[off + 8..off + 8 + name.len()].copy_from_slice(name);
                off += rec_len;
            }
            // Checksum tail.
            put32(b, cap, 0);
            put16(b, cap + 4, DIR_TAIL as u16);
            b[cap + 6] = 0;
            b[cap + 7] = 0xde;
            let csum = crc32c(self.ext_csum(ino), &b[..cap]);
            put32(b, cap + 8, csum);
        }
        out
    }

    fn xattr_prefix(name: &str) -> (u8, &str) {
        match name {
            "system.posix_acl_access" => (2, ""),
            "system.posix_acl_default" => (3, ""),
            _ => {
                for (p, i) in [("user.", 1u8), ("trusted.", 4), ("security.", 6), ("system.", 7)] {
                    if let Some(rest) = name.strip_prefix(p) {
                        return (i, rest);
                    }
                }
                (0, name)
            }
        }
    }

    fn build_xattr_block(&self, blk: u64, attrs: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
        let mut items: Vec<(u8, &str, &[u8])> = attrs
            .iter()
            .map(|(n, v)| {
                let (idx, rest) = Self::xattr_prefix(n);
                (idx, rest, v.as_slice())
            })
            .collect();
        items.sort_by(|a, b| (a.0, a.1.len(), a.1).cmp(&(b.0, b.1.len(), b.1)));
        let mut b = vec![0u8; BS];
        put32(&mut b, 0, 0xEA02_0000);
        put32(&mut b, 4, 1); // refcount
        put32(&mut b, 8, 1); // blocks
        let mut entry_off = 32;
        let mut value_end = BS;
        let mut block_hash = 0u32;
        for (idx, name, value) in &items {
            let padded = value.len().div_ceil(4) * 4;
            let ent_size = (16 + name.len()).div_ceil(4) * 4;
            if entry_off + ent_size + 4 > value_end.saturating_sub(padded) {
                return Err(Error::Invalid("xattrs do not fit one block"));
            }
            value_end -= padded;
            b[value_end..value_end + value.len()].copy_from_slice(value);
            let mut h = 0u32;
            for c in name.bytes() {
                h = (h << 5) ^ (h >> 27) ^ c as u32;
            }
            for w in b[value_end..value_end + padded].chunks(4) {
                h = (h << 16) ^ (h >> 16) ^ u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            }
            b[entry_off] = name.len() as u8;
            b[entry_off + 1] = *idx;
            put16(&mut b, entry_off + 2, value_end as u16);
            put32(&mut b, entry_off + 4, 0);
            put32(&mut b, entry_off + 8, value.len() as u32);
            put32(&mut b, entry_off + 12, h);
            b[entry_off + 16..entry_off + 16 + name.len()].copy_from_slice(name.as_bytes());
            entry_off += ent_size;
            block_hash = (block_hash << 16) ^ (block_hash >> 16) ^ h;
        }
        put32(&mut b, 12, block_hash);
        let csum = crc32c(crc32c(self.csum_seed, &blk.to_le_bytes()), &b);
        put32(&mut b, 16, csum);
        Ok(b)
    }

    /// Fills `i_block` with the extent tree for `ino`, allocating and writing leaf blocks.
    fn build_extent_root(&mut self, ino: u32) -> Result<()> {
        let extents = self.dir(ino).extents.clone();
        let mut root = [0u8; 60];
        put16(&mut root, 0, 0xF30A);
        put16(&mut root, 4, 4); // eh_max
        if extents.len() <= 4 {
            put16(&mut root, 2, extents.len() as u16);
            for (i, e) in extents.iter().enumerate() {
                write_extent(&mut root[12 + 12 * i..], e);
            }
        } else {
            const PER_LEAF: usize = 340;
            let leaves = extents.len().div_ceil(PER_LEAF);
            if leaves > 4 {
                return Err(Error::Invalid("file too fragmented"));
            }
            put16(&mut root, 2, leaves as u16);
            put16(&mut root, 6, 1); // depth
            for (li, chunk) in extents.chunks(PER_LEAF).enumerate() {
                let blk = self.alloc_one()?;
                let mut leaf = vec![0u8; BS];
                put16(&mut leaf, 0, 0xF30A);
                put16(&mut leaf, 2, chunk.len() as u16);
                put16(&mut leaf, 4, PER_LEAF as u16);
                for (i, e) in chunk.iter().enumerate() {
                    write_extent(&mut leaf[12 + 12 * i..], e);
                }
                let tail = 12 + 12 * PER_LEAF;
                let csum = crc32c(self.ext_csum(ino), &leaf[..tail]);
                put32(&mut leaf, tail, csum);
                self.st.write_at(blk * BS as u64, &leaf)?;
                let o = 12 + 12 * li;
                put32(&mut root, o, chunk[0].lblk);
                put32(&mut root, o + 4, blk as u32);
                put16(&mut root, o + 8, (blk >> 32) as u16);
                self.dir_mut(ino).meta_blocks += 1;
            }
        }
        self.dir_mut(ino).i_block = root;
        Ok(())
    }

    fn inode_bytes(&self, ino: u32, n: &Inode) -> [u8; ISIZE] {
        let mut b = [0u8; ISIZE];
        put16(&mut b, 0x00, n.mode);
        put16(&mut b, 0x02, n.uid as u16);
        put32(&mut b, 0x04, n.size as u32);
        let t = n.mtime.min((1u64 << 34) - 1);
        put32(&mut b, 0x08, t as u32);
        put32(&mut b, 0x0c, t as u32);
        put32(&mut b, 0x10, t as u32);
        put16(&mut b, 0x18, n.gid as u16);
        let links = if n.kind == FileKind::Dir && n.nlink >= 65000 { 1 } else { n.nlink.min(65000) };
        put16(&mut b, 0x1a, links as u16);
        let data_blocks: u64 = n.extents.iter().map(|e| e.len as u64).sum();
        let sectors = (data_blocks + n.meta_blocks + (n.xattr_block != 0) as u64) * (BS as u64 / 512);
        put32(&mut b, 0x1c, sectors as u32);
        let uses_extents = matches!(n.kind, FileKind::Regular | FileKind::Dir) || (n.kind == FileKind::Symlink && n.fast_symlink.is_empty());
        put32(&mut b, 0x20, if uses_extents { FL_EXTENTS } else { 0 });
        b[0x28..0x28 + 60].copy_from_slice(&n.i_block);
        put32(&mut b, 0x68, n.xattr_block as u32);
        put32(&mut b, 0x6c, (n.size >> 32) as u32);
        put16(&mut b, 0x74, (sectors >> 32) as u16);
        put16(&mut b, 0x76, (n.xattr_block >> 32) as u16);
        put16(&mut b, 0x78, (n.uid >> 16) as u16);
        put16(&mut b, 0x7a, (n.gid >> 16) as u16);
        put16(&mut b, 0x80, EXTRA_ISIZE);
        let epoch = ((t >> 32) & 3) as u32;
        put32(&mut b, 0x84, epoch);
        put32(&mut b, 0x88, epoch);
        put32(&mut b, 0x8c, epoch);
        put32(&mut b, 0x90, self.opts.now as u32);
        put32(&mut b, 0x94, ((self.opts.now >> 32) & 3) as u32);
        let csum = crc32c(self.ext_csum(ino), &b);
        put16(&mut b, 0x7c, csum as u16);
        put16(&mut b, 0x82, (csum >> 16) as u16);
        b
    }

    fn superblock(&self, group: u32, free_blocks: u64, free_inodes: u64) -> Vec<u8> {
        let mut sb = vec![0u8; 1024];
        put32(&mut sb, 0x00, self.ipg * self.groups);
        put32(&mut sb, 0x04, self.blocks as u32);
        put32(&mut sb, 0x08, (self.blocks * self.opts.reserved_percent / 100) as u32);
        put32(&mut sb, 0x0c, free_blocks as u32);
        put32(&mut sb, 0x10, free_inodes as u32);
        put32(&mut sb, 0x14, 0); // first data block
        put32(&mut sb, 0x18, 2); // log block size (4096)
        put32(&mut sb, 0x1c, 2);
        put32(&mut sb, 0x20, BPG as u32);
        put32(&mut sb, 0x24, BPG as u32);
        put32(&mut sb, 0x28, self.ipg);
        put32(&mut sb, 0x30, self.opts.now as u32); // wtime
        put16(&mut sb, 0x36, 0xffff); // max mount count -1
        put16(&mut sb, 0x38, 0xEF53);
        put16(&mut sb, 0x3a, 1); // clean
        put16(&mut sb, 0x3c, 1); // errors: continue
        put32(&mut sb, 0x40, self.opts.now as u32); // lastcheck
        put32(&mut sb, 0x4c, 1); // dynamic rev
        put32(&mut sb, 0x54, FIRST_INO);
        put16(&mut sb, 0x58, ISIZE as u16);
        put16(&mut sb, 0x5a, group as u16);
        let journal = self.inodes.get(JOURNAL_INO as usize).and_then(|n| n.as_ref());
        put32(&mut sb, 0x5c, 0x8 | 0x20 | if journal.is_some() { 0x4 } else { 0 }); // ext_attr, dir_index, has_journal
        put32(&mut sb, 0x60, 0x2 | 0x40); // filetype, extents
        put32(&mut sb, 0x64, 0x1 | 0x2 | 0x20 | 0x40 | 0x400); // sparse_super, large_file, dir_nlink, extra_isize, metadata_csum
        sb[0x68..0x78].copy_from_slice(&self.opts.uuid);
        let label = self.opts.label.as_bytes();
        let n = label.len().min(16);
        sb[0x78..0x78 + n].copy_from_slice(&label[..n]);
        for (i, s) in self.opts.hash_seed.iter().enumerate() {
            put32(&mut sb, 0xec + 4 * i, *s);
        }
        sb[0xfc] = 1; // half-MD4 directory hash
        put32(&mut sb, 0x100, 0x0c); // default mount opts: user_xattr, acl
        if let Some(j) = journal {
            put32(&mut sb, 0xe0, JOURNAL_INO); // s_journal_inum (journal_uuid and journal_dev stay 0: internal)
            // s_jnl_blocks: backup of the journal inode's block map, then its size (high, low).
            sb[0x10c..0x10c + 60].copy_from_slice(&j.i_block);
            put32(&mut sb, 0x10c + 60, (j.size >> 32) as u32);
            put32(&mut sb, 0x10c + 64, j.size as u32);
            sb[0xfd] = 1; // s_jnl_backup_type: block map backed up in s_jnl_blocks
        }
        put32(&mut sb, 0x108, self.opts.now as u32); // mkfs time
        put16(&mut sb, 0x15c, EXTRA_ISIZE);
        put16(&mut sb, 0x15e, EXTRA_ISIZE);
        put32(&mut sb, 0x160, 1); // signed dir hash
        sb[0x175] = 1; // crc32c
        let csum = crc32c(!0, &sb[..0x3fc]);
        put32(&mut sb, 0x3fc, csum);
        sb
    }

    /// Allocates and writes all remaining metadata and flushes the filesystem structures.
    pub fn finish(&mut self) -> Result<()> {
        if self.open.is_some() {
            return Err(Error::Invalid("a file is still open"));
        }
        let max_ino = self.next_ino - 1;

        // Directory contents, xattr blocks and extent trees.
        for ino in ROOT_INO..=max_ino {
            if ino > ROOT_INO && ino < FIRST_INO {
                continue;
            }
            if self.inodes[ino as usize].is_none() {
                continue;
            }
            if self.dir(ino).kind == FileKind::Dir {
                let data = self.build_dir_blocks(ino);
                let nblocks = (data.len() / BS) as u64;
                let mut done = 0u64;
                let mut lblk = 0u32;
                while done < nblocks {
                    let (pblk, len) = self.alloc_run(nblocks - done)?;
                    self.st.write_at(pblk * BS as u64, &data[done as usize * BS..(done + len) as usize * BS])?;
                    self.dir_mut(ino).extents.push(Extent { lblk, len: len as u32, pblk });
                    lblk += len as u32;
                    done += len;
                }
                self.dir_mut(ino).size = data.len() as u64;
            }
            if !self.dir(ino).xattrs.is_empty() {
                let blk = self.alloc_one()?;
                let block = self.build_xattr_block(blk, &self.dir(ino).xattrs.clone())?;
                self.st.write_at(blk * BS as u64, &block)?;
                self.dir_mut(ino).xattr_block = blk;
            }
            match self.dir(ino).kind {
                FileKind::Regular | FileKind::Dir => self.build_extent_root(ino)?,
                FileKind::Symlink => {
                    if self.dir(ino).fast_symlink.is_empty() {
                        self.build_extent_root(ino)?;
                    } else {
                        let t = self.dir(ino).fast_symlink.clone();
                        self.dir_mut(ino).i_block[..t.len()].copy_from_slice(&t);
                    }
                }
                FileKind::CharDev | FileKind::BlockDev => {
                    let (maj, min) = self.dir(ino).rdev;
                    let ib = &mut self.dir_mut(ino).i_block;
                    if maj < 256 && min < 256 {
                        put32(ib, 0, (maj << 8) | min);
                    } else {
                        put32(ib, 4, (min & 0xff) | (maj << 8) | ((min & !0xff) << 12));
                    }
                }
                FileKind::Fifo => {}
            }
        }

        // Per-group accounting.
        let ng = self.groups as usize;
        let mut used_inodes = vec![0u32; ng];
        let mut highest = vec![0u32; ng]; // highest used index (1-based) in each table
        let mut dirs = vec![0u32; ng];
        for ino in 1..=max_ino {
            let reserved = ino < FIRST_INO && ino != ROOT_INO;
            if !reserved && self.inodes[ino as usize].is_none() {
                continue;
            }
            let g = self.group_of_ino(ino) as usize;
            used_inodes[g] += 1;
            highest[g] = highest[g].max((ino - 1) % self.ipg + 1);
            if !reserved && self.dir(ino).kind == FileKind::Dir {
                dirs[g] += 1;
            }
        }

        let mut gdt = vec![0u8; self.gdt_blocks as usize * BS];
        let mut total_free_blocks = 0u64;
        let mut total_free_inodes = 0u64;
        for g in 0..self.groups {
            let l = self.layout(g);
            let gi = g as usize;

            // Block bitmap.
            let bb = self.bitmaps[gi].clone();
            self.st.write_at(l.block_bitmap * BS as u64, &bb)?;
            let bb_csum = crc32c(self.csum_seed, &bb[..(BPG / 8) as usize]);

            // Inode bitmap.
            let mut ib = vec![0xffu8; BS];
            for b in ib.iter_mut().take(self.ipg as usize / 8) {
                *b = 0;
            }
            for ino in 1..=max_ino {
                if self.group_of_ino(ino) != g {
                    continue;
                }
                let reserved = ino < FIRST_INO && ino != ROOT_INO;
                if reserved || self.inodes[ino as usize].is_some() {
                    let bit = ((ino - 1) % self.ipg) as usize;
                    ib[bit / 8] |= 1 << (bit % 8);
                }
            }
            // Reserved inodes 1..10 live in group 0.
            self.st.write_at(l.inode_bitmap * BS as u64, &ib)?;
            let ib_csum = crc32c(self.csum_seed, &ib[..self.ipg as usize / 8]);

            // Inode table (skipped for groups without inodes; INODE_UNINIT covers them).
            let uninit = used_inodes[gi] == 0;
            if !uninit {
                let mut table = vec![0u8; self.itable_blocks as usize * BS];
                let first = g * self.ipg + 1;
                for ino in first..first + highest[gi].min(self.ipg) {
                    if ino > max_ino {
                        break;
                    }
                    if let Some(n) = self.inodes[ino as usize].as_ref() {
                        let off = ((ino - 1) % self.ipg) as usize * ISIZE;
                        table[off..off + ISIZE].copy_from_slice(&self.inode_bytes(ino, n));
                    }
                }
                let used_table = highest[gi] as usize * ISIZE;
                let bytes = used_table.div_ceil(BS) * BS;
                self.st.write_at(l.inode_table * BS as u64, &table[..bytes])?;
            }

            let free_blocks = self.free_blocks[gi];
            let free_inodes = self.ipg - used_inodes[gi];
            total_free_blocks += free_blocks;
            total_free_inodes += free_inodes as u64;

            let d = &mut gdt[gi * 32..(gi + 1) * 32];
            put32(d, 0x00, l.block_bitmap as u32);
            put32(d, 0x04, l.inode_bitmap as u32);
            put32(d, 0x08, l.inode_table as u32);
            put16(d, 0x0c, free_blocks as u16);
            put16(d, 0x0e, free_inodes as u16);
            put16(d, 0x10, dirs[gi] as u16);
            put16(d, 0x12, if uninit { 1 } else { 4 }); // INODE_UNINIT / ITABLE_ZEROED
            put16(d, 0x18, bb_csum as u16);
            put16(d, 0x1a, ib_csum as u16);
            put16(d, 0x1c, (self.ipg - highest[gi]) as u16);
            let mut c = crc32c(self.csum_seed, &g.to_le_bytes());
            c = crc32c(c, &d[..0x1e]);
            c = crc32c(c, &[0, 0]); // the checksum field itself, as zeros
            put16(d, 0x1e, c as u16);
        }

        // Superblock and group descriptor copies.
        for g in 0..self.groups {
            if !has_backup(g) {
                continue;
            }
            let l = self.layout(g);
            let sb = self.superblock(g, total_free_blocks, total_free_inodes);
            if g == 0 {
                let mut blk0 = vec![0u8; BS];
                blk0[1024..2048].copy_from_slice(&sb);
                self.st.write_at(0, &blk0)?;
            } else {
                let mut blk = vec![0u8; BS];
                blk[..1024].copy_from_slice(&sb);
                self.st.write_at(l.start * BS as u64, &blk)?;
            }
            self.st.write_at((l.start + 1) * BS as u64, &gdt)?;
        }
        Ok(())
    }
}

fn write_extent(b: &mut [u8], e: &Extent) {
    put32(b, 0, e.lblk);
    put16(b, 4, e.len as u16);
    put16(b, 6, (e.pblk >> 32) as u16);
    put32(b, 8, e.pblk as u32);
}
