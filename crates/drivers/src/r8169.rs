//! Realtek RTL8168/8169 gigabit and RTL8125/8126 multi-gigabit NICs, polled.
//!
//! UNTESTED on hardware: QEMU has no r8169 or r8125 model. This is the generic init sequence
//! shared by the family; chip revisions that need PHY or OCP setup (the 8125/8126 in particular
//! have chip-specific MAC tuning in the Linux driver) may not link up.
use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::{platform, Devices};
use alloc::boxed::Box;
use hal::{Error, NetDevice, Result};

const IDR0: usize = 0x00;
const TNPDS: usize = 0x20;
const CR: usize = 0x37;
const TXPOLL: usize = 0x38;
const INTR_MASK: usize = 0x3c;
const INTR_STATUS: usize = 0x3e;
// RTL8125/8126 move these and use a 32-bit interrupt status.
const INTR_MASK_8125: usize = 0x38;
const INTR_STATUS_8125: usize = 0x3c;
const TXPOLL_8125: usize = 0x90;
const TX_CONFIG: usize = 0x40;
const RX_CONFIG: usize = 0x44;
const CFG9346: usize = 0x50;
const PHY_STATUS: usize = 0x6c;
const RX_MAX_SIZE: usize = 0xda;
const RDSAR: usize = 0xe4;
const MAX_TX_PACKET: usize = 0xec;

const CR_RST: u8 = 1 << 4;
const CR_RE: u8 = 1 << 3;
const CR_TE: u8 = 1 << 2;
const TXPOLL_NPQ: u8 = 1 << 6;
const PHY_LINK: u8 = 1 << 1;

const DESC_OWN: u32 = 1 << 31;
const DESC_EOR: u32 = 1 << 30;
const DESC_FS: u32 = 1 << 29;
const DESC_LS: u32 = 1 << 28;
const RX_RES: u32 = 1 << 21;

const RING: usize = 32;
const BUF: usize = 2048;

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.vendor != 0x10ec || !matches!(dev.device, 0x8168 | 0x8169 | 0x8161 | 0x8167 | 0x8125 | 0x8126) {
        return;
    }
    // Memory BAR is BAR2 on PCIe parts, BAR1 on older PCI ones.
    let Some(regs) = dev.map_bar(2).or_else(|| dev.map_bar(1)) else { return };
    dev.enable();
    if let Ok(d) = R8169::new(regs, matches!(dev.device, 0x8125 | 0x8126)) {
        out.net.push(Box::new(d));
    }
}

pub struct R8169 {
    regs: Mmio,
    rx_ring: Dma,
    tx_ring: Dma,
    rx_bufs: Dma,
    tx_buf: Dma,
    rx_next: usize,
    tx_next: usize,
    mac: [u8; 6],
    is8125: bool,
}

impl R8169 {
    fn new(regs: Mmio, is8125: bool) -> Result<R8169> {
        regs.write8(CR, CR_RST);
        platform::wait_until(100, || regs.read8(CR) & CR_RST == 0)?;
        if is8125 {
            regs.write32(INTR_MASK_8125, 0);
            regs.write32(INTR_STATUS_8125, 0xffff_ffff);
        } else {
            regs.write16(INTR_MASK, 0);
            regs.write16(INTR_STATUS, 0xffff);
        }

        let mut mac = [0u8; 6];
        for (i, b) in mac.iter_mut().enumerate() {
            *b = regs.read8(IDR0 + i);
        }
        if mac == [0; 6] {
            return Err(Error::Unsupported);
        }

        let rx_ring = Dma::new(RING * 16, 256);
        let tx_ring = Dma::new(RING * 16, 256);
        let rx_bufs = Dma::new(RING * BUF, 4096);
        let tx_buf = Dma::new(BUF, 16);
        for i in 0..RING {
            init_rx_desc(&rx_ring, &rx_bufs, i);
        }
        let td = Mmio::new(tx_ring.as_ptr());
        td.write32(16 * (RING - 1), DESC_EOR);

        regs.write8(CFG9346, 0xc0); // unlock config registers
        regs.write16(RX_MAX_SIZE, 0x1fff);
        regs.write8(MAX_TX_PACKET, 0x3b);
        regs.write64(TNPDS, tx_ring.phys());
        regs.write64(RDSAR, rx_ring.phys());
        regs.write8(CR, CR_RE | CR_TE);
        regs.write32(TX_CONFIG, 0x0300_0700);
        regs.write32(RX_CONFIG, 0x0000_e70e); // broadcast, own MAC, multicast; no FIFO limits
        regs.write8(CFG9346, 0x00);

        Ok(R8169 { regs, rx_ring, tx_ring, rx_bufs, tx_buf, rx_next: 0, tx_next: 0, mac, is8125 })
    }
}

fn init_rx_desc(ring: &Dma, bufs: &Dma, i: usize) {
    let d = Mmio::new(ring.as_ptr()).offset(16 * i);
    d.write32(4, 0);
    d.write64(8, bufs.phys_at(i * BUF));
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    let eor = if i == RING - 1 { DESC_EOR } else { 0 };
    d.write32(0, DESC_OWN | eor | BUF as u32);
}

impl NetDevice for R8169 {
    fn name(&self) -> &str {
        if self.is8125 {
            "r8125"
        } else {
            "r8169"
        }
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        self.regs.read8(PHY_STATUS) & PHY_LINK != 0
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() > BUF || frame.is_empty() {
            return Err(Error::InvalidArgument);
        }
        let d = Mmio::new(self.tx_ring.as_ptr()).offset(16 * self.tx_next);
        self.tx_buf.as_mut_slice()[..frame.len()].copy_from_slice(frame);
        d.write32(4, 0);
        d.write64(8, self.tx_buf.phys());
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let eor = if self.tx_next == RING - 1 { DESC_EOR } else { 0 };
        d.write32(0, DESC_OWN | eor | DESC_FS | DESC_LS | frame.len() as u32);
        self.tx_next = (self.tx_next + 1) % RING;
        if self.is8125 {
            self.regs.write16(TXPOLL_8125, 1);
        } else {
            self.regs.write8(TXPOLL, TXPOLL_NPQ);
        }
        platform::wait_until(1000, || d.read32(0) & DESC_OWN == 0)
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let d = Mmio::new(self.rx_ring.as_ptr()).offset(16 * self.rx_next);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let opts = d.read32(0);
        if opts & DESC_OWN != 0 {
            return None;
        }
        let len = (opts & 0x3fff) as usize;
        let n = len.saturating_sub(4).min(buf.len()); // strip FCS
        let ok = opts & RX_RES == 0;
        let off = self.rx_next * BUF;
        if ok {
            buf[..n].copy_from_slice(&self.rx_bufs.as_slice()[off..off + n]);
        }
        init_rx_desc(&self.rx_ring, &self.rx_bufs, self.rx_next);
        self.rx_next = (self.rx_next + 1) % RING;
        if ok {
            Some(n)
        } else {
            None
        }
    }
}
