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

    pub fn sector_size(&self) -> u32 {
        self.dev.sector_size()
    }

    pub fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Result<()> {
        self.transfer(off, buf.len(), |dev, lba, sectors, skip, part| {
            let ss = dev.sector_size() as usize;
            let mut tmp = vec![0u8; sectors * ss];
            dev.read(lba, &mut tmp)?;
            buf[part.0..part.1].copy_from_slice(&tmp[skip..skip + part.1 - part.0]);
            Ok(())
        })
    }

    pub fn write_at(&mut self, off: u64, data: &[u8]) -> Result<()> {
        self.transfer(off, data.len(), |dev, lba, sectors, skip, part| {
            let ss = dev.sector_size() as usize;
            let n = part.1 - part.0;
            if skip == 0 && n == sectors * ss {
                // Whole sectors: write straight from the caller's buffer.
                dev.write(lba, &data[part.0..part.1])?;
            } else {
                let mut tmp = vec![0u8; sectors * ss];
                dev.read(lba, &mut tmp)?;
                tmp[skip..skip + n].copy_from_slice(&data[part.0..part.1]);
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
        if off.checked_add(len as u64).map_or(true, |e| e > self.len) {
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
