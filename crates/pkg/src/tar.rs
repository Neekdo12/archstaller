//! Streaming tar reader: ustar, GNU long names, pax extended headers (incl. SCHILY.xattr.*).
use crate::io::Read;
use crate::{Error, Result};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Hardlink,
    CharDev,
    BlockDev,
    Fifo,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub mtime: u64,
    pub link: String,
    pub dev_major: u32,
    pub dev_minor: u32,
    pub xattrs: Vec<(String, Vec<u8>)>,
}

pub struct TarReader<R: Read> {
    src: R,
    /// Bytes of the current entry's data not yet consumed, and padding after it.
    remaining: u64,
    padding: u64,
    done: bool,
}

fn field(b: &[u8]) -> &[u8] {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    &b[..end]
}

fn string(b: &[u8]) -> String {
    String::from_utf8_lossy(field(b)).into_owned()
}

fn octal(b: &[u8]) -> Result<u64> {
    if b.first().is_some_and(|c| c & 0x80 != 0) {
        // GNU base-256.
        let mut v = (b[0] & 0x7f) as u64;
        for &c in &b[1..] {
            v = v.checked_mul(256).ok_or(Error::Format("tar number overflow"))? | c as u64;
        }
        return Ok(v);
    }
    let mut v = 0u64;
    for &c in b {
        match c {
            b'0'..=b'7' => v = v.checked_mul(8).ok_or(Error::Format("tar number overflow"))? + (c - b'0') as u64,
            0 | b' ' => {
                if v != 0 || c == 0 {
                    // Terminator once digits were seen (or an empty field).
                    if c == 0 {
                        break;
                    }
                }
            }
            _ => return Err(Error::Format("bad tar number")),
        }
    }
    Ok(v)
}

impl<R: Read> TarReader<R> {
    pub fn new(src: R) -> Self {
        TarReader { src, remaining: 0, padding: 0, done: false }
    }

    fn skip(&mut self, mut n: u64) -> Result<()> {
        let mut sink = [0u8; 4096];
        while n > 0 {
            let want = n.min(sink.len() as u64) as usize;
            let got = self.src.read(&mut sink[..want])?;
            if got == 0 {
                return Err(Error::Eof);
            }
            n -= got as u64;
        }
        Ok(())
    }

    fn read_block(&mut self) -> Result<Option<[u8; 512]>> {
        let mut b = [0u8; 512];
        let mut got = 0;
        while got < 512 {
            let n = self.src.read(&mut b[got..])?;
            if n == 0 {
                return if got == 0 { Ok(None) } else { Err(Error::Eof) };
            }
            got += n;
        }
        Ok(Some(b))
    }

    fn read_blob(&mut self, size: u64) -> Result<Vec<u8>> {
        if size > 1 << 20 {
            return Err(Error::Format("tar metadata record too large"));
        }
        let mut v = vec![0u8; size as usize];
        self.src.read_exact(&mut v)?;
        self.skip((512 - size % 512) % 512)?;
        Ok(v)
    }

