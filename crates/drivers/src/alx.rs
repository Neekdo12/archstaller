//! Qualcomm Atheros / Attansic gigabit and fast Ethernet NICs, polled: one TX queue, one RX queue, no
//! checksum offload, no WoL, no ASPM. Two generations share one register map and descriptor format:
//!
//! * "alx": AR8161, AR8162, AR8171, AR8172, Killer E2200/E2400/E2500 (Linux's `alx`);
//! * "atl1c": AR8131, AR8132, AR8151, AR8152 (L1C, L2C, L1D, L2C B/B2, L1D 2.0; Linux's `atl1c`),
//!   very common on 2008-2014 desktop boards.
//!
//! UNTESTED on hardware (QEMU has no model of these chips). Written from the Linux drivers' reset,
//! PHY, queue and ring setup; the PHY is reset and put on auto-negotiation as they do.
use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::{platform, Devices};
use alloc::boxed::Box;
use hal::{Error, NetDevice, Result};

const VENDOR: u16 = 0x1969;
const ALX_IDS: &[u16] = &[0x1091, 0xe091, 0xe0a1, 0xe0b1, 0x1090, 0x10a1, 0x10a0];

/// The atl1c generation's chip types (Linux's `athr_*`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Chip {
    L1c,
    L2c,
    L2cB,
    L2cB2,
    L1d,
    L1d2,
}

const ATL1C_IDS: &[(u16, Chip)] = &[(0x1063, Chip::L1c), (0x1062, Chip::L2c), (0x2060, Chip::L2cB), (0x2062, Chip::L2cB2), (0x1073, Chip::L1d), (0x1083, Chip::L1d2)];

fn chip_of(device: u16) -> Option<Option<Chip>> {
    if ALX_IDS.contains(&device) {
        Some(None)
    } else {
        ATL1C_IDS.iter().find(|&&(d, _)| d == device).map(|&(_, c)| Some(c))
    }
}

const MSIX_MASK: usize = 0x0090;
const UE_SVRT: usize = 0x010c;
const EFLD: usize = 0x0204;
const SLD: usize = 0x0218;
const PDLL_TRNS1: usize = 0x1104;
const MASTER: usize = 0x1400;
const PHY_CTRL: usize = 0x140c;
const MAC_STS: usize = 0x1410;
const MDIO: usize = 0x1414;
const SERDES: usize = 0x1424;
const MDIO_EXTN: usize = 0x1448;
const WOL0: usize = 0x14a0;
const IDLE_DECISN_TIMER: usize = 0x1474;
const MAC_CTRL: usize = 0x1480;
const STAD0: usize = 0x1488;
const STAD1: usize = 0x148c;
const MTU: usize = 0x149c;
const SRAM5: usize = 0x1524;
const SRAM9: usize = 0x1534;
const RX_BASE_ADDR_HI: usize = 0x1540;
const TX_BASE_ADDR_HI: usize = 0x1544;
const RFD_ADDR_LO: usize = 0x1550;
const RFD_RING_SZ: usize = 0x1560;
const RFD_BUF_SZ: usize = 0x1564;
const RRD_ADDR_LO: usize = 0x1568;
const RRD_RING_SZ: usize = 0x1578;
const TPD_PRI0_ADDR_LO: usize = 0x1580;
const TPD_RING_SZ: usize = 0x1584;
const TXQ0: usize = 0x1590;
const TXQ1: usize = 0x1594;
const RXQ0: usize = 0x15a0;
const RXQ2: usize = 0x15a8;
const DMA: usize = 0x15c0;
const SMB_TIMER: usize = 0x15c4;
const TINT_TPD_THRSHLD: usize = 0x15c8;
const TINT_TIMER: usize = 0x15cc;
const RFD_PIDX: usize = 0x15e0;
const TPD_PRI0_PIDX: usize = 0x15f2;
const TPD_PRI0_CIDX: usize = 0x15f6;
const ISR: usize = 0x1600;
const IMR: usize = 0x1604;
const INT_RETRIG: usize = 0x1608;
const CLK_GATE: usize = 0x1814;
const WRR: usize = 0x1938;
const HQTPD: usize = 0x193c;
const MISC: usize = 0x19c0;
const MSIC2: usize = 0x19c8;
const MISC3: usize = 0x19cc;
const IRQ_MODU_TIMER: usize = 0x1408;

