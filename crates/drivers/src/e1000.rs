//! Intel e1000 (82540/82545) and e1000e (82574 and relatives) with legacy descriptors, polled.
use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::{platform, Devices};
use alloc::boxed::Box;
use hal::{Error, NetDevice, Result};

const CTRL: usize = 0x0000;
const STATUS: usize = 0x0008;
const IMC: usize = 0x00d8;
const RCTL: usize = 0x0100;
const TCTL: usize = 0x0400;
const TIPG: usize = 0x0410;
const RDBAL: usize = 0x2800;
const RDBAH: usize = 0x2804;
const RDLEN: usize = 0x2808;
const RDH: usize = 0x2810;
const RDT: usize = 0x2818;
const RXDCTL: usize = 0x2828;
const TDBAL: usize = 0x3800;
const TDBAH: usize = 0x3804;
const TDLEN: usize = 0x3808;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const TXDCTL: usize = 0x3828;
const MTA: usize = 0x5200;
const RAL0: usize = 0x5400;
const RAH0: usize = 0x5404;

const CTRL_SLU: u32 = 1 << 6;
const CTRL_RST: u32 = 1 << 26;
const CTRL_PHY_RST: u32 = 1 << 31;
const STATUS_LU: u32 = 1 << 1;

const RCTL_EN: u32 = 1 << 1;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_SECRC: u32 = 1 << 26;
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;

const DESC_DD: u8 = 1;
const TX_CMD: u8 = 0x01 | 0x02 | 0x08; // EOP | IFCS | RS

const RING: usize = 32;
const BUF: usize = 2048;

/// PCI ids of the 8254x "PRO/1000" family (Linux's `e1000` driver): PCI and PCI-X, no queue enable bits.
const IDS_LEGACY: &[u16] = &[
    0x1000, 0x1001, 0x1004, 0x1008, 0x1009, 0x100c, 0x100d, 0x100e, 0x100f, 0x1010, 0x1011, 0x1012,
    0x1013, 0x1014, 0x1015, 0x1016, 0x1017, 0x1018, 0x1019, 0x101a, 0x101d, 0x101e, 0x1026, 0x1027,
    0x1028, 0x1075, 0x1076, 0x1077, 0x1078, 0x1079, 0x107a, 0x107b, 0x107c, 0x108a, 0x1099, 0x10b5,
    0x2e6e,
];

/// PCI ids of the PCIe 8257x/8258x/80003ES2LAN parts and the ICH8..ICH10 / PCH integrated MACs, up to
/// the I219 (Linux's `e1000e`). Only the copper models bring up a link; the fiber and SerDes ones are
/// listed for completeness. The I217 and later (ids 0x153a and above) are the least certain: the
/// installer has run on none of them.
const IDS_E1000E: &[u16] = &[
    0x0d4c, 0x0d4d, 0x0d4e, 0x0d4f, 0x0d53, 0x0d55, 0x0dc5, 0x0dc6, 0x0dc7, 0x0dc8, 0x1049, 0x104a,
    0x104b, 0x104c, 0x104d, 0x105e, 0x105f, 0x1060, 0x107d, 0x107e, 0x107f, 0x108b, 0x108c, 0x1096,
    0x1098, 0x109a, 0x10a4, 0x10a5, 0x10b9, 0x10ba, 0x10bb, 0x10bc, 0x10bd, 0x10bf, 0x10c0, 0x10c2,
    0x10c3, 0x10c4, 0x10c5, 0x10cb, 0x10cc, 0x10cd, 0x10ce, 0x10d3, 0x10d5, 0x10d9, 0x10da, 0x10de,
    0x10df, 0x10e5, 0x10ea, 0x10eb, 0x10ef, 0x10f0, 0x10f5, 0x10f6, 0x1501, 0x1502, 0x1503, 0x150c,
    0x1525, 0x153a, 0x153b, 0x1559, 0x155a, 0x156f, 0x1570, 0x15a0, 0x15a1, 0x15a2, 0x15a3, 0x15b7,
    0x15b8, 0x15b9, 0x15bb, 0x15bc, 0x15bd, 0x15be, 0x15d6, 0x15d7, 0x15d8, 0x15df, 0x15e0, 0x15e1,
    0x15e2, 0x15e3, 0x15f4, 0x15f5, 0x15f9, 0x15fa, 0x15fb, 0x15fc, 0x1a1c, 0x1a1d, 0x1a1e, 0x1a1f,
    0x294c, 0x550a, 0x550b, 0x550c, 0x550d, 0x550e, 0x550f, 0x5510, 0x5511, 0x57a0, 0x57a1, 0x57b3,
    0x57b4, 0x57b7, 0x57b8, 0x57b9, 0x57ba,
];

