//! Realtek RTL8168/8169 gigabit and RTL8125/8126 multi-gigabit NICs, polled.
//!
//! UNTESTED on hardware: QEMU has no r8169 or r8125 model. This is the generic init sequence
//! shared by the family; chip revisions that need PHY or OCP setup (the 8125/8126 in particular
//! have chip-specific MAC tuning in the Linux driver) may not link up.
use crate::dma::Dma;
use crate::r8169_chip as chip;
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
const MAR0: usize = 0x08;
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

/// Receive descriptors. The CPU does not poll while it decrypts a TLS record (milliseconds on an old
/// CPU), and a gigabit sender can deliver a whole TCP window in that time, so the ring must hold a
/// window's worth of frames (256 x 2 KiB = 512 KiB) or the card drops them and the download stalls.
const RING: usize = 256;
const BUF: usize = 2048;

/// Realtek's own PCI ids, plus the D-Link boards built around the same chip.
fn supported(vendor: u16, device: u16) -> bool {
    match vendor {
        0x10ec => matches!(device, 0x8168 | 0x8169 | 0x8161 | 0x8162 | 0x8167 | 0x8136 | 0x8129 | 0x2502 | 0x2600 | 0x8125 | 0x8126),
        0x1186 => matches!(device, 0x4300 | 0x4302),
        _ => false,
    }
}

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if !supported(dev.vendor, dev.device) {
        return;
    }
    // Memory BAR is BAR2 on PCIe parts, BAR1 on older PCI ones.
    let Some(regs) = dev.map_bar(2).or_else(|| dev.map_bar(1)) else { return };
    dev.enable();
    if let Ok(d) = R8169::new(dev, regs, matches!(dev.device, 0x8125 | 0x8126)) {
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
    /// Frames sent / frames received / receive errors so far (the first few are logged).
    tx_n: u32,
    rx_n: u32,
    rx_err: u32,
    /// Receive diagnostics while nothing has arrived yet: when last logged and how many times.
    diag_t: u64,
    diag_n: u32,
    /// Linux's MAC version of the chip (0: unknown); some revisions need a patch whenever the link comes up.
    ver: u8,
    last_link: bool,
    link_t: u64,
}

/// Writes a 64-bit DMA address as two 32-bit register writes, high half first: these chips do not
/// handle a single 64-bit access, and Linux writes them in this order too.
fn write_dma_addr(regs: &Mmio, off: usize, phys: u64) {
    regs.write32(off + 4, (phys >> 32) as u32);
    regs.write32(off, phys as u32);
}