const MASTER_PCLKSEL_SRDS: u32 = 1 << 12;
const MASTER_OOB_DIS: u32 = 1 << 6;
const MASTER_WAKEN_25M: u32 = 1 << 5;
const MASTER_DMA_MAC_RST: u32 = 1;
const MASTER_IRQMOD: u32 = (1 << 11) | (1 << 10) | (1 << 7);
const PHY_CTRL_DSPRST_OUT: u32 = 1 << 0;
const PHY_CTRL_IDDQ: u32 = 1 << 7;
const PHY_CTRL_GATE_25M: u32 = 1 << 5;
const PHY_CTRL_POWER_DOWN: u32 = 1 << 14;
const PHY_CTRL_RST_ANALOG: u32 = 1 << 12;
const PHY_CTRL_HIB_PULSE: u32 = 1 << 11;
const PHY_CTRL_HIB_EN: u32 = 1 << 10;
const PHY_CTRL_CLS: u32 = (1 << 2) | (1 << 17) | (1 << 13); // LED_MODE | 100AB_EN | PLL_ON
const MAC_STS_IDLE: u32 = 0xf; // TXQ, RXQ, TXMAC, RXMAC busy
const MAC_CTRL_TX_EN: u32 = 1 << 0;
const MAC_CTRL_RX_EN: u32 = 1 << 1;
const MAC_CTRL_FULLD: u32 = 1 << 5;
const MAC_CTRL_MULTIALL_EN: u32 = 1 << 25;
const TXQ0_EN: u32 = 1 << 5;
const RXQ0_EN: u32 = 1 << 31;
const MISC_ISO_EN: u32 = 1 << 12;
const MISC_INTNLOSC_OPEN: u32 = 1 << 3;
const MISC3_25M_BY_SW: u32 = 1 << 1;
const MISC3_25M_NOTO_INTNL: u32 = 1 << 0;
const ISR_DIS: u32 = 1 << 31;

const RING: usize = 128;
const BUF: usize = 2048;

fn bit(n: u32) -> u32 {
    1 << n
}

/// Whether this driver claims the PCI id (`xtask coverage` counts recognized ids with it).
pub fn recognizes(vendor: u16, device: u16) -> bool {
    vendor == VENDOR && chip_of(device).is_some()
}

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.vendor != VENDOR {
        return;
    }
    let Some(chip) = chip_of(dev.device) else { return };
    let Some(regs) = dev.map_bar(0) else { return };
    dev.enable();
    match Alx::new(dev, regs, chip) {
        Ok(d) => out.net.push(Box::new(d)),
        Err(e) => hal::info!("alx: {:04x}:{:04x} not brought up ({e:?})", dev.vendor, dev.device),
    }
}

pub struct Alx {
    regs: Mmio,
    /// `Some` for the atl1c generation, `None` for alx.
    chip: Option<Chip>,
    giga: bool,
    rx_ctrl: u32,
    mac: [u8; 6],
    /// Transmit descriptors (16 bytes each), receive free descriptors (8) and return descriptors (16).
    tpd: Dma,
    rfd: Dma,
    rrd: Dma,
    bufs: Dma,
    tx_buf: Dma,
    tx_w: usize,
    rx_r: usize,
    rx_w: usize,
    rrd_r: usize,
    link_speed: u32, // 0: down
    link_t: u64,
}

fn spin_us(us: u64) {
    platform::delay_us(us);
}

impl Alx {
    fn r(&self, off: usize) -> u32 {
        self.regs.read32(off)
    }
    fn w(&self, off: usize, v: u32) {
        self.regs.write32(off, v)
    }

    // --- PHY (MDIO) -----------------------------------------------------------------------------------

    fn mdio_wait(&self) -> Result<()> {
        platform::wait_until(5, || self.r(MDIO) & bit(27) == 0)
    }

    fn clk_sel(&self) -> u32 {
        // alx: slow clock while the link is down (the PHY may be in hibernation), 25M/4 otherwise.
        // atl1c: always 25M/4 (the slow clock is only for the hibernating B2/D2 parts, which we never put there).
        if self.link_speed != 0 || self.chip.is_some() {
            0
        } else {
            7
        }
    }

