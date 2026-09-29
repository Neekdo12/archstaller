//! PCI configuration space access via legacy port I/O (mechanism #1) and enumeration.
use crate::mmio::Mmio;
use crate::platform;
use crate::port::{inl, outl};
use alloc::vec::Vec;

const CONFIG_ADDRESS: u16 = 0xcf8;
const CONFIG_DATA: u16 = 0xcfc;

pub const CMD_IO: u16 = 1 << 0;
pub const CMD_MEM: u16 = 1 << 1;
pub const CMD_BUS_MASTER: u16 = 1 << 2;
pub const CMD_INTX_DISABLE: u16 = 1 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addr {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct PciDevice {
    pub addr: Addr,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
}

#[derive(Clone, Copy, Debug)]
pub enum Bar {
    Mem { addr: u64, size: u64 },
    Io { port: u16, size: u32 },
}

fn cfg_addr(a: Addr, off: u16) -> u32 {
    0x8000_0000 | (a.bus as u32) << 16 | (a.dev as u32) << 11 | (a.func as u32) << 8 | (off as u32 & 0xfc)
}

pub fn read32(a: Addr, off: u16) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, cfg_addr(a, off));
        inl(CONFIG_DATA)
    }
}

pub fn write32(a: Addr, off: u16, v: u32) {
    unsafe {
        outl(CONFIG_ADDRESS, cfg_addr(a, off));
        outl(CONFIG_DATA, v);
    }
}

pub fn read16(a: Addr, off: u16) -> u16 {
    (read32(a, off) >> ((off & 2) * 8)) as u16
}

pub fn read8(a: Addr, off: u16) -> u8 {
    (read32(a, off) >> ((off & 3) * 8)) as u8
}

pub fn write16(a: Addr, off: u16, v: u16) {
    let shift = (off & 2) * 8;
    let old = read32(a, off) & !(0xffff << shift);
    write32(a, off, old | (v as u32) << shift);
}

/// Scans every bus/device/function.
pub fn enumerate() -> Vec<PciDevice> {
    let mut out = Vec::new();
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            let a0 = Addr { bus, dev, func: 0 };
            if read16(a0, 0) == 0xffff {
                continue;
            }
            let multi = read8(a0, 0x0e) & 0x80 != 0;
            for func in 0..if multi { 8 } else { 1 } {
                let addr = Addr { bus, dev, func };
                let id = read32(addr, 0);
                if id & 0xffff == 0xffff {
                    continue;
                }
                let class = read32(addr, 0x08);
                out.push(PciDevice {
                    addr,
                    vendor: id as u16,
                    device: (id >> 16) as u16,
                    class: (class >> 24) as u8,
                    subclass: (class >> 16) as u8,
                    prog_if: (class >> 8) as u8,
                    revision: class as u8,
                });
            }
        }
    }
    out
}

impl PciDevice {
    pub fn read32(&self, off: u16) -> u32 {
        read32(self.addr, off)
    }
    pub fn read16(&self, off: u16) -> u16 {
        read16(self.addr, off)
    }
    pub fn read8(&self, off: u16) -> u8 {
        read8(self.addr, off)
    }
    pub fn write32(&self, off: u16, v: u32) {
        write32(self.addr, off, v)
    }

    pub fn command(&self) -> u16 {
        read16(self.addr, 0x04)
    }

    pub fn set_command(&self, bits: u16) {
        write16(self.addr, 0x04, self.command() | bits);
    }

    pub fn enable(&self) {
        self.set_command(CMD_MEM | CMD_IO | CMD_BUS_MASTER);
    }

    /// Reads and sizes BAR `n`; returns `None` if unimplemented.
    pub fn bar(&self, n: u8) -> Option<Bar> {
        let off = 0x10 + 4 * n as u16;
        let orig = self.read32(off);
        let cmd = self.command();
        write16(self.addr, 0x04, cmd & !(CMD_IO | CMD_MEM));
        self.write32(off, 0xffff_ffff);
        let mask = self.read32(off);
        self.write32(off, orig);
        let is_io = orig & 1 != 0;
        let mut result = None;
        if is_io {
            let size = !(mask & !3) + 1;
            if mask & !3 != 0 {
                result = Some(Bar::Io { port: (orig & !3) as u16, size });
            }
        } else {
            let is64 = (orig >> 1) & 3 == 2;
            let (mut base, mut m) = ((orig & !0xf) as u64, (mask & !0xf) as u64);
            if is64 {
                let hi_off = off + 4;
                let orig_hi = self.read32(hi_off);
                self.write32(hi_off, 0xffff_ffff);
                let mask_hi = self.read32(hi_off);
                self.write32(hi_off, orig_hi);
                base |= (orig_hi as u64) << 32;
                m |= (mask_hi as u64) << 32;
            } else {
                m |= 0xffff_ffff_0000_0000;
            }
            if m & !0xf != 0 && (mask & !0xf) | (m >> 32) as u32 != 0 {
                result = Some(Bar::Mem { addr: base, size: (!m).wrapping_add(1) });
            }
        }
        write16(self.addr, 0x04, cmd);
        result
    }

    /// Maps BAR `n` (memory BARs only, uncached).
    pub fn map_bar(&self, n: u8) -> Option<Mmio> {
        match self.bar(n)? {
            Bar::Mem { addr, size } => Some(Mmio::new(platform::map_mmio(addr, size as usize))),
            Bar::Io { .. } => None,
        }
    }

    /// Iterates capability list entries as `(config offset, capability id)`.
    pub fn capabilities(&self) -> Vec<(u16, u8)> {
        let mut out = Vec::new();
        if self.read16(0x06) & (1 << 4) == 0 {
            return out;
        }
        let mut ptr = self.read8(0x34) & !3;
        let mut guard = 0;
        while ptr != 0 && guard < 48 {
            out.push((ptr as u16, self.read8(ptr as u16)));
            ptr = self.read8(ptr as u16 + 1) & !3;
            guard += 1;
        }
        out
    }
}
