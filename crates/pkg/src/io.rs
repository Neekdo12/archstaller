use crate::{Error, Result};

pub trait Read {
    /// Reads up to `buf.len()` bytes; returns 0 at end of input.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

    fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<()> {
        while !buf.is_empty() {
            let n = self.read(buf)?;
            if n == 0 {
                return Err(Error::Eof);
            }
            buf = &mut buf[n..];
        }
        Ok(())
    }
}

impl Read for &[u8] {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = self.len().min(buf.len());
        buf[..n].copy_from_slice(&self[..n]);
        *self = &self[n..];
        Ok(n)
    }
}

impl<R: Read + ?Sized> Read for &mut R {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        (**self).read(buf)
    }
}