    /// atl1c: the MAC polls the PHY on its own while MDIO_CTRL "auto polling" (bit 28) is on; it must be
    /// off for manual accesses, and stays off (the MAC speed is set by software).
    fn stop_polling(&self) {
        if self.chip.is_some() && self.r(MDIO) & bit(28) != 0 {
            self.w(MDIO, self.r(MDIO) & !bit(28));
            let _ = self.mdio_wait();
        }
    }

    fn phy_read(&self, reg: u32) -> Result<u16> {
        self.stop_polling();
        self.w(MDIO, bit(22) | self.clk_sel() << 24 | reg << 16 | bit(23) | bit(21));
        self.mdio_wait()?;
        Ok(self.r(MDIO) as u16)
    }

    fn phy_write(&self, reg: u32, val: u16) -> Result<()> {
        self.stop_polling();
        self.w(MDIO, bit(22) | self.clk_sel() << 24 | reg << 16 | (val as u32) << 0 | bit(23));
        self.mdio_wait()
    }

    fn phy_ext_write(&self, dev: u32, reg: u32, val: u16) -> Result<()> {
        self.stop_polling();
        self.w(MDIO_EXTN, dev << 16 | reg);
        self.w(MDIO, bit(22) | self.clk_sel() << 24 | (val as u32) | bit(23) | bit(30));
        self.mdio_wait()
    }

    fn phy_dbg_write(&self, reg: u16, val: u16) -> Result<()> {
        self.phy_write(0x1d, reg)?;
        self.phy_write(0x1e, val)
    }

    fn phy_dbg_read(&self, reg: u16) -> Result<u16> {
        self.phy_write(0x1d, reg)?;
        self.phy_read(0x1e)
    }

    // --- Reset ------------------------------------------------------------------------------------------

    fn stop_mac(&mut self) -> Result<()> {
        self.w(RXQ0, self.r(RXQ0) & !RXQ0_EN);
        self.w(TXQ0, self.r(TXQ0) & !TXQ0_EN);
        spin_us(40);
        self.rx_ctrl &= !(MAC_CTRL_RX_EN | MAC_CTRL_TX_EN);
        self.w(MAC_CTRL, self.rx_ctrl);
        for _ in 0..50 {
            if self.r(MAC_STS) & MAC_STS_IDLE == 0 {
                return Ok(());
            }
            spin_us(10);
        }
        Err(Error::Timeout)
    }

    fn reset_osc(&self, rev: u8) {
        let v = self.r(MISC3);
        self.w(MISC3, (v & !MISC3_25M_BY_SW) | MISC3_25M_NOTO_INTNL);
        let mut val = self.r(MISC);
        if rev >= 2 {
            val = (val & !(7 << 21)) | (7 << 21); // over-current protection default
            val &= !MISC_INTNLOSC_OPEN;
            self.w(MISC, val);
            self.w(MISC, val | MISC_INTNLOSC_OPEN);
            let v2 = self.r(MSIC2) & !(1 << 6);
            self.w(MSIC2, v2);
            self.w(MSIC2, v2 | (1 << 6)); // calibration start
        } else {
            val &= !MISC_INTNLOSC_OPEN;
            if rev <= 1 {
                val &= !MISC_ISO_EN;
            }
            self.w(MISC, val | MISC_INTNLOSC_OPEN);
            self.w(MISC, val);
        }
        spin_us(20);
    }

    fn reset_mac(&mut self, rev: u8) -> Result<()> {
        self.w(MSIX_MASK, 0xffff_ffff);
        self.w(IMR, 0);
        self.w(ISR, ISR_DIS);
        self.stop_mac()?;
        self.w(RFD_PIDX, 1); // workaround from the vendor driver
        let m = self.r(MASTER);
        self.w(MASTER, m | MASTER_DMA_MAC_RST | MASTER_OOB_DIS);
        spin_us(10);
        let mut i = 0;
        while i < 50 {
            if self.r(RFD_PIDX) == 0 {
                break;
            }
            spin_us(10);
            i += 1;
        }
        while i < 50 {
            if self.r(MASTER) & MASTER_DMA_MAC_RST == 0 {
                break;
            }
            spin_us(10);
            i += 1;
        }
        if i == 50 {
            return Err(Error::Timeout);
        }
        spin_us(10);
        self.reset_osc(rev);
        let v = self.r(MISC3);
        self.w(MISC3, (v & !MISC3_25M_BY_SW) | MISC3_25M_NOTO_INTNL);
        let mut v = self.r(MISC) & !MISC_INTNLOSC_OPEN;
        if rev <= 1 {
            v &= !MISC_ISO_EN;
        }
        self.w(MISC, v);
        spin_us(20);
        self.w(MAC_CTRL, self.rx_ctrl);
        let s = self.r(SERDES);
        self.w(SERDES, s | (1 << 17) | (1 << 18));
        Ok(())
    }

