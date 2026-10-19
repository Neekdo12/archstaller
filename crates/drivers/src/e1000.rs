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

const PBA: usize = 0x1000;
const PBS: usize = 0x1008;

const CTRL_ASDE: u32 = 1 << 5;
const CTRL_SLU: u32 = 1 << 6;
const CTRL_FRCSPD: u32 = 1 << 11;
const CTRL_FRCDPLX: u32 = 1 << 12;
const CTRL_RST: u32 = 1 << 26;
const CTRL_PHY_RST: u32 = 1 << 31;
const STATUS_LU: u32 = 1 << 1;

const RCTL_EN: u32 = 1 << 1;
const RCTL_UPE: u32 = 1 << 3;
const RCTL_MPE: u32 = 1 << 4;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_SECRC: u32 = 1 << 26;
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;

const RAH_AV: u32 = 1 << 31;
const TXDCTL_GRAN: u32 = 1 << 24;
const TXDCTL_COUNT_DESC: u32 = 1 << 22;
const TXDCTL_WTHRESH_1: u32 = 1 << 16;
const RXDCTL_QUEUE_ENABLE: u32 = 1 << 25;

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

/// Whether this driver claims the PCI id (`xtask coverage` counts recognized ids with it).
pub fn recognizes(vendor: u16, device: u16) -> bool {
    vendor == 0x8086 && family(device).is_some()
}

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.vendor != 0x8086 || family(dev.device).is_none() || dev.class != 0x02 {
        return;
    }
    let Some(regs) = dev.map_bar(0) else { return };
    dev.enable();
    if let Ok(d) = E1000::new(regs, dev.device) {
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
    fn new(regs: Mmio, device: u16) -> Result<E1000> {
        let e1000e = family(device) == Some(true);
        regs.write32(IMC, 0xffff_ffff);
        regs.write32(RCTL, 0);
        regs.write32(TCTL, TCTL_PSP);

        if is_ich_pbs_errata(device) {
            regs.write32(PBA, 0x08);
            regs.write32(PBS, 0x10);
        }

        regs.write32(CTRL, regs.read32(CTRL) | CTRL_RST);
        platform::delay_us(10_000);
        platform::wait_until(1000, || regs.read32(CTRL) & CTRL_RST == 0)?;
        regs.write32(IMC, 0xffff_ffff);
        regs.write32(CTRL, ctrl_link_config(regs.read32(CTRL)));

        let ral = regs.read32(RAL0);
        let rah = regs.read32(RAH0);
        let mac = parse_mac(ral, rah);
        if mac == [0; 6] {
            return Err(Error::Unsupported);
        }
        regs.write32(RAL0, ral);
        regs.write32(RAH0, rah0_address_valid(rah));
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
        // Force immediate transmit descriptor writeback: GRAN=1 (descriptors), WTHRESH=1.
        regs.write32(TXDCTL, txdctl_config());
        if e1000e {
            // Enable receive queue on controllers supporting RXDCTL.ENABLE (82574, ICH8..PCH).
            regs.write32(RXDCTL, regs.read32(RXDCTL) | RXDCTL_QUEUE_ENABLE);
            let _res = platform::wait_until(1, || regs.read32(RXDCTL) & RXDCTL_QUEUE_ENABLE != 0);
        }
        regs.write32(TIPG, 0x0060_200a);
        regs.write32(TCTL, TCTL_EN | TCTL_PSP | (0x0f << 4) | (0x40 << 12));
        regs.write32(RCTL, rctl_config());
        regs.write32(RDT, (RING - 1) as u32);

        Ok(E1000 {
            regs,
            rx_ring,
            tx_ring,
            rx_bufs,
            tx_buf,
            rx_next: 0,
            tx_next: 0,
            mac,
            e1000e,
        })
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
        let Some(dest) = self.tx_buf.as_mut_slice().get_mut(..frame.len()) else {
            return Err(Error::InvalidArgument);
        };
        dest.copy_from_slice(frame);
        let d = Mmio::new(self.tx_ring.as_ptr()).offset(16 * self.tx_next);
        d.write64(0, self.tx_buf.phys());
        d.write64(8, format_tx_descriptor(frame.len(), TX_CMD));
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.tx_next = (self.tx_next + 1) % RING;
        self.regs.write32(TDT, self.tx_next as u32);
        platform::wait_until(1000, || {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            d.read8(12) & DESC_DD != 0
        })
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let d = Mmio::new(self.rx_ring.as_ptr()).offset(16 * self.rx_next);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        if d.read8(12) & DESC_DD == 0 {
            return None;
        }
        let len = d.read16(8) as usize;
        let n = len.min(buf.len()).min(BUF);
        let off = self.rx_next * BUF;
        let res = if let (Some(dest), Some(src)) = (
            buf.get_mut(..n),
            self.rx_bufs.as_slice().get(off..off + n),
        ) {
            dest.copy_from_slice(src);
            Some(n)
        } else {
            None
        };
        d.write64(8, 0);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.regs.write32(RDT, self.rx_next as u32);
        self.rx_next = (self.rx_next + 1) % RING;
        res
    }
}

