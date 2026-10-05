//! virtio 1.x over PCI (modern transport only), split virtqueues, polled.
#[cfg(feature = "virtio-blk")]
mod blk;
#[cfg(feature = "virtio-net")]
mod net;

use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::{platform, Devices};
use alloc::boxed::Box;
use core::sync::atomic::{fence, Ordering};
use hal::{Error, Result};

const VENDOR: u16 = 0x1af4;

const STATUS_ACK: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;
const STATUS_FAILED: u8 = 0x80;

const CAP_COMMON: u8 = 1;
const CAP_NOTIFY: u8 = 2;
const CAP_DEVICE: u8 = 4;

pub const DESC_F_NEXT: u16 = 1;
pub const DESC_F_WRITE: u16 = 2;

const MAX_QUEUE: u16 = 64;

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.vendor != VENDOR {
        return;
    }
    // Transitional ids 0x1000..=0x103f and modern ids 0x1040 + type.
    let kind = match dev.device {
        #[cfg(feature = "virtio-net")]
        0x1000 | 0x1041 => Kind::Net,
        #[cfg(feature = "virtio-blk")]
        0x1001 | 0x1042 => Kind::Blk,
        _ => return,
    };
    let Some(t) = Transport::new(dev) else { return };
    match kind {
        #[cfg(feature = "virtio-blk")]
        Kind::Blk => match blk::VirtioBlk::new(t) {
            Ok(d) => out.block.push(Box::new(d)),
            Err(_) => {}
        },
        #[cfg(feature = "virtio-net")]
        Kind::Net => match net::VirtioNet::new(t) {
            Ok(d) => out.net.push(Box::new(d)),
            Err(_) => {}
        },
    }
}

enum Kind {
    #[cfg(feature = "virtio-blk")]
    Blk,
    #[cfg(feature = "virtio-net")]
    Net,
}

pub struct Transport {
    common: Mmio,
    notify: Mmio,
    notify_mult: u32,
    pub device_cfg: Mmio,
}

impl Transport {
    fn new(dev: &PciDevice) -> Option<Transport> {
        let mut common = None;
        let mut notify = None;
        let mut notify_mult = 0;
        let mut device_cfg = None;
        for (off, id) in dev.capabilities() {
            if id != 0x09 {
                continue;
            }
            let cfg_type = dev.read8(off + 3);
            let bar = dev.read8(off + 4);
            let offset = dev.read32(off + 8) as usize;
            let map = |dev: &PciDevice| dev.map_bar(bar).map(|m| m.offset(offset));
            match cfg_type {
                CAP_COMMON => common = map(dev),
                CAP_NOTIFY => {
                    notify = map(dev);
                    notify_mult = dev.read32(off + 16);
                }
                CAP_DEVICE => device_cfg = map(dev),
                _ => {}
            }
        }
        dev.enable();
        Some(Transport { common: common?, notify: notify?, notify_mult, device_cfg: device_cfg? })
    }

    /// Resets the device and negotiates features. `wanted` are low-word feature bits that are
    /// accepted if offered; VERSION_1 is mandatory. Returns the accepted low-word bits.
    fn negotiate(&self, wanted: u32) -> Result<u32> {
        let c = &self.common;
        c.write8(0x14, 0);
        platform::wait_until(100, || c.read8(0x14) == 0)?;
        c.write8(0x14, STATUS_ACK);
        c.write8(0x14, STATUS_ACK | STATUS_DRIVER);
        c.write32(0x00, 0);
        let lo = c.read32(0x04);
        c.write32(0x00, 1);
        let hi = c.read32(0x04);
        if hi & 1 == 0 {
            c.write8(0x14, STATUS_FAILED);
            return Err(Error::Unsupported);
        }
        let accepted = lo & wanted;
        c.write32(0x08, 0);
        c.write32(0x0c, accepted);
        c.write32(0x08, 1);
        c.write32(0x0c, 1); // VERSION_1
        c.write8(0x14, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
        if c.read8(0x14) & STATUS_FEATURES_OK == 0 {
            c.write8(0x14, STATUS_FAILED);
            return Err(Error::Unsupported);
        }
        Ok(accepted)
    }

    fn queue(&self, index: u16) -> Result<VirtQueue> {
        let c = &self.common;
        c.write16(0x16, index);
        let max = c.read16(0x18);
        if max == 0 {
            return Err(Error::Unsupported);
        }
        let size = max.min(MAX_QUEUE);
        c.write16(0x18, size);
        let q = VirtQueue::new(size, index);
        c.write64(0x20, q.mem.phys_at(q.desc_off));
        c.write64(0x28, q.mem.phys_at(q.avail_off));
        c.write64(0x30, q.mem.phys_at(q.used_off));
        c.write16(0x1c, 1);
        let notify_off = c.read16(0x1e) as usize;
        Ok(VirtQueue { notify: self.notify.offset(notify_off * self.notify_mult as usize), ..q })
    }

    fn driver_ok(&self) {
        let s = self.common.read8(0x14);
        self.common.write8(0x14, s | STATUS_DRIVER_OK);
    }
}

pub struct VirtQueue {
    mem: Dma,
    size: u16,
    desc_off: usize,
    avail_off: usize,
    used_off: usize,
    avail_idx: u16,
    last_used: u16,
    notify: Mmio,
    index: u16,
}

impl VirtQueue {
    fn new(size: u16, index: u16) -> VirtQueue {
        let n = size as usize;
        let desc_off = 0;
        let avail_off = 16 * n;
        let used_off = (avail_off + 6 + 2 * n + 3) & !3;
        let total = used_off + 6 + 8 * n;
        VirtQueue {
            mem: Dma::new(total, 4096),
            size,
            desc_off,
            avail_off,
            used_off,
            avail_idx: 0,
            last_used: 0,
            notify: Mmio::new(core::ptr::null_mut()),
            index,
        }
    }

    pub fn size(&self) -> u16 {
        self.size
    }

    pub fn set_desc(&mut self, i: u16, addr: u64, len: u32, flags: u16, next: u16) {
        let d = Mmio::new(self.mem.as_ptr()).offset(self.desc_off + 16 * i as usize);
        d.write64(0, addr);
        d.write32(8, len);
        d.write16(12, flags);
        d.write16(14, next);
    }

    /// Makes the chain starting at `head` available and notifies the device.
    pub fn submit(&mut self, head: u16) {
        let m = Mmio::new(self.mem.as_ptr()).offset(self.avail_off);
        m.write16(4 + 2 * (self.avail_idx % self.size) as usize, head);
        fence(Ordering::SeqCst);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        m.write16(2, self.avail_idx);
        fence(Ordering::SeqCst);
        self.notify.write16(0, self.index);
    }

    /// Pops one completed chain as `(head, bytes written by device)`.
    pub fn pop_used(&mut self) -> Option<(u16, u32)> {
        let m = Mmio::new(self.mem.as_ptr()).offset(self.used_off);
        fence(Ordering::SeqCst);
        if m.read16(2) == self.last_used {
            return None;
        }
        let e = m.offset(4 + 8 * (self.last_used % self.size) as usize);
        let r = (e.read32(0) as u16, e.read32(4));
        self.last_used = self.last_used.wrapping_add(1);
        Some(r)
    }
}