    fn reset_pcie(&self, dev: &PciDevice, rev: u8, with_cr: bool) {
        let cmd = dev.command();
        if cmd & 7 != 7 || cmd & (1 << 10) != 0 {
            dev.set_command((cmd | 7) & !(1 << 10));
        }
        self.w(WOL0, 0);
        self.w(PDLL_TRNS1, self.r(PDLL_TRNS1) & !(1 << 11)); // D3PLLOFF_EN
        self.w(UE_SVRT, self.r(UE_SVRT) & !((1 << 13) | (1 << 4))); // FC / DL protocol errors
        let m = self.r(MASTER);
        if rev <= 1 && with_cr {
            if m & MASTER_WAKEN_25M == 0 || m & MASTER_PCLKSEL_SRDS == 0 {
                self.w(MASTER, m | MASTER_PCLKSEL_SRDS | MASTER_WAKEN_25M);
            }
        } else if m & MASTER_WAKEN_25M == 0 || m & MASTER_PCLKSEL_SRDS != 0 {
            self.w(MASTER, (m & !MASTER_PCLKSEL_SRDS) | MASTER_WAKEN_25M);
        }
        spin_us(10);
    }

    fn reset_phy(&self) {
        let mut v = self.r(PHY_CTRL);
        v &= !(PHY_CTRL_DSPRST_OUT | PHY_CTRL_IDDQ | PHY_CTRL_GATE_25M | PHY_CTRL_POWER_DOWN | PHY_CTRL_CLS);
        v |= PHY_CTRL_RST_ANALOG | PHY_CTRL_HIB_PULSE | PHY_CTRL_HIB_EN;
        self.w(PHY_CTRL, v);
        spin_us(10);
        self.w(PHY_CTRL, v | PHY_CTRL_DSPRST_OUT);
        spin_us(800); // the DSP needs about 800 us (80 x 10 us in Linux)
        // Power saving and tuning, values from the vendor driver.
        let _ = self.phy_dbg_write(0x29, 0x129d); // legacy power saving
        let _ = self.phy_dbg_write(0x04, 0xbb8b); // system mode control
        let _ = self.phy_ext_write(3, 0x8062, 0x3); // VDRVBIAS
        self.w(0x1440, self.r(0x1440) & !(1 << 0)); // LPI_CTRL: EEE off
        let _ = self.phy_ext_write(7, 0x3c, 0); // no EEE advertisement
        let _ = self.phy_dbg_write(0x12, 0x4c04);
        let _ = self.phy_dbg_write(0x05, 0x2c46);
        let _ = self.phy_dbg_write(0x36, 0xe12c);
        let _ = self.phy_dbg_write(0x00, 0x02ef);
        if let Ok(g) = self.phy_dbg_read(0x3d) {
            let _ = self.phy_dbg_write(0x3d, g & !0x0080);
        }
        let _ = self.phy_ext_write(7, 0x8027, 0x8a05);
        let _ = self.phy_ext_write(7, 0x8023, 0);
        let _ = self.phy_write(0x12, 0x0400 | 0x0800); // link up / down interrupt mask
    }

    /// Auto-negotiation advertising 10/100/1000, full and half duplex, with the PHY reset first.
    fn setup_autoneg(&self) -> Result<()> {
        self.phy_write(0x1d, 0)?;
        self.phy_write(4, 0x01e1 | 0x0400 | 0x0800)?; // CSMA, 10/100 HD+FD, pause
        self.phy_write(9, if self.giga { 0x0200 } else { 0 })?; // 1000 full
        self.phy_write(0, 0x8000 | 0x1000 | 0x0200)?; // reset, enable and restart auto-negotiation
        self.phy_write(0x1d, 0x003f) // "PHY initialized" marker
    }

    // --- MAC address -----------------------------------------------------------------------------------

