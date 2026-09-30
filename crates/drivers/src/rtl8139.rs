//! Realtek RTL8139 (10/100) NICs: one shared receive ring, four transmit slots, polled. Common
//! as the default emulated NIC of older hypervisors. Tested against QEMU's `rtl8139` model.
use crate::dma::Dma;
use crate::pci::{Bar, PciDevice};
use crate::{platform, port, Devices};
use alloc::boxed::Box;
use hal::{Error, NetDevice, Result};

const IDR0: u16 = 0x00;
const TSD0: u16 = 0x10;
const TSAD0: u16 = 0x20;
const RBSTART: u16 = 0x30;
const CR: u16 = 0x37;
const CAPR: u16 = 0x38;
const IMR: u16 = 0x3c;
const ISR: u16 = 0x3e;
const TCR: u16 = 0x40;
const RCR: u16 = 0x44;
const CONFIG1: u16 = 0x52;
const MSR: u16 = 0x58;

const CR_BUFE: u8 = 1;
const CR_TE: u8 = 1 << 2;
const CR_RE: u8 = 1 << 3;
const CR_RST: u8 = 1 << 4;
const MSR_LINK_DOWN: u8 = 1 << 2;
const TSD_OWN: u32 = 1 << 13;
const TSD_TOK: u32 = 1 << 15;
const RX_ROK: u32 = 1;

const RX_LEN: usize = 8192;
/// Ring plus room for a packet that starts near the end and spills past it (WRAP mode).
const RX_ALLOC: usize = RX_LEN + 16 + 2048;
const TX_SLOT: usize = 2048;

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.vendor != 0x10ec || dev.device != 0x8139 {
        return;
    }
    // BAR0 is I/O space, BAR1 the same registers in memory space.
    let Some(Bar::Io { port, .. }) = dev.bar(0) else { return };
    dev.enable();
    if let Ok(d) = Rtl8139::new(port) {
        out.net.push(Box::new(d));
    }
}

pub struct Rtl8139 {
    io: u16,
    rx: Dma,
    tx: Dma,
    rx_off: usize,
    tx_next: usize,
    mac: [u8; 6],
}

impl Rtl8139 {
    fn r8(&self, off: u16) -> u8 {
        unsafe { port::inb(self.io + off) }
    }
    fn w8(&self, off: u16, v: u8) {
        unsafe { port::outb(self.io + off, v) }
    }
    fn r32(&self, off: u16) -> u32 {
        unsafe { port::inl(self.io + off) }
    }
    fn w32(&self, off: u16, v: u32) {
        unsafe { port::outl(self.io + off, v) }
    }
    fn w16(&self, off: u16, v: u16) {
        unsafe { port::outw(self.io + off, v) }
    }

    fn new(io: u16) -> Result<Rtl8139> {
        let rx = Dma::new(RX_ALLOC, 16);
        let tx = Dma::new(4 * TX_SLOT, 16);
        // The chip addresses DMA memory with 32 bits only.
        if (rx.phys() + RX_ALLOC as u64) >> 32 != 0 || (tx.phys() + 4 * TX_SLOT as u64) >> 32 != 0 {
            return Err(Error::Unsupported);
        }
        let mut d = Rtl8139 { io, rx, tx, rx_off: 0, tx_next: 0, mac: [0; 6] };
        d.w8(CONFIG1, 0); // power on
        d.w8(CR, CR_RST);
        platform::wait_until(100, || d.r8(CR) & CR_RST == 0)?;
        for i in 0..6 {
            d.mac[i] = d.r8(IDR0 + i as u16);
        }
        d.w32(RBSTART, d.rx.phys() as u32);
        d.w16(IMR, 0);
        d.w16(ISR, 0xffff);
        d.w8(CR, CR_RE | CR_TE);
        // Accept broadcast, multicast and own address; wrap mode; unlimited DMA bursts.
        d.w32(RCR, 0x0000_e78e);
        d.w32(TCR, 0x0300_0700);
        Ok(d)
    }
}

impl NetDevice for Rtl8139 {
    fn name(&self) -> &str {
        "rtl8139"
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        self.r8(MSR) & MSR_LINK_DOWN == 0
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() > TX_SLOT - 4 || frame.is_empty() {
            return Err(Error::InvalidArgument);
        }
        let slot = self.tx_next;
        self.tx_next = (self.tx_next + 1) % 4;
        let buf = self.tx.as_mut_slice();
        buf[slot * TX_SLOT..slot * TX_SLOT + frame.len()].copy_from_slice(frame);
        let len = frame.len().max(60); // the chip does not pad
        self.w32(TSAD0 + 4 * slot as u16, self.tx.phys_at(slot * TX_SLOT) as u32);
        self.w32(TSD0 + 4 * slot as u16, len as u32); // OWN clear: start
        platform::wait_until(1000, || self.r32(TSD0 + 4 * slot as u16) & (TSD_TOK | TSD_OWN) != 0)
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        if self.r8(CR) & CR_BUFE != 0 {
            return None;
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let ring = self.rx.as_slice();
        let hdr = u32::from_le_bytes(ring[self.rx_off..self.rx_off + 4].try_into().unwrap());
        let len = (hdr >> 16) as usize; // includes the 4-byte CRC
        let ok = hdr & RX_ROK != 0 && (8..=1792).contains(&len);
        let n = if ok { (len - 4).min(buf.len()) } else { 0 };
        if ok {
            buf[..n].copy_from_slice(&ring[self.rx_off + 4..self.rx_off + 4 + n]);
        }
        if !ok {
            // Lost sync with the ring: restart the receiver.
            self.w8(CR, CR_TE);
            self.rx_off = 0;
            self.w32(RBSTART, self.rx.phys() as u32);
            self.w8(CR, CR_RE | CR_TE);
            self.w16(CAPR, 0xfff0);
            return None;
        }
        self.rx_off = (self.rx_off + len + 4 + 3) & !3;
        self.w16(CAPR, (self.rx_off as u16).wrapping_sub(16));
        self.rx_off %= RX_LEN;
        Some(n)
    }
}
