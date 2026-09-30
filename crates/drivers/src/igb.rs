//! Intel igb (82576/82580/I350/I210/I211) and igc (I225/I226) gigabit NICs: one RX/TX queue pair
//! with advanced descriptors, polled.
//!
//! igb is tested against QEMU's `igb` model. igc is UNTESTED: QEMU has no I225/I226 model, so it
//! follows the Linux driver's reset and queue setup without being able to run it.
use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::{platform, Devices};
use alloc::boxed::Box;
use hal::{Error, NetDevice, Result};

const CTRL: usize = 0x0000;
const STATUS: usize = 0x0008;
const EECD: usize = 0x0010;
const IMC: usize = 0x00d8;
const RCTL: usize = 0x0100;
const TCTL: usize = 0x0400;
const RDBAL: usize = 0xc000;
const RDBAH: usize = 0xc004;
const RDLEN: usize = 0xc008;
const SRRCTL: usize = 0xc00c;
const RDH: usize = 0xc010;
const RDT: usize = 0xc018;
const RXDCTL: usize = 0xc028;
const TDBAL: usize = 0xe000;
const TDBAH: usize = 0xe004;
const TDLEN: usize = 0xe008;
const TDH: usize = 0xe010;
const TDT: usize = 0xe018;
const TXDCTL: usize = 0xe028;
const MDIC: usize = 0x0020;
const MTA: usize = 0x5200;
const RAL0: usize = 0x5400;
const RAH0: usize = 0x5404;

const CTRL_SLU: u32 = 1 << 6;
const CTRL_RST: u32 = 1 << 26;
const CTRL_DEV_RST: u32 = 1 << 29; // igc
const STATUS_LU: u32 = 1 << 1;
const EECD_AUTO_RD: u32 = 1 << 9;

const RCTL_EN: u32 = 1 << 1;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_SECRC: u32 = 1 << 26;
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;
const QUEUE_ENABLE: u32 = 1 << 25;
/// SRRCTL: 2 KiB buffers (1 KiB units), advanced descriptors with one buffer.
const SRRCTL_ONE_BUFFER_2K: u32 = 2 | (1 << 25);

const MDIC_READY: u32 = 1 << 28;
const MDIC_ERROR: u32 = 1 << 30;
/// Internal copper PHY address and the standard MII basic control register.
const PHY_ADDR: u32 = 1;
const MII_BMCR: u32 = 0;
const BMCR_AUTONEG_RESTART: u32 = 0x1000 | 0x0200;

const RING: usize = 32;
const BUF: usize = 2048;

const TX_DTYP_DATA: u32 = 3 << 20;
const TX_EOP: u32 = 1 << 24;
const TX_IFCS: u32 = 1 << 25;
const TX_RS: u32 = 1 << 27;
const TX_DEXT: u32 = 1 << 29;
const DD: u32 = 1;

const IGB_IDS: &[u16] = &[
    0x10c9, 0x10e6, 0x10e7, 0x10e8, 0x150a, 0x150d, 0x150e, 0x150f, 0x1510, 0x1511, 0x1516, 0x1518, 0x1521, 0x1522,
    0x1523, 0x1524, 0x1533, 0x1534, 0x1535, 0x1536, 0x1537, 0x1538, 0x1539, 0x157b, 0x157c, 0x1f40, 0x1f41, 0x1f45,
];
const IGC_IDS: &[u16] = &[0x15f2, 0x15f3, 0x0d9f, 0x125b, 0x125c, 0x125d, 0x3100, 0x3101, 0x5502, 0x5503];

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Igb,
    Igc,
}

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.vendor != 0x8086 || dev.class != 0x02 {
        return;
    }
    let variant = if IGB_IDS.contains(&dev.device) {
        Variant::Igb
    } else if IGC_IDS.contains(&dev.device) {
        Variant::Igc
    } else {
        return;
    };
    let Some(regs) = dev.map_bar(0) else { return };
    dev.enable();
    if let Ok(d) = Igb::new(regs, variant) {
        out.net.push(Box::new(d));
    }
}

pub struct Igb {
    regs: Mmio,
    rx_ring: Dma,
    tx_ring: Dma,
    rx_bufs: Dma,
    tx_buf: Dma,
    rx_next: usize,
    tx_next: usize,
    mac: [u8; 6],
    variant: Variant,
}