    fn read_mac(&self) -> Option<[u8; 6]> {
        let (m0, m1) = (self.r(STAD0), self.r(STAD1));
        let a = [(m1 >> 8) as u8, m1 as u8, (m0 >> 24) as u8, (m0 >> 16) as u8, (m0 >> 8) as u8, m0 as u8];
        (a != [0; 6] && a != [0xff; 6] && a[0] & 1 == 0).then_some(a)
    }

    /// The permanent address: the register (loaded by the BIOS/EFUSE), else the efuse or flash loader.
    fn perm_mac(&self) -> Option<[u8; 6]> {
        if let Some(a) = self.read_mac() {
            return Some(a);
        }
        let wait = |reg: usize, bits: u32| -> Option<u32> {
            for _ in 0..1000 {
                let v = self.r(reg);
                if v & bits == 0 {
                    return Some(v);
                }
                platform::delay_us(1000);
            }
            None
        };
        let v = wait(SLD, (1 << 12) | (1 << 11))?; // SLD_STAT | SLD_START
        self.w(SLD, v | (1 << 11));
        wait(SLD, 1 << 11)?;
        if let Some(a) = self.read_mac() {
            return Some(a);
        }
        if self.r(EFLD) & ((1 << 10) | (1 << 9)) != 0 {
            let v = wait(EFLD, (1 << 5) | 1)?; // EFLD_STAT | EFLD_START
            self.w(EFLD, v | 1);
            wait(EFLD, 1)?;
            return self.read_mac();
        }
        None
    }

    // --- Configuration ----------------------------------------------------------------------------------

    fn configure_basic(&self, rev: u8) {
        let m = self.mac;
        self.w(STAD0, u32::from_be_bytes([m[2], m[3], m[4], m[5]]));
        self.w(STAD1, u32::from_be_bytes([0, 0, m[0], m[1]]));
        self.w(CLK_GATE, 0x3f);
        if rev >= 2 {
            self.w(IDLE_DECISN_TIMER, 0x400);
        }
        self.w(SMB_TIMER, 400 * 500);
        self.w(MASTER, self.r(MASTER) | MASTER_IRQMOD);
        self.w(IRQ_MODU_TIMER, 200 >> 1);
        self.w(INT_RETRIG, 20_000);
        self.w(TINT_TPD_THRSHLD, (RING as u32) / 3);
        self.w(TINT_TIMER, 200);
        let raw_mtu = 1500 + 14 + 4 + 4;
        self.w(MTU, raw_mtu);
        self.w(TXQ1, ((raw_mtu + 7) >> 3) | (1 << 11));
        // Max read request size: keep what the BIOS set unless it is below 256 bytes.
        let max_payload = 2u32; // read request size field: 512 bytes, the minimum Linux enforces
        self.w(TXQ0, 5 | (1 << 6) | (1 << 7) | (1 << 4) | (0x200 << 16)); // burst pref, enhance, LSO 802.3, IP options
        self.w(HQTPD, 5 << 0 | 5 << 4 | 5 << 8 | (1 << 31));
        let sram = (self.r(SRAM5) & 0xfff) << 3;
        let (xoff, xon) = if sram > 8 * 1024 { (1536 >> 3, (sram - 3212) >> 3) } else { (1536 >> 3, (sram - 1536) >> 3) };
        self.w(RXQ2, xoff << 16 | xon);
        let mut rxq = 8 << 20 | (0x100 << 8) | (1 << 7) | (0xf << 2) | (1 << 29);
        if self.giga {
            rxq |= 3; // ASPM threshold for 100M
        }
        self.w(RXQ0, rxq & !(1 << 29)); // RSS off
        let chnl = if rev >= 2 { 4 } else { 2 };
        self.w(DMA, 4 | (1 << 10) | max_payload << 4 | 4 << 16 | 15 << 11 | (chnl - 1) << 26);
        self.w(WRR, 3 << 29 | 4 | 4 << 8 | 4 << 16 | 4 << 24);
    }


    // --- atl1c generation ------------------------------------------------------------------------------