impl R8169 {
    fn new(dev: &PciDevice, regs: Mmio, is8125: bool) -> Result<R8169> {
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

        let xid = (regs.read32(TX_CONFIG) >> 20) & 0xfcf; // chip revision, as Linux reads it
        let (ver, name) = chip::identify(xid);
        hal::log!("r8169: pci {:04x}:{:04x} rev {:#04x}, chip xid {xid:#x} = {name} (MAC version {ver})", dev.vendor, dev.device, dev.revision);
        regs.write8(CFG9346, 0xc0); // unlock config registers
        if !is8125 {
            chip::hw_start(&chip::Ctx { regs: &regs, dev }, ver);
        }
        regs.write16(RX_MAX_SIZE, 0x3fff);
        if is8125 {
            regs.write8(MAX_TX_PACKET, 0x3b);
        }
        write_dma_addr(&regs, TNPDS, tx_ring.phys());
        write_dma_addr(&regs, RDSAR, rx_ring.phys());
        regs.write8(CFG9346, 0x00);
        regs.write8(CR, CR_RE | CR_TE);
        // Receive config for the revision (Linux's rtl_init_rxcfg), then accept broadcast, own MAC and
        // multicast, with the multicast filter open.
        regs.write32(RX_CONFIG, if is8125 { 0x0000_c700 } else { chip::rx_config(ver) });
        regs.write32(TX_CONFIG, if is8125 { 0x0300_0780 } else { chip::tx_config(ver) });
        regs.write32(MAR0 + 4, 0xffff_ffff);
        regs.write32(MAR0, 0xffff_ffff);
        regs.write32(RX_CONFIG, (regs.read32(RX_CONFIG) & !0x3f) | 0x0e);

        // Power up the PHY and (re)start auto-negotiation advertising 10/100/1000, the way Linux's
        // generic path does; firmware leaves the PHY in whatever state it last used.
        if !is8125 {
            chip::phy_write(&regs, 0x1f, 0); // register page 0
            chip::phy_write(&regs, 4, 0x01e1); // 10/100 half and full duplex
            chip::phy_write(&regs, 9, 0x0300); // 1000 half and full duplex (ignored by a 100 Mbit/s PHY)
            chip::phy_write(&regs, 0, 0x1200); // auto-negotiation enable + restart
            let up = platform::wait_until(5000, || regs.read8(PHY_STATUS) & PHY_LINK != 0).is_ok();
            hal::log!(
                "r8169: PHYstatus {:#04x}, link {}; PHY bmcr {:x?} bmsr {:x?} anar {:x?} anlpar {:x?} gbcr {:x?} gbsr {:x?}",
                regs.read8(PHY_STATUS),
                if up { "up" } else { "down" },
                chip::phy_read(&regs, 0),
                chip::phy_read(&regs, 1),
                chip::phy_read(&regs, 4),
                chip::phy_read(&regs, 5),
                chip::phy_read(&regs, 9),
                chip::phy_read(&regs, 10)
            );
        } else {
            hal::log!("r8125: PHYstatus {:#04x}", regs.read8(PHY_STATUS));
        }

        Ok(R8169 { regs, rx_ring, tx_ring, rx_bufs, tx_buf, rx_next: 0, tx_next: 0, mac, is8125, tx_n: 0, rx_n: 0, rx_err: 0, diag_t: 0, diag_n: 0, ver, last_link: false, link_t: 0 })
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

impl R8169 {
    /// Current link state; on a rising edge applies the revision's per-link tuning.
    fn poll_link(&mut self) -> bool {
        let up = self.regs.read8(PHY_STATUS) & PHY_LINK != 0;
        if up && !self.last_link && chip::needs_link_patch(self.ver) {
            chip::link_changed(&self.regs, self.ver, self.regs.read8(PHY_STATUS));
            hal::log!("r8169: link up, PHYstatus {:#04x}, applied the link patch for MAC version {}", self.regs.read8(PHY_STATUS), self.ver);
        }
        self.last_link = up;
        up
    }
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
        self.poll_link()
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
        if self.tx_n < 4 {
            hal::log!("r8169: tx frame {} bytes", frame.len());
        }
        self.tx_n += 1;
        if self.is8125 {
            self.regs.write16(TXPOLL_8125, 1);
        } else {
            self.regs.write8(TXPOLL, TXPOLL_NPQ);
        }
        platform::wait_until(1000, || d.read32(0) & DESC_OWN == 0)
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        // Notice a link coming up after the initial wait (auto-negotiation can take seconds).
        let now = platform::now_ns();
        if chip::needs_link_patch(self.ver) && now.wrapping_sub(self.link_t) > 100_000_000 {
            self.link_t = now;
            self.poll_link();
        }
        if self.diag_n < 8 && !self.is8125 && (self.rx_n == 0 || self.regs.read32(0x4c) > 0) {
            let now = platform::now_ns();
            if now.wrapping_sub(self.diag_t) > 2_000_000_000 {
                self.diag_t = now;
                self.diag_n += 1;
                let r = &self.regs;
                // The interrupt status latches events even with interrupts masked: bit0 RxOK, bit1 RxErr,
                // bit2 TxOK, bit4 RxOverflow, bit6 link change, bit7 RxDescUnavailable, bit8 TxDescUnavailable.
                hal::log!(
                    "r8169: status {:#06x} rxmissed {} cmd {:#04x} rxcfg {:#010x} phy {:#04x} rdsar {:#x}{:08x} (want {:#x}) rx[0] {:#010x}",
                    r.read16(INTR_STATUS),
                    r.read32(0x4c),
                    r.read8(CR),
                    r.read32(RX_CONFIG),
                    r.read8(PHY_STATUS),
                    r.read32(RDSAR + 4),
                    r.read32(RDSAR),
                    self.rx_ring.phys(),
                    Mmio::new(self.rx_ring.as_ptr()).read32(0)
                );
            }
        }
        let d = Mmio::new(self.rx_ring.as_ptr()).offset(16 * self.rx_next);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let opts = d.read32(0);
        if opts & DESC_OWN != 0 {
            return None;
        }
        let len = (opts & 0x3fff) as usize;
        let n = len.saturating_sub(4).min(buf.len()); // strip FCS
        let ok = opts & RX_RES == 0;
        if self.rx_n < 4 || (!ok && self.rx_err < 4) {
            hal::log!("r8169: rx descriptor {opts:#010x} ({len} bytes){}", if ok { "" } else { ", error" });
        }
        self.rx_n += 1;
        if !ok {
            self.rx_err += 1;
        }
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
