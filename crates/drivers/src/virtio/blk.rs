use super::{Transport, VirtQueue, DESC_F_NEXT, DESC_F_WRITE};
use crate::dma::Dma;
use crate::platform;
use alloc::string::String;
use hal::{BlockDevice, Error, Result};

const F_FLUSH: u32 = 1 << 9;

const T_IN: u32 = 0;
const T_OUT: u32 = 1;
const T_FLUSH: u32 = 4;
const T_GET_ID: u32 = 8;

const SECTOR: usize = 512;
/// Bounce buffer size; larger transfers are split.
const CHUNK: usize = 128 * 1024;

pub struct VirtioBlk {
    q: VirtQueue,
    capacity: u64,
    flush: bool,
    serial: String,
    /// Request header (16 bytes) + status byte (offset 16).
    hdr: Dma,
    data: Dma,
}

impl VirtioBlk {
    pub fn new(t: Transport) -> Result<VirtioBlk> {
        let accepted = t.negotiate(F_FLUSH)?;
        let q = t.queue(0)?;
        t.driver_ok();
        let mut d = VirtioBlk {
            q,
            capacity: t.device_cfg.read64(0),
            flush: accepted & F_FLUSH != 0,
            serial: String::new(),
            hdr: Dma::new(64, 16),
            data: Dma::new(CHUNK, 4096),
        };
        let mut id = [0u8; 20];
        if d.request(T_GET_ID, 0, 20, true).is_ok() {
            id.copy_from_slice(&d.data.as_slice()[..20]);
            let end = id.iter().position(|&b| b == 0).unwrap_or(20);
            d.serial = String::from_utf8_lossy(&id[..end]).trim().into();
        }
        Ok(d)
    }

    /// Runs one request; data goes through `self.data`. `dev_writes` is true if the device fills
    /// the data buffer.
    fn request(&mut self, ty: u32, sector: u64, len: usize, dev_writes: bool) -> Result<()> {
        let h = self.hdr.as_ptr();
        unsafe {
            (h as *mut u32).write_volatile(ty);
            (h.add(4) as *mut u32).write_volatile(0);
            (h.add(8) as *mut u64).write_volatile(sector);
            h.add(16).write_volatile(0xff);
        }
        self.q.set_desc(0, self.hdr.phys(), 16, DESC_F_NEXT, if len > 0 { 1 } else { 2 });
        if len > 0 {
            let flags = DESC_F_NEXT | if dev_writes { DESC_F_WRITE } else { 0 };
            self.q.set_desc(1, self.data.phys(), len as u32, flags, 2);
        }
        self.q.set_desc(2, self.hdr.phys_at(16), 1, DESC_F_WRITE, 0);
        self.q.submit(0);
        platform::wait_until(5000, || {
            self.q.pop_used().is_some()
        })?;
        match unsafe { h.add(16).read_volatile() } {
            0 => Ok(()),
            2 => Err(Error::Unsupported),
            _ => Err(Error::Io),
        }
    }

    fn check(&self, lba: u64, len: usize) -> Result<()> {
        if len % SECTOR != 0 || lba.checked_add((len / SECTOR) as u64).map_or(true, |e| e > self.capacity) {
            return Err(Error::InvalidArgument);
        }
        Ok(())
    }
}

impl BlockDevice for VirtioBlk {
    fn model(&self) -> &str {
        "virtio-blk"
    }
    fn serial(&self) -> &str {
        &self.serial
    }
    fn sector_size(&self) -> u32 {
        SECTOR as u32
    }
    fn sector_count(&self) -> u64 {
        self.capacity
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        for (i, chunk) in buf.chunks_mut(CHUNK).enumerate() {
            let sector = lba + (i * CHUNK / SECTOR) as u64;
            self.request(T_IN, sector, chunk.len(), true)?;
            chunk.copy_from_slice(&self.data.as_slice()[..chunk.len()]);
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        for (i, chunk) in buf.chunks(CHUNK).enumerate() {
            let sector = lba + (i * CHUNK / SECTOR) as u64;
            self.data.as_mut_slice()[..chunk.len()].copy_from_slice(chunk);
            self.request(T_OUT, sector, chunk.len(), false)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if !self.flush {
            return Ok(());
        }
        self.request(T_FLUSH, 0, 0, false)
    }
}