    /// Advances to the next entry, discarding any unread data of the current one.
    pub fn next_entry(&mut self) -> Result<Option<Entry>> {
        if self.done {
            return Ok(None);
        }
        self.skip(self.remaining + self.padding)?;
        self.remaining = 0;
        self.padding = 0;

        let mut long_path: Option<String> = None;
        let mut long_link: Option<String> = None;
        let mut pax: Vec<(String, Vec<u8>)> = Vec::new();
        loop {
            let Some(h) = self.read_block()? else {
                self.done = true;
                return Ok(None);
            };
            if h.iter().all(|&b| b == 0) {
                self.done = true;
                return Ok(None);
            }
            // Checksum over the header with the checksum field as spaces.
            let stored = octal(&h[148..156])?;
            let sum: u64 = h.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { b as u64 }).sum();
            if sum != stored {
                return Err(Error::Format("tar checksum mismatch"));
            }
            let size = octal(&h[124..136])?;
            let typeflag = h[156];
            match typeflag {
                b'L' => {
                    let b = self.read_blob(size)?;
                    long_path = Some(string(&b));
                    continue;
                }
                b'K' => {
                    let b = self.read_blob(size)?;
                    long_link = Some(string(&b));
                    continue;
                }
                b'x' => {
                    let b = self.read_blob(size)?;
                    parse_pax(&b, &mut pax)?;
                    continue;
                }
                b'g' => {
                    self.read_blob(size)?;
                    continue;
                }
                _ => {}
            }

            let mut path = if &h[257..262] == b"ustar" && !field(&h[345..500]).is_empty() {
                let mut p = string(&h[345..500]);
                p.push('/');
                p.push_str(&string(&h[..100]));
                p
            } else {
                string(&h[..100])
            };
            let mut e = Entry {
                path: String::new(),
                kind: match typeflag {
                    b'0' | 0 | b'7' => Kind::File,
                    b'1' => Kind::Hardlink,
                    b'2' => Kind::Symlink,
                    b'3' => Kind::CharDev,
                    b'4' => Kind::BlockDev,
                    b'5' => Kind::Dir,
                    b'6' => Kind::Fifo,
                    _ => return Err(Error::Format("unsupported tar entry type")),
                },
                mode: octal(&h[100..108])? as u32,
                uid: octal(&h[108..116])? as u32,
                gid: octal(&h[116..124])? as u32,
                size,
                mtime: octal(&h[136..148])?,
                link: string(&h[157..257]),
                dev_major: octal(&h[329..337])? as u32,
                dev_minor: octal(&h[337..345])? as u32,
                xattrs: Vec::new(),
            };
            if let Some(p) = long_path {
                path = p;
            }
            if let Some(l) = long_link {
                e.link = l;
            }
            for (k, v) in pax {
                let text = || String::from_utf8_lossy(&v).into_owned();
                match k.as_str() {
                    "path" => path = text(),
                    "linkpath" => e.link = text(),
                    "size" => e.size = text().parse().map_err(|_| Error::Format("bad pax size"))?,
                    "uid" => e.uid = text().parse().map_err(|_| Error::Format("bad pax uid"))?,
                    "gid" => e.gid = text().parse().map_err(|_| Error::Format("bad pax gid"))?,
                    "mtime" => e.mtime = text().split('.').next().unwrap_or("0").parse().unwrap_or(0),
                    _ => {
                        if let Some(name) = k.strip_prefix("SCHILY.xattr.") {
                            e.xattrs.push((String::from(name), v));
                        }
                    }
                }
            }
            e.path = path;
            if matches!(e.kind, Kind::File) {
                self.remaining = e.size;
                self.padding = (512 - e.size % 512) % 512;
            } else {
                self.remaining = 0;
                self.padding = 0;
            }
            return Ok(Some(e));
        }
    }

    /// Reads data of the current entry; returns 0 at its end.
    pub fn read_data(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.remaining) as usize;
        let n = self.src.read(&mut buf[..want])?;
        if n == 0 {
            return Err(Error::Eof);
        }
        self.remaining -= n as u64;
        Ok(n)
    }

    pub fn read_all(&mut self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.remaining as usize);
        let mut buf = [0u8; 4096];
        loop {
            let n = self.read_data(&mut buf)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
        }
    }
}

/// Parses `"<len> <key>=<value>\n"` records.
fn parse_pax(mut b: &[u8], out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    while !b.is_empty() {
        let sp = b.iter().position(|&c| c == b' ').ok_or(Error::Format("bad pax record"))?;
        let len: usize = core::str::from_utf8(&b[..sp]).ok().and_then(|s| s.parse().ok()).ok_or(Error::Format("bad pax length"))?;
        if len < sp + 2 || len > b.len() {
            return Err(Error::Format("bad pax length"));
        }
        let rec = &b[sp + 1..len - 1]; // strip trailing newline
        let eq = rec.iter().position(|&c| c == b'=').ok_or(Error::Format("bad pax record"))?;
        out.push((String::from_utf8_lossy(&rec[..eq]).into_owned(), rec[eq + 1..].to_vec()));
        b = &b[len..];
    }
    Ok(())
}
