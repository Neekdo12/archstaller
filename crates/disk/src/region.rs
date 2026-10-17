//! Byte-granular access to a range of a block device.
use crate::{Error, Result};
use alloc::vec;
use hal::BlockDevice;

pub struct Region<'a> {
    dev: &'a mut dyn BlockDevice,
    /// Byte offset of the region on the device.
    base: u64,
    len: u64,
}

impl<'a> Region<'a> {
    pub fn new(dev: &'a mut dyn BlockDevice, base: u64, len: u64) -> Region<'a> {
        Region { dev, base, len }
    }

    /// The whole device.
    pub fn whole(dev: &'a mut dyn BlockDevice) -> Region<'a> {
        let len = dev.sector_count() * dev.sector_size() as u64;
        Region { dev, base: 0, len }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn sector_size(&self) -> u32 {
        self.dev.sector_size()
    }

    pub fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Result<()> {
        self.transfer(off, buf.len(), |dev, lba, sectors, skip, part| {
            let ss = dev.sector_size() as usize;
            let mut tmp = vec![0u8; sectors * ss];
            dev.read(lba, &mut tmp)?;
            let n = part.1 - part.0;
            if let (Some(dst), Some(src)) = (buf.get_mut(part.0..part.1), tmp.get(skip..skip + n)) {
                dst.copy_from_slice(src);
            }
            Ok(())
        })
    }

    pub fn write_at(&mut self, off: u64, data: &[u8]) -> Result<()> {
        self.transfer(off, data.len(), |dev, lba, sectors, skip, part| {
            let ss = dev.sector_size() as usize;
            let n = part.1 - part.0;
            if skip == 0 && n == sectors * ss {
                // Whole sectors: write straight from the caller's buffer.
                let slice = data.get(part.0..part.1).ok_or(Error::Size("buffer range out of bounds"))?;
                dev.write(lba, slice)?;
            } else {
                let mut tmp = vec![0u8; sectors * ss];
                dev.read(lba, &mut tmp)?;
                if let (Some(dst), Some(src)) = (tmp.get_mut(skip..skip + n), data.get(part.0..part.1)) {
                    dst.copy_from_slice(src);
                }
                dev.write(lba, &tmp)?;
            }
            Ok(())
        })
    }

    pub fn flush(&mut self) -> Result<()> {
        Ok(self.dev.flush()?)
    }

    /// Splits `[off, off+len)` into an unaligned head, an aligned middle and an unaligned tail and
    /// calls `f(dev, first_lba, sector_count, byte_skip_in_first_sector, (buf_start, buf_end))`.
    fn transfer(
        &mut self,
        off: u64,
        len: usize,
        mut f: impl FnMut(&mut dyn BlockDevice, u64, usize, usize, (usize, usize)) -> Result<()>,
    ) -> Result<()> {
        if off.checked_add(len as u64).is_none_or(|e| e > self.len) {
            return Err(Error::Size("access beyond region"));
        }
        let ss = self.dev.sector_size() as u64;
        let mut pos = self.base + off;
        let mut done = 0usize;
        // Large aligned runs go in 1 MiB steps.
        let max_sectors = ((1u64 << 20) / ss).max(1) as usize;
        while done < len {
            let lba = pos / ss;
            let skip = (pos % ss) as usize;
            let remaining = len - done;
            let sectors = if skip == 0 && remaining as u64 >= ss {
                ((remaining as u64 / ss) as usize).min(max_sectors)
            } else {
                1
            };
            let n = (sectors * ss as usize - skip).min(remaining);
            f(&mut *self.dev, lba, sectors, skip, (done, done + n))?;
            done += n;
            pos += n as u64;
        }
        Ok(())
    }
}