    fn reset_mac_c(&mut self, chip: Chip) -> Result<()> {
        self.stop_mac()?;
        let m = self.r(MASTER) | MASTER_OOB_DIS;
        self.w(MASTER, m | MASTER_DMA_MAC_RST); // soft reset
        spin_us(10_000);
        let mut idle = false;
        for _ in 0..100 {
            if self.r(MAC_STS) & MAC_STS_IDLE == 0 {
                idle = true;
                break;
            }
            spin_us(100);
        }
        if !idle {
            return Err(Error::Timeout);
        }
        self.w(MASTER, m);
        self.w(MAC_CTRL, self.r(MAC_CTRL) | (1 << 30)); // speed/duplex set by software
        let sd = self.r(SERDES);
        match chip {
            Chip::L2cB => self.w(SERDES, sd & !((1 << 18) | (1 << 17))),
            Chip::L2cB2 | Chip::L1d2 => self.w(SERDES, sd | (1 << 18) | (1 << 17)),
            _ => {}
        }
        Ok(())
    }

    fn reset_phy_c(&self, chip: Chip) {
        let mut v = self.r(PHY_CTRL);
        v &= !(PHY_CTRL_DSPRST_OUT | PHY_CTRL_IDDQ | PHY_CTRL_GATE_25M | PHY_CTRL_POWER_DOWN | PHY_CTRL_CLS);
        v |= PHY_CTRL_RST_ANALOG | PHY_CTRL_HIB_EN | PHY_CTRL_HIB_PULSE;
        self.w(PHY_CTRL, v);
        spin_us(10);
        self.w(PHY_CTRL, v | PHY_CTRL_DSPRST_OUT);
        spin_us(800);
        let b = matches!(chip, Chip::L2cB | Chip::L2cB2);
        if chip == Chip::L2cB {
            if let Ok(d) = self.phy_dbg_read(0x0a) {
                let _ = self.phy_dbg_write(0x0a, d & !0x2000);
            }
        }
        if b {
            if let Ok(d) = self.phy_dbg_read(0x3e) {
                let _ = self.phy_dbg_write(0x3e, d | 0x8000); // tx half amplitude fix
            }
            if let Ok(d) = self.phy_dbg_read(0x3b) {
                let _ = self.phy_dbg_write(0x3b, d & !0x0008); // lower voltage
            }
        }
        let _ = self.phy_dbg_write(0x29, if matches!(chip, Chip::L1d | Chip::L1d2) { 0x129d } else { 0x36dd }); // power saving
        let _ = self.phy_dbg_write(0x04, 0x88bb); // hibernate
        if matches!(chip, Chip::L1d | Chip::L1d2 | Chip::L2cB2) {
            self.w(0x1440, self.r(0x1440) & !1); // LPI_CTRL: EEE off
            let _ = self.phy_ext_write(7, 0x3c, 0);
            let _ = self.phy_ext_write(3, 0x8003, 0x4d19);
        }
        let _ = self.phy_dbg_write(0x00, 0x02ef);
        let _ = self.phy_dbg_write(0x05, 0x2c46);
        let _ = self.phy_dbg_write(0x12, 0x4c04);
        let _ = self.phy_dbg_write(0x36, 0xe12c | 0x0080);
        let _ = self.phy_write(0x12, 0x0400 | 0x0800);
    }

    fn configure_c(&self, chip: Chip) {
        self.w(ISR, 0xffff_ffff);
        self.w(WOL0, 0);
        self.w(CLK_GATE, if chip == Chip::L2cB { 0x3f & !0x20 } else { 0x3f });
        self.w(INT_RETRIG, 50_000);
        // Descriptor rings were programmed by init_rings; the B chips also need their SRAM carved up.
        if chip == Chip::L2cB {
            self.w(0x1524, 0x02a0);
            self.w(0x152c, 0x0100);
            self.w(0x1520, 0x029f_0000);
            self.w(0x1500, 0x02bf_02a0);
            self.w(0x1528, 0x03bf_02c0);
            self.w(0x1518, 0x03df_03c0);
            self.w(0x1598, 0);
            self.w(0x15ac, 0);
        }
        let m = self.r(MASTER) & !((1 << 11) | (1 << 10) | (1 << 14));
        self.w(MASTER, m | (1 << 7)); // system alive timer
        self.w(SMB_TIMER, 200_000);
        self.w(MTU, 1500 + 14 + 4 + 4);
        self.w(TXQ1, ((7 * 1024) >> 3) & 0x7ff); // TSO offload threshold
        let burst: u32 = if matches!(chip, Chip::L2cB | Chip::L2cB2) { 0x40 } else { 0x200 };
        self.w(TXQ0, 5 | (1 << 6) | (1 << 7) | (1 << 4) | burst << 16);
        let mut rxq = 8u32 << 20;
        if self.giga && chip != Chip::L1d2 {
            rxq |= 3; // ASPM throughput limit 100M
        }
        self.w(RXQ0, rxq);
        self.w(DMA, 4 | (1 << 10) | 2 << 4 | 4 << 16 | 15 << 11);
    }