fn is_ich_pbs_errata(device: u16) -> bool {
    (0x1049..=0x104d).contains(&device)
        || device == 0x10c4
        || device == 0x10c5
        || device == 0x1501
}

fn ctrl_link_config(existing: u32) -> u32 {
    (existing | CTRL_SLU | CTRL_ASDE) & !(CTRL_PHY_RST | CTRL_FRCSPD | CTRL_FRCDPLX)
}

fn rctl_config() -> u32 {
    RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_SECRC
}

fn txdctl_config() -> u32 {
    TXDCTL_GRAN | TXDCTL_COUNT_DESC | TXDCTL_WTHRESH_1
}

fn format_tx_descriptor(len: usize, cmd: u8) -> u64 {
    ((cmd as u64) << 24) | (len as u64)
}

fn rah0_address_valid(rah: u32) -> u32 {
    (rah & 0x0000_ffff) | RAH_AV
}

fn parse_mac(ral: u32, rah: u32) -> [u8; 6] {
    [
        ral as u8,
        (ral >> 8) as u8,
        (ral >> 16) as u8,
        (ral >> 24) as u8,
        rah as u8,
        (rah >> 8) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recognizes_devices() {
        // Legacy e1000 (82540, 82545, etc.)
        assert!(recognizes(0x8086, 0x100e));
        assert!(recognizes(0x8086, 0x100f));
        assert_eq!(family(0x100e), Some(false));

        // e1000e (82574L, ICH8, ICH9, I217, etc.)
        assert!(recognizes(0x8086, 0x10d3));
        assert!(recognizes(0x8086, 0x1049)); // ICH8
        assert!(recognizes(0x8086, 0x104a)); // ICH8 82566DM
        assert!(recognizes(0x8086, 0x153a)); // I217
        assert_eq!(family(0x10d3), Some(true));
        assert_eq!(family(0x104a), Some(true));

        // Non-Intel vendor
        assert!(!recognizes(0x10ec, 0x100e));
        assert!(!recognizes(0x1022, 0x10d3));

        // Unknown device ID
        assert!(!recognizes(0x8086, 0x9999));
        assert_eq!(family(0x9999), None);
    }

    #[test]
    fn test_identifies_ich_pbs_errata_devices() {
        // 82566DM / 82566DC / 82562V / 82566MC
        assert!(is_ich_pbs_errata(0x1049));
        assert!(is_ich_pbs_errata(0x104a));
        assert!(is_ich_pbs_errata(0x104b));
        assert!(is_ich_pbs_errata(0x104c));
        assert!(is_ich_pbs_errata(0x104d));
        // 82562GT, 82562G, 82567V-3
        assert!(is_ich_pbs_errata(0x10c4));
        assert!(is_ich_pbs_errata(0x10c5));
        assert!(is_ich_pbs_errata(0x1501));

        // Non-errata devices
        assert!(!is_ich_pbs_errata(0x100e)); // 82540 legacy
        assert!(!is_ich_pbs_errata(0x10d3)); // 82574L
        assert!(!is_ich_pbs_errata(0x153a)); // I217
        assert!(!is_ich_pbs_errata(0x10c0)); // 82562V-2
    }

    #[test]
    fn test_configures_ctrl_link() {
        let initial = 0x0000_0241; // some existing control bits (e.g. FD, PRIO)
        let configured = ctrl_link_config(initial);
        // ASDE (bit 5) and SLU (bit 6) must be set
        assert_ne!(configured & (1 << 5), 0, "ASDE bit 5 must be set");
        assert_ne!(configured & (1 << 6), 0, "SLU bit 6 must be set");
        // Force speed (bit 11), force duplex (bit 12), and PHY_RST (bit 31) must be cleared
        assert_eq!(configured & (1 << 11), 0, "FRCSPD bit 11 must be cleared");
        assert_eq!(configured & (1 << 12), 0, "FRCDPLX bit 12 must be cleared");
        assert_eq!(configured & (1 << 31), 0, "PHY_RST bit 31 must be cleared");
        // Preserves unrelated bits (like bit 0)
        assert_ne!(configured & 1, 0, "unrelated bit 0 must be preserved");
    }

    #[test]
    fn test_configures_rctl_reception() {
        let rctl = rctl_config();
        assert_ne!(rctl & RCTL_EN, 0, "RCTL_EN bit must be set");
        assert_ne!(rctl & (1 << 3), 0, "RCTL_UPE (unicast promiscuous) must be set");
        assert_ne!(rctl & (1 << 4), 0, "RCTL_MPE (multicast promiscuous) must be set");
        assert_ne!(rctl & RCTL_BAM, 0, "RCTL_BAM must be set");
        assert_ne!(rctl & RCTL_SECRC, 0, "RCTL_SECRC must be set");
    }

    #[test]
    fn test_configures_txdctl_immediate_writeback() {
        let txdctl = txdctl_config();
        // GRAN = 1 (bit 24), COUNT_DESC = 1 (bit 22), and WTHRESH = 1 (bit 16) -> 0x0141_0000
        assert_eq!(txdctl & (1 << 24), 1 << 24, "GRAN bit 24 must be 1 (descriptor units)");
        assert_eq!(txdctl & (1 << 22), 1 << 22, "COUNT_DESC bit 22 must be 1");
        assert_eq!(txdctl & (0x3f << 16), 1 << 16, "WTHRESH bits [21:16] must be 1");
        // Bit 25 is LWTHRESH and should NOT be set as queue enable
        assert_eq!(txdctl & (1 << 25), 0, "Bit 25 must not be set in TXDCTL");
    }

    #[test]
    fn test_formats_tx_descriptor() {
        let desc = format_tx_descriptor(342, TX_CMD);
        assert_eq!(desc & 0xffff, 342, "length must be lower 16 bits");
        assert_eq!((desc >> 16) & 0xff, 0, "cso/flags must be zero");
        assert_eq!((desc >> 24) & 0xff, TX_CMD as u64, "command must match TX_CMD");
        assert_eq!(desc >> 32, 0, "status/css/special upper 32 bits must be zero");
    }

    #[test]
    fn test_configures_rah0_address_valid() {
        let raw_rah = 0x1234_5678; // upper bytes 0x5678, dirty upper bits 0x1234
        let validated = rah0_address_valid(raw_rah);
        assert_eq!(validated & 0xffff, 0x5678, "MAC upper 16 bits must be preserved");
        assert_eq!(validated & (1 << 31), 1 << 31, "RAH_AV bit 31 must be set");
        assert_eq!(validated & 0x7fff_0000, 0, "dirty bits must be masked off");
    }

    #[test]
    fn test_parses_mac_address() {
        let ral = 0x7856_3412; // bytes: 12, 34, 56, 78
        let rah = 0x8000_ba90; // bytes: 90, ba
        let mac = parse_mac(ral, rah);
        assert_eq!(mac, [0x12, 0x34, 0x56, 0x78, 0x90, 0xba]);
    }
}
