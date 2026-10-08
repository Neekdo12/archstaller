//! Transparent gzip / zstd decompression chosen by magic bytes.
use crate::io::Read;
use crate::{Error, Result};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

/// Re-serves already-read magic bytes before the rest of the source.
pub struct Peeked<R> {
    head: [u8; 4],
    pos: usize,
    len: usize,
    inner: R,
}

impl<R: Read> Read for Peeked<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.pos < self.len {
            let n = (self.len - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.head[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.inner.read(buf)
    }
}

/// The largest zstd window accepted. ruzstd's default is 100 MiB, but Arch compresses packages with
/// `zstd --ultra -20`, which uses a 128 MiB window (`js140` is one); anything larger is refused so a
/// hostile stream cannot make the installer allocate without bound.
const ZSTD_MAX_WINDOW: u64 = 128 * 1024 * 1024;

/// Adapts our reader to ruzstd's.
pub struct ZAdapter<R>(R);

impl<R: Read> ruzstd::io::Read for ZAdapter<R> {
    fn read(&mut self, buf: &mut [u8]) -> core::result::Result<usize, ruzstd::io::Error> {
        self.0.read(buf).map_err(|_| ruzstd::io::Error::from(ruzstd::io::ErrorKind::Other))
    }
}

pub struct Gzip<R> {
    src: R,
    inbuf: Vec<u8>,
    in_pos: usize,
    in_len: usize,
    state: Box<InflateState>,
    done: bool,
}

impl<R: Read> Gzip<R> {
    fn new(mut src: R) -> Result<Self> {
        // Header: 10 fixed bytes, then optional fields.
        let mut h = [0u8; 10];
        src.read_exact(&mut h)?;
        if h[..3] != [0x1f, 0x8b, 8] {
            return Err(Error::Format("bad gzip header"));
        }
        let flg = h[3];
        let mut byte = [0u8; 1];
        if flg & 4 != 0 {
            let mut l = [0u8; 2];
            src.read_exact(&mut l)?;
            let mut skip = vec![0u8; u16::from_le_bytes(l) as usize];
            src.read_exact(&mut skip)?;
        }
        for bit in [8u8, 16] {
            if flg & bit != 0 {
                loop {
                    src.read_exact(&mut byte)?;
                    if byte[0] == 0 {
                        break;
                    }
                }
            }
        }
        if flg & 2 != 0 {
            let mut crc = [0u8; 2];
            src.read_exact(&mut crc)?;
        }
        Ok(Gzip {
            src,
            inbuf: vec![0; 16 * 1024],
            in_pos: 0,
            in_len: 0,
            state: InflateState::new_boxed(DataFormat::Raw),
            done: false,
        })
    }
}

impl<R: Read> Read for Gzip<R> {
    fn read(&mut self, out: &mut [u8]) -> Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if self.done {
                return Ok(0);
            }
            if self.in_pos == self.in_len {
                self.in_len = self.src.read(&mut self.inbuf)?;
                self.in_pos = 0;
            }
            let eof = self.in_len == 0;
            let flush = if eof { MZFlush::Finish } else { MZFlush::None };
            let r = inflate(&mut self.state, &self.inbuf[self.in_pos..self.in_len], out, flush);
            self.in_pos += r.bytes_consumed;
            match r.status {
                Ok(MZStatus::StreamEnd) => self.done = true,
                Ok(_) => {}
                Err(MZError::Buf) => {
                    if eof && r.bytes_written == 0 {
                        return Err(Error::Eof);
                    }
                }
                Err(_) => return Err(Error::Format("corrupt deflate stream")),
            }
            if r.bytes_written > 0 {
                return Ok(r.bytes_written);
            }
        }
    }
}

type ZstdReader<R> = ruzstd::decoding::StreamingDecoder<ZAdapter<Peeked<R>>, ruzstd::decoding::FrameDecoder>;

pub enum Decompressor<R: Read> {
    Plain(Peeked<R>),
    Gzip(Gzip<Peeked<R>>),
    Zstd(Box<ZstdReader<R>>),
}

/// Sniffs the stream and returns a decompressing reader (or passthrough if uncompressed).
pub fn open<R: Read>(mut src: R) -> Result<Decompressor<R>> {
    let mut head = [0u8; 4];
    let mut len = 0;
    while len < 4 {
        let n = src.read(&mut head[len..])?;
        if n == 0 {
            break;
        }
        len += n;
    }
    let peeked = Peeked { head, pos: 0, len, inner: src };
    if head[..2] == [0x1f, 0x8b] {
        Ok(Decompressor::Gzip(Gzip::new(peeked)?))
    } else if head == [0x28, 0xb5, 0x2f, 0xfd] {
        let d = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(ZAdapter(peeked), ZSTD_MAX_WINDOW).map_err(|_| Error::Format("bad zstd frame"))?;
        Ok(Decompressor::Zstd(Box::new(d)))
    } else if head == [0xfd, b'7', b'z', b'X'] {
        Err(Error::Unsupported("xz"))
    } else {
        Ok(Decompressor::Plain(peeked))
    }
}

impl<R: Read> Read for Decompressor<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self {
            Decompressor::Plain(r) => r.read(buf),
            Decompressor::Gzip(r) => r.read(buf),
            Decompressor::Zstd(r) => ruzstd::io::Read::read(&mut **r, buf).map_err(|_| Error::Format("corrupt zstd stream")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zstd frame with one raw block holding "hello" and the given window descriptor byte.
    fn frame(window_descriptor: u8) -> Vec<u8> {
        let mut f = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, window_descriptor];
        f.extend_from_slice(&[0x29, 0x00, 0x00]); // last block, raw, 5 bytes
        f.extend_from_slice(b"hello");
        f
    }

    fn decode(data: &[u8]) -> Result<Vec<u8>> {
        let mut d = open(data)?;
        let mut out = Vec::new();
        let mut buf = [0u8; 64];
        loop {
            let n = d.read(&mut buf)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    #[test]
    fn arch_packages_use_a_128_mib_window_and_it_is_accepted() {
        // Window_Descriptor 0x88: exponent 17, mantissa 0 -> 2^(10+17) = 128 MiB (`zstd --ultra -20`).
        assert_eq!(decode(&frame(0x88)).unwrap(), b"hello");
        // 64 MiB and the 8 MiB default of `zstd -19` are fine too.
        assert_eq!(decode(&frame(0x80)).unwrap(), b"hello");
        assert_eq!(decode(&frame(0x48)).unwrap(), b"hello");
    }

    #[test]
    fn a_window_larger_than_128_mib_is_refused() {
        // exponent 18 -> 256 MiB
        assert!(matches!(decode(&frame(0x90)), Err(Error::Format("bad zstd frame"))));
    }
}