    fn init_rings(&mut self) -> Result<()> {
        let hi = (self.tpd.phys() >> 32) as u32;
        for p in [self.tpd.phys(), self.rfd.phys(), self.rrd.phys(), self.tpd.phys() + (RING * 16) as u64, self.rfd.phys() + (RING * 8) as u64, self.rrd.phys() + (RING * 16) as u64] {
            if (p >> 32) as u32 != hi {
                return Err(Error::Unsupported); // one shared high address register for all descriptors
            }
        }
        self.w(TPD_PRI0_ADDR_LO, self.tpd.phys() as u32);
        self.w(RRD_ADDR_LO, self.rrd.phys() as u32);
        self.w(RFD_ADDR_LO, self.rfd.phys() as u32);
        self.w(TX_BASE_ADDR_HI, hi);
        self.w(TPD_RING_SZ, RING as u32);
        self.w(RX_BASE_ADDR_HI, hi);
        self.w(RRD_RING_SZ, RING as u32);
        self.w(RFD_RING_SZ, RING as u32);
        self.w(RFD_BUF_SZ, BUF as u32);
        self.w(SRAM9, 1); // load the pointers into the chip
        // Arm all but one receive slot.
        let rfd = Mmio::new(self.rfd.as_ptr());
        for i in 0..RING {
            rfd.write64(8 * i, self.bufs.phys_at(i * BUF));
        }
        self.rx_r = 0;
        self.rx_w = RING - 1;
        self.rrd_r = 0;
        self.tx_w = 0;
        self.regs.write16(RFD_PIDX, self.rx_w as u16);
        Ok(())
    }

    fn new(dev: &PciDevice, regs: Mmio, chip: Option<Chip>) -> Result<Alx> {
        let revision = dev.revision;
        let rev = revision >> 3;
        if chip.is_none() && rev > 3 {
            return Err(Error::Unsupported);
        }
        let mut a = Alx {
            regs,
            chip,
            giga: dev.device & 1 != 0,
            // Linux's default MAC control: wake-speed switching (alx) or software speed (atl1c), hash,
            // broadcast, pad+CRC, flow control, preamble length 7.
            rx_ctrl: (1 << 30) | (1 << 29) | (1 << 26) | (1 << 7) | (1 << 6) | (1 << 3) | (1 << 2) | (7 << 10) | if chip.is_some() { 1 << 28 } else { 0 },
            mac: [0; 6],
            tpd: Dma::new(RING * 16, 64),
            rfd: Dma::new(RING * 8, 64),
            rrd: Dma::new(RING * 16, 64),
            bufs: Dma::new(RING * BUF, 4096),
            tx_buf: Dma::new(BUF, 64),
            tx_w: 0,
            rx_r: 0,
            rx_w: 0,
            rrd_r: 0,
            link_speed: 0,
            link_t: 0,
        };
        a.mac = a.perm_mac().ok_or(Error::Unsupported)?;
        a.rx_ctrl |= MAC_CTRL_MULTIALL_EN; // every multicast frame (the stack filters)
        match chip {
            None => {
                a.reset_pcie(dev, rev, revision & 1 != 0);
                a.reset_mac(rev)?;
                a.reset_phy();
                a.setup_autoneg()?;
                a.configure_basic(rev);
                a.init_rings()?;
            }
            Some(c) => {
                // atl1c: the MAC address registers hold the BIOS-loaded address; the MAC takes its
                // address and the rings after the reset like alx does.
                a.reset_phy_c(c);
                a.reset_mac_c(c)?;
                a.setup_autoneg()?;
                let m = a.mac;
                a.w(STAD0, u32::from_be_bytes([m[2], m[3], m[4], m[5]]));
                a.w(STAD1, u32::from_be_bytes([0, 0, m[0], m[1]]));
                a.init_rings()?;
                a.configure_c(c);
            }
        }
        a.w(MAC_CTRL, a.rx_ctrl);
        hal::log!("alx: {:04x}:{:04x} rev {:#04x} ({}), mac {:02x?}", dev.vendor, dev.device, revision, chip.map_or("alx".into(), |c| alloc::format!("atl1c {c:?}")), a.mac);
        // Wait for a link (auto-negotiation takes a second or two).
        let _ = platform::wait_until(5000, || a.poll_link());
        Ok(a)
    }