impl Igb {
    fn mdic_write(regs: &Mmio, reg: u32, value: u32) -> Result<()> {
        regs.write32(MDIC, value | reg << 16 | PHY_ADDR << 21 | 1 << 26);
        platform::wait_until(100, || regs.read32(MDIC) & MDIC_READY != 0)?;
        if regs.read32(MDIC) & MDIC_ERROR != 0 {
            return Err(Error::Io);
        }
        Ok(())
    }

    fn new(regs: Mmio, variant: Variant) -> Result<Igb> {
        regs.write32(IMC, 0xffff_ffff);
        regs.write32(RCTL, 0);
        regs.write32(TCTL, TCTL_PSP);
        regs.read32(STATUS); // flush
        platform::delay_us(2_000);
        let reset = if variant == Variant::Igc { CTRL_DEV_RST } else { CTRL_RST };
        regs.write32(CTRL, regs.read32(CTRL) | reset);
        platform::delay_us(20_000);
        platform::wait_until(1000, || regs.read32(CTRL) & reset == 0)?;
        // The NVM autoload fills in the MAC address; wait for it.
        let _ = platform::wait_until(500, || regs.read32(EECD) & EECD_AUTO_RD != 0);
        regs.write32(IMC, 0xffff_ffff);
        regs.write32(CTRL, regs.read32(CTRL) | CTRL_SLU);
        if variant == Variant::Igb {
            // Start copper auto-negotiation the way the Linux driver does.
            let _ = Self::mdic_write(&regs, MII_BMCR, BMCR_AUTONEG_RESTART);
        }

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
            rd.write64(16 * i + 8, 0); // header buffer unused
        }
        regs.write32(RDBAL, rx_ring.phys() as u32);
        regs.write32(RDBAH, (rx_ring.phys() >> 32) as u32);
        regs.write32(RDLEN, (RING * 16) as u32);
        regs.write32(SRRCTL, SRRCTL_ONE_BUFFER_2K);
        regs.write32(RDH, 0);
        regs.write32(RDT, 0);
        regs.write32(RXDCTL, QUEUE_ENABLE);
        platform::wait_until(1000, || regs.read32(RXDCTL) & QUEUE_ENABLE != 0)?;
        regs.write32(RCTL, RCTL_EN | RCTL_BAM | RCTL_SECRC);
        regs.write32(RDT, (RING - 1) as u32);

        regs.write32(TDBAL, tx_ring.phys() as u32);
        regs.write32(TDBAH, (tx_ring.phys() >> 32) as u32);
        regs.write32(TDLEN, (RING * 16) as u32);
        regs.write32(TDH, 0);
        regs.write32(TDT, 0);
        regs.write32(TXDCTL, QUEUE_ENABLE);
        platform::wait_until(1000, || regs.read32(TXDCTL) & QUEUE_ENABLE != 0)?;
        regs.write32(TCTL, TCTL_EN | TCTL_PSP | (0x0f << 4) | (0x40 << 12));

        Ok(Igb { regs, rx_ring, tx_ring, rx_bufs, tx_buf, rx_next: 0, tx_next: 0, mac, variant })
    }
}

impl NetDevice for Igb {
    fn name(&self) -> &str {
        if self.variant == Variant::Igc {
            "igc"
        } else {
            "igb"
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
        d.write32(12, (frame.len() as u32) << 14); // PAYLEN, DD cleared
        d.write32(8, frame.len() as u32 | TX_DTYP_DATA | TX_EOP | TX_IFCS | TX_RS | TX_DEXT);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.tx_next = (self.tx_next + 1) % RING;
        self.regs.write32(TDT, self.tx_next as u32);
        platform::wait_until(1000, || d.read32(12) & DD != 0)
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let d = Mmio::new(self.rx_ring.as_ptr()).offset(16 * self.rx_next);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let status = d.read32(8);
        if status & DD == 0 {
            return None;
        }
        let len = d.read16(12) as usize;
        let n = len.min(buf.len());
        let off = self.rx_next * BUF;
        buf[..n].copy_from_slice(&self.rx_bufs.as_slice()[off..off + n]);
        // Hand the descriptor back: fresh read format.
        d.write64(0, self.rx_bufs.phys_at(off));
        d.write64(8, 0);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.regs.write32(RDT, self.rx_next as u32);
        self.rx_next = (self.rx_next + 1) % RING;
        Some(n)
    }
}