fn family(device: u16) -> Option<bool> {
    if IDS_LEGACY.contains(&device) {
        Some(false)
    } else if IDS_E1000E.contains(&device) {
        Some(true)
    } else {
        None
    }
}

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.vendor != 0x8086 || family(dev.device).is_none() || dev.class != 0x02 {
        return;
    }
    let Some(regs) = dev.map_bar(0) else { return };
    dev.enable();
    if let Ok(d) = E1000::new(regs, family(dev.device) == Some(true)) {
        out.net.push(Box::new(d));
    }
}

pub struct E1000 {
    regs: Mmio,
    rx_ring: Dma,
    tx_ring: Dma,
    rx_bufs: Dma,
    tx_buf: Dma,
    rx_next: usize,
    tx_next: usize,
    mac: [u8; 6],
    e1000e: bool,
}

impl E1000 {
    fn new(regs: Mmio, e1000e: bool) -> Result<E1000> {
        regs.write32(IMC, 0xffff_ffff);
        regs.write32(RCTL, 0);
        regs.write32(TCTL, TCTL_PSP);
        regs.write32(CTRL, regs.read32(CTRL) | CTRL_RST);
        platform::delay_us(10_000);
        platform::wait_until(1000, || regs.read32(CTRL) & CTRL_RST == 0)?;
        regs.write32(IMC, 0xffff_ffff);
        regs.write32(CTRL, (regs.read32(CTRL) | CTRL_SLU) & !CTRL_PHY_RST);

        let ral = regs.read32(RAL0);
        let rah = regs.read32(RAH0);
        let mac = [ral as u8, (ral >> 8) as u8, (ral >> 16) as u8, (ral >> 24) as u8, rah as u8, (rah >> 8) as u8];
        if mac == [0; 6] {
            return Err(Error::Unsupported);
        }
        for i in 0..128 {
            regs.write32(MTA + 4 * i, 0);
        }

        let rx_ring = Dma::new(RING * 16, 128);
        let tx_ring = Dma::new(RING * 16, 128);
        let rx_bufs = Dma::new(RING * BUF, 4096);
        let tx_buf = Dma::new(BUF, 16);

        let rd = Mmio::new(rx_ring.as_ptr());
        for i in 0..RING {
            rd.write64(16 * i, rx_bufs.phys_at(i * BUF));
        }
        regs.write32(RDBAL, rx_ring.phys() as u32);
        regs.write32(RDBAH, (rx_ring.phys() >> 32) as u32);
        regs.write32(RDLEN, (RING * 16) as u32);
        regs.write32(RDH, 0);
        regs.write32(RDT, 0);
        regs.write32(TDBAL, tx_ring.phys() as u32);
        regs.write32(TDBAH, (tx_ring.phys() >> 32) as u32);
        regs.write32(TDLEN, (RING * 16) as u32);
        regs.write32(TDH, 0);
        regs.write32(TDT, 0);
        if e1000e {
            regs.write32(RXDCTL, regs.read32(RXDCTL) | 1 << 25);
            regs.write32(TXDCTL, regs.read32(TXDCTL) | 1 << 25);
            let _ = platform::wait_until(100, || regs.read32(RXDCTL) & (1 << 25) != 0);
        }
        regs.write32(TIPG, 0x0060_200a);
        regs.write32(TCTL, TCTL_EN | TCTL_PSP | (0x0f << 4) | (0x40 << 12));
        regs.write32(RCTL, RCTL_EN | RCTL_BAM | RCTL_SECRC);
        regs.write32(RDT, (RING - 1) as u32);

        Ok(E1000 { regs, rx_ring, tx_ring, rx_bufs, tx_buf, rx_next: 0, tx_next: 0, mac, e1000e })
    }
}

impl NetDevice for E1000 {
    fn name(&self) -> &str {
        if self.e1000e {
            "e1000e"
        } else {
            "e1000"
        }
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        self.regs.read32(STATUS) & STATUS_LU != 0
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() > BUF || frame.is_empty() {
            return Err(Error::InvalidArgument);
        }
        let d = Mmio::new(self.tx_ring.as_ptr()).offset(16 * self.tx_next);
        self.tx_buf.as_mut_slice()[..frame.len()].copy_from_slice(frame);
        d.write64(0, self.tx_buf.phys());
        d.write16(8, frame.len() as u16);
        d.write8(10, 0);
        d.write8(11, TX_CMD);
        d.write8(12, 0);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.tx_next = (self.tx_next + 1) % RING;
        self.regs.write32(TDT, self.tx_next as u32);
        platform::wait_until(1000, || d.read8(12) & DESC_DD != 0)
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let d = Mmio::new(self.rx_ring.as_ptr()).offset(16 * self.rx_next);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        if d.read8(12) & DESC_DD == 0 {
            return None;
        }
        let len = d.read16(8) as usize;
        let n = len.min(buf.len());
        let off = self.rx_next * BUF;
        buf[..n].copy_from_slice(&self.rx_bufs.as_slice()[off..off + n]);
        d.write8(12, 0);
        self.regs.write32(RDT, self.rx_next as u32);
        self.rx_next = (self.rx_next + 1) % RING;
        Some(n)
    }
}