    /// Reads the link state and, on a change, starts or stops the MAC with the negotiated speed/duplex.
    fn poll_link(&mut self) -> bool {
        // The link bit is latched low: read twice.
        let _ = self.phy_read(1);
        let up = self.phy_read(1).map_or(false, |b| b & 0x0004 != 0);
        if !up {
            if self.link_speed != 0 {
                self.link_speed = 0;
                let _ = self.stop_mac();
            }
            return false;
        }
        if self.link_speed == 0 {
            let Ok(pssr) = self.phy_read(0x11) else { return false };
            if pssr & 0x0800 == 0 {
                return false; // speed/duplex not resolved yet
            }
            let speed = match pssr & 0xc000 {
                0x8000 => 1000,
                0x4000 => 100,
                _ => 10,
            };
            let full = pssr & 0x2000 != 0;
            self.link_speed = speed;
            self.w(RXQ0, self.r(RXQ0) | RXQ0_EN);
            self.w(TXQ0, self.r(TXQ0) | TXQ0_EN);
            let mut mac = self.rx_ctrl & !(MAC_CTRL_FULLD | (3 << 20));
            if full {
                mac |= MAC_CTRL_FULLD;
            }
            mac |= (if speed == 1000 { 2 } else { 1 }) << 20;
            mac |= MAC_CTRL_TX_EN | MAC_CTRL_RX_EN;
            self.rx_ctrl = mac;
            self.w(MAC_CTRL, mac);
            hal::log!("alx: link up at {speed} Mbit/s {}", if full { "full duplex" } else { "half duplex" });
        }
        true
    }
}

impl NetDevice for Alx {
    fn name(&self) -> &str {
        if self.chip.is_some() {
            "atl1c"
        } else {
            "alx"
        }
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        let now = platform::now_ns();
        if now.wrapping_sub(self.link_t) > 100_000_000 || self.link_speed == 0 {
            self.link_t = now;
            return self.poll_link();
        }
        true
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() > BUF || frame.is_empty() {
            return Err(Error::InvalidArgument);
        }
        self.tx_buf.as_mut_slice()[..frame.len()].copy_from_slice(frame);
        let d = Mmio::new(self.tpd.as_ptr()).offset(16 * self.tx_w);
        d.write16(0, frame.len() as u16);
        d.write16(2, 0);
        d.write32(4, 1 << 31); // end of packet
        d.write64(8, self.tx_buf.phys());
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.tx_w = (self.tx_w + 1) % RING;
        self.regs.write16(TPD_PRI0_PIDX, self.tx_w as u16);
        let want = self.tx_w as u16;
        platform::wait_until(1000, || self.regs.read16(TPD_PRI0_CIDX) == want)
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let rrd = Mmio::new(self.rrd.as_ptr()).offset(16 * self.rrd_r);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let w3 = rrd.read32(12);
        if w3 & (1 << 31) == 0 {
            return None; // nothing "updated"
        }
        rrd.write32(12, w3 & !(1 << 31));
        let w0 = rrd.read32(0);
        let (si, nor) = ((w0 >> 20) & 0xfff, (w0 >> 16) & 0xf);
        let slot = self.rx_r;
        let ok = si as usize == slot && nor == 1 && w3 & ((1 << 20) | (1 << 21)) == 0; // no RES / FCS error
        let len = ((w3 & 0x3fff) as usize).saturating_sub(4);
        let n = len.min(buf.len());
        if ok {
            buf[..n].copy_from_slice(&self.bufs.as_slice()[slot * BUF..slot * BUF + n]);
        }
        // Give the slot back: the descriptor already points at its buffer, so only the producer index moves.
        self.rrd_r = (self.rrd_r + 1) % RING;
        self.rx_r = (self.rx_r + 1) % RING;
        self.rx_w = (self.rx_w + 1) % RING;
        self.regs.write16(RFD_PIDX, self.rx_w as u16);
        ok.then_some(n)
    }
}
