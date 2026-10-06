//! Chip revisions of the Realtek RTL8101/8102/8105/8106, RTL8168/8111 (all steppings) and
//! RTL8169/8110 families: which revision a TxConfig value is, and the per-revision start sequence.
//! The register sequences follow the Linux `r8169` driver (`rtl_hw_start_*`), without the PHY
//! firmware and the ASPM/EEE/wake-on-LAN parts that are not needed for a polled installer link.
//!
//! The revision numbers are Linux's `RTL_GIGA_MAC_VER_*`, so the two can be compared directly.
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::platform;

pub const CONFIG1: usize = 0x52;
pub const CONFIG2: usize = 0x53;
pub const CONFIG3: usize = 0x54;
pub const CONFIG4: usize = 0x55;
pub const CONFIG5: usize = 0x56;
pub const PHYAR: usize = 0x60;
const CSIDR: usize = 0x64;
const CSIAR: usize = 0x68;
pub const ERIDR: usize = 0x70;
pub const ERIAR: usize = 0x74;
const EPHYAR: usize = 0x80;
const OCPDR: usize = 0xb0;
const DLLPR: usize = 0xd0;
const DBG_REG: usize = 0xd1;
const MCU: usize = 0xd3;
pub const CPLUSCMD: usize = 0xe0;
pub const INTR_MITIGATE: usize = 0xe2;
pub const MAX_TX_PACKET: usize = 0xec; // EarlyTxThres on the 8169
/// "FuncEvent" and "MISC" are the same register.
const MISC: usize = 0xf0;
const MISC_1: usize = 0xf2;
const IBCR0: usize = 0xf8;
const IBCR2: usize = 0xf9;
const IBISR0: usize = 0xfb;
const COMBO_LTR_EXTEND: usize = 0xb6;

const BEACON_EN: u8 = 1 << 0;
const RDY_TO_L23: u8 = 1 << 1;
const SPI_EN: u8 = 1 << 3;
const ASPM_EN: u8 = 1 << 0;
const CLKREQ_EN: u8 = 1 << 7;
const SPEED_DOWN: u8 = 1 << 4;
const FIX_NAK_1: u8 = 1 << 4;
const FIX_NAK_2: u8 = 1 << 3;
const NOW_IS_OOB: u8 = 1 << 7;
const EN_NDP: u8 = 1 << 3;
const EN_OOB_RESET: u8 = 1 << 2;
const PFM_EN: u8 = 1 << 6;
const TX_10M_PS_EN: u8 = 1 << 7;
const PFM_D3COLD_EN: u8 = 1 << 6;
const TXPLA_RST: u32 = 1 << 29;
const DISABLE_LAN_EN: u32 = 1 << 23;
const PWM_EN: u32 = 1 << 22;
const RXDV_GATED_EN: u32 = 1 << 19;
const EARLY_TALLY_EN: u32 = 1 << 16;
const PCI_MUL_RW: u16 = 1 << 3;
const EN_ANA_PLL: u16 = 1 << 14;

/// The register window and the PCI function it belongs to.
pub struct Ctx<'a> {
    pub regs: &'a Mmio,
    pub dev: &'a PciDevice,
}

// --- Indirect register interfaces ---------------------------------------------------------------------

/// A GMII PHY register through the PHYAR window (not on the 8125/8126).
pub fn phy_read(regs: &Mmio, reg: u32) -> Option<u16> {
    regs.write32(PHYAR, reg << 16);
    platform::wait_until(10, || regs.read32(PHYAR) & (1 << 31) != 0).ok()?;
    Some(regs.read32(PHYAR) as u16)
}

pub fn phy_write(regs: &Mmio, reg: u32, val: u16) {
    regs.write32(PHYAR, 1 << 31 | reg << 16 | val as u32);
    let _ = platform::wait_until(10, || regs.read32(PHYAR) & (1 << 31) == 0);
}

/// Extended register interface (ERI): MAC-internal registers behind ERIAR/ERIDR.
pub fn eri_write(regs: &Mmio, addr: u32, mask: u32, val: u32) {
    regs.write32(ERIDR, val);
    regs.write32(ERIAR, 1 << 31 | mask << 12 | addr);
    let _ = platform::wait_until(10, || regs.read32(ERIAR) & (1 << 31) == 0);
}

pub fn eri_read(regs: &Mmio, addr: u32, mask: u32) -> u32 {
    regs.write32(ERIAR, mask << 12 | addr);
    let _ = platform::wait_until(10, || regs.read32(ERIAR) & (1 << 31) != 0);
    regs.read32(ERIDR)
}

/// Read-modify-write of an ERI register: `(old & !clear) | set`.
pub fn eri_update(regs: &Mmio, addr: u32, clear: u32, set: u32) {
    let v = eri_read(regs, addr, 0xf);
    eri_write(regs, addr, 0xf, (v & !clear) | set);
}

/// PCIe PHY (EPHY) registers.
pub fn ephy_read(regs: &Mmio, reg: u32) -> u16 {
    regs.write32(EPHYAR, (reg & 0x1f) << 16);
    let _ = platform::wait_until(10, || regs.read32(EPHYAR) & (1 << 31) != 0);
    regs.read32(EPHYAR) as u16
}

pub fn ephy_write(regs: &Mmio, reg: u32, val: u16) {
    regs.write32(EPHYAR, 1 << 31 | (reg & 0x1f) << 16 | val as u32);
    let _ = platform::wait_until(10, || regs.read32(EPHYAR) & (1 << 31) == 0);
}

/// Applies EPHY changes: (register, bits to clear, bits to set).
fn ephy_init(regs: &Mmio, table: &[(u32, u16, u16)]) {
    for &(reg, mask, bits) in table {
        let v = (ephy_read(regs, reg) & !mask) | bits;
        ephy_write(regs, reg, v);
    }
}

/// MAC OCP registers (RTL8168g and later).
fn ocp_read(regs: &Mmio, reg: u32) -> u16 {
    regs.write32(OCPDR, reg << 15);
    regs.read32(OCPDR) as u16
}

fn ocp_write(regs: &Mmio, reg: u32, data: u32) {
    regs.write32(OCPDR, 1 << 31 | reg << 15 | data);
}

fn ocp_modify(regs: &Mmio, reg: u32, mask: u16, set: u16) {
    let v = ocp_read(regs, reg);
    ocp_write(regs, reg, ((v & !mask) | set) as u32);
}

/// CSI: access to the device's own PCI configuration space, including the extended part that the
/// legacy port-I/O mechanism cannot reach.
fn csi_read(c: &Ctx, addr: u32) -> u32 {
    c.regs.write32(CSIAR, (addr & 0xfff) | (c.dev.addr.func as u32) << 16 | 0xf000);
    if platform::wait_until(5, || c.regs.read32(CSIAR) & (1 << 31) != 0).is_ok() {
        c.regs.read32(CSIDR)
    } else {
        !0
    }
}

fn csi_write(c: &Ctx, addr: u32, val: u32) {
    c.regs.write32(CSIDR, val);
    c.regs.write32(CSIAR, 1 << 31 | (addr & 0xfff) | 0xf000 | (c.dev.addr.func as u32) << 16);
    let _ = platform::wait_until(5, || c.regs.read32(CSIAR) & (1 << 31) == 0);
}

/// L0s/L1 entrance latency (config space byte 0x70f; Realtek says so).
fn set_aspm_entry_latency(c: &Ctx, val: u8) {
    let v = csi_read(c, 0x70c);
    csi_write(c, 0x70c, (v & 0x00ff_ffff) | (val as u32) << 24);
}

fn set_def_aspm_entry_latency(c: &Ctx) {
    set_aspm_entry_latency(c, 0x27); // L0 7 us, L1 16 us
}

fn clock_request(c: &Ctx, on: bool) {
    if let Some((off, _)) = c.dev.capabilities().into_iter().find(|&(_, id)| id == 0x10) {
        let v = c.dev.read32(off + 0x10);
        c.dev.write32(off + 0x10, if on { v | (1 << 4) } else { v & !(1 << 4) });
    }
}

fn mod8(regs: &Mmio, reg: usize, clear: u8, set: u8) {
    regs.write8(reg, (regs.read8(reg) & !clear) | set);
}

fn set_fifo_size(regs: &Mmio, rx_stat: u32, tx_stat: u32, rx_dyn: u32, tx_dyn: u32) {
    eri_write(regs, 0xc8, 0xf, (rx_stat << 16) | rx_dyn);
    eri_write(regs, 0xe8, 0xf, (tx_stat << 16) | tx_dyn);
}

fn pause_thresholds(regs: &Mmio, low: u32, high: u32) {
    eri_write(regs, 0xcc, 0x1, low);
    eri_write(regs, 0xd0, 0x1, high);
}

fn reset_packet_filter(regs: &Mmio) {
    eri_update(regs, 0xdc, 1, 0);
    eri_update(regs, 0xdc, 0, 1);
}

fn disable_rxdvgate(regs: &Mmio) {
    regs.write32(MISC, regs.read32(MISC) & !RXDV_GATED_EN);
}

fn l2l3_disable(regs: &Mmio) {
    mod8(regs, CONFIG3, RDY_TO_L23, 0);
}

fn phy_read_paged(regs: &Mmio, page: u32, reg: u32) -> u16 {
    phy_write(regs, 0x1f, page as u16);
    let v = phy_read(regs, reg).unwrap_or(0);
    phy_write(regs, 0x1f, 0);
    v
}

// --- Revision detection ---------------------------------------------------------------------------------

/// (mask, value, revision, name): first match of `(xid & mask) == value`, as in Linux's `rtl_chip_infos`.
const CHIPS: &[(u32, u32, u8, &str)] = &[
    (0x7cf, 0x6c9, 80, "RTL8127A"),
    (0x7cf, 0x64a, 70, "RTL8126A"),
    (0x7cf, 0x649, 70, "RTL8126A"),
    (0x7cf, 0x681, 66, "RTL8125BP"),
    (0x7cf, 0x708, 65, "RTL8125CP"),
    (0x7cf, 0x68b, 64, "RTL9151A"),
    (0x7cf, 0x68a, 64, "RTL8125K"),
    (0x7cf, 0x689, 64, "RTL8125D"),
    (0x7cf, 0x688, 64, "RTL8125D"),
    (0x7cf, 0x641, 63, "RTL8125B"),
    (0x7cf, 0x609, 61, "RTL8125A"),
    (0x7cf, 0x54b, 52, "RTL8168fp/RTL8117"),
    (0x7cf, 0x54a, 52, "RTL8168fp/RTL8117"),
    (0x7cf, 0x502, 51, "RTL8168ep/8111ep"),
    (0x7cf, 0x541, 46, "RTL8168h/8111h"),
    (0x7cf, 0x6c0, 46, "RTL8168M"),
    (0x7cf, 0x5c8, 44, "RTL8411b"),
    (0x7cf, 0x509, 42, "RTL8168gu/8111gu"),
    (0x7cf, 0x4c0, 40, "RTL8168g/8111g"),
    (0x7c8, 0x488, 38, "RTL8411"),
    (0x7cf, 0x481, 36, "RTL8168f/8111f"),
    (0x7cf, 0x480, 35, "RTL8168f/8111f"),
    (0x7c8, 0x2c8, 34, "RTL8168evl/8111evl"),
    (0x7cf, 0x2c1, 32, "RTL8168e/8111e"),
    (0x7c8, 0x2c0, 33, "RTL8168e/8111e"),
    (0x7cf, 0x281, 25, "RTL8168d/8111d"),
    (0x7c8, 0x280, 26, "RTL8168d/8111d"),
    (0x7cf, 0x28a, 28, "RTL8168dp/8111dp"),
    (0x7cf, 0x28b, 31, "RTL8168dp/8111dp"),
    (0x7cf, 0x3c9, 23, "RTL8168cp/8111cp"),
    (0x7cf, 0x3c8, 18, "RTL8168cp/8111cp"),
    (0x7c8, 0x3c8, 24, "RTL8168cp/8111cp"),
    (0x7cf, 0x3c0, 19, "RTL8168c/8111c"),
    (0x7cf, 0x3c2, 20, "RTL8168c/8111c"),
    (0x7cf, 0x3c3, 21, "RTL8168c/8111c"),
    (0x7c8, 0x3c0, 22, "RTL8168c/8111c"),
    (0x7c8, 0x380, 17, "RTL8168b/8111b"),
    (0x7c8, 0x448, 39, "RTL8106e"),
    (0x7c8, 0x440, 37, "RTL8402"),
    (0x7cf, 0x409, 29, "RTL8105e"),
    (0x7c8, 0x408, 30, "RTL8105e"),
    (0x7cf, 0x349, 8, "RTL8102e"),
    (0x7cf, 0x249, 8, "RTL8102e"),
    (0x7cf, 0x348, 7, "RTL8102e"),
    (0x7cf, 0x248, 7, "RTL8102e"),
    (0x7cf, 0x240, 14, "RTL8401"),
    (0x7c8, 0x348, 9, "RTL8102e/RTL8103e"),
    (0x7c8, 0x248, 9, "RTL8102e/RTL8103e"),
    (0x7c8, 0x340, 10, "RTL8101e/RTL8100e"),
    (0xfc8, 0x980, 6, "RTL8169sc/8110sc"),
    (0xfc8, 0x180, 5, "RTL8169sc/8110sc"),
    (0xfc8, 0x100, 4, "RTL8169sb/8110sb"),
    (0xfc8, 0x040, 3, "RTL8110s"),
    (0xfc8, 0x008, 2, "RTL8169s"),
];

/// `xid` is `(TxConfig >> 20) & 0xfcf`. Returns (revision, name), revision 0 if unknown.
pub fn identify(xid: u32) -> (u8, &'static str) {
    CHIPS.iter().find(|&&(m, v, _, _)| xid & m == v).map_or((0, "unknown"), |&(_, _, ver, name)| (ver, name))
}



/// RTL8168evl and later (not the RTL8106e): TxConfig auto FIFO, different early-transmit size.
pub fn evl_up(ver: u8) -> bool {
    (34..=52).contains(&ver) && ver != 39
}

/// RxConfig as Linux's `rtl_init_rxcfg` sets it for this revision.
pub fn rx_config(ver: u8) -> u32 {
    const FIFO_THRESH: u32 = 7 << 13;
    const BURST: u32 = 7 << 8;
    const RX128_INT_EN: u32 = 1 << 15;
    const MULTI_EN: u32 = 1 << 14;
    const EARLY_OFF: u32 = 1 << 11;
    match ver {
        2..=6 | 10..=17 => FIFO_THRESH | BURST,
        18..=24 | 34..=36 | 38 => RX128_INT_EN | MULTI_EN | BURST,
        40..=52 => RX128_INT_EN | MULTI_EN | BURST | EARLY_OFF,
        _ => RX128_INT_EN | BURST,
    }
}

/// TxConfig: unlimited DMA burst, shortest inter-frame gap, auto FIFO where the chip has it.
pub fn tx_config(ver: u8) -> u32 {
    0x0300_0700 | if evl_up(ver) { 1 << 7 } else { 0 }
}

/// Per-link tuning some revisions need whenever the link renegotiates (Linux's `rtl_link_chg_patch`).
/// `phystatus` is the PHYstatus register (bit 4: 1000, bit 3: 100).
pub fn link_changed(regs: &Mmio, ver: u8, phystatus: u8) {
    let (gig, hundred) = (phystatus & 0x10 != 0, phystatus & 0x08 != 0);
    match ver {
        34 | 38 => {
            let (bc, dc) = if gig {
                (0x11, 0x05)
            } else if hundred {
                (0x1f, 0x05)
            } else {
                (0x1f, 0x3f)
            };
            eri_write(regs, 0x1bc, 0xf, bc);
            eri_write(regs, 0x1dc, 0xf, dc);
            reset_packet_filter(regs);
        }
        35 | 36 => {
            let (bc, dc) = if gig { (0x11, 0x05) } else { (0x1f, 0x3f) };
            eri_write(regs, 0x1bc, 0xf, bc);
            eri_write(regs, 0x1dc, 0xf, dc);
        }
        37 => {
            if !gig && !hundred {
                eri_write(regs, 0x1d0, 0x3, 0x4d02);
                eri_write(regs, 0x1dc, 0x3, 0x0060a);
            } else {
                eri_write(regs, 0x1d0, 0x3, 0x0000);
            }
        }
        _ => {}
    }
}

/// Whether `link_changed` does anything for this revision.
pub fn needs_link_patch(ver: u8) -> bool {
    matches!(ver, 34..=38)
}

// --- Start sequences --------------------------------------------------------------------------------------

/// Linux's `rtl_hw_start` for everything except the 8125 generation: disables ASPM and clock request
/// first (the EPHY must be accessed with them off) and leaves them off, then runs the revision's sequence.
pub fn hw_start(c: &Ctx, ver: u8) {
    let r = c.regs;
    if ver >= 32 {
        mod8(r, CONFIG5, ASPM_EN, 0);
        mod8(r, CONFIG2, CLKREQ_EN, 0);
    }
    if ver <= 6 {
        hw_start_8169(c, ver);
    } else {
        r.write8(MAX_TX_PACKET, if evl_up(ver) { 0x27 } else { (8064 >> 7) as u8 });
        hw_config(c, ver);
        r.write16(INTR_MITIGATE, 0);
    }
    // Which events make the chip leave ASPM L1 (harmless with ASPM off, and Linux always sets them).
    match ver {
        34..=36 => eri_update(r, 0xd4, 0, 0x1f00),
        37 | 38 => eri_update(r, 0xd4, 0, 0x0c00),
        v if v >= 40 => ocp_modify(r, 0xc0ac, 0, 0x1f80),
        _ => {}
    }
}

fn hw_start_8169(c: &Ctx, ver: u8) {
    let r = c.regs;
    r.write8(MAX_TX_PACKET, 0x3f); // no early transmit
    let mut cp = (r.read16(CPLUSCMD) & 0x2000) | PCI_MUL_RW;
    if ver == 2 || ver == 3 {
        cp |= EN_ANA_PLL;
    }
    r.write16(CPLUSCMD, cp);
    if ver == 5 || ver == 6 {
        let mut v = if ver == 5 { 0x000f_ff00 } else { 0x00ff_ff00 };
        if r.read8(CONFIG2) & 1 != 0 {
            v |= 0xff;
        }
        r.write32(0x7c, v);
    }
    r.write16(INTR_MITIGATE, 0);
}

fn hw_config(c: &Ctx, ver: u8) {
    match ver {
        7 => hw_8102e_1(c),
        8 => {
            hw_8102e_2(c);
            ephy_write(c.regs, 0x03, 0xc2f9);
        }
        9 => hw_8102e_2(c),
        14 => {
            ephy_init(c.regs, &[(0x01, 0xffff, 0x6fe5), (0x03, 0xffff, 0x0599), (0x06, 0xffff, 0xaf25), (0x07, 0xffff, 0x8e68)]);
            mod8(c.regs, CONFIG3, BEACON_EN, 0);
        }
        17 => mod8(c.regs, CONFIG3, BEACON_EN, 0),
        18 => {
            set_def_aspm_entry_latency(c);
            ephy_init(c.regs, &[(0x01, 0, 0x0001), (0x02, 0x0800, 0x1000), (0x03, 0, 0x0042), (0x06, 0x0080, 0x0000), (0x07, 0, 0x2000)]);
            hw_8168cp_common(c);
        }
        19 => {
            set_def_aspm_entry_latency(c);
            c.regs.write8(DBG_REG, 0x06 | FIX_NAK_1 | FIX_NAK_2);
            ephy_init(c.regs, &[(0x02, 0x0800, 0x1000), (0x03, 0, 0x0002), (0x06, 0x0080, 0x0000)]);
            hw_8168cp_common(c);
        }
        20 | 21 => {
            set_def_aspm_entry_latency(c);
            ephy_init(c.regs, &[(0x01, 0, 0x0001), (0x03, 0x0400, 0x0020)]);
            hw_8168cp_common(c);
        }
        22 => {
            set_def_aspm_entry_latency(c);
            hw_8168cp_common(c);
        }
        23 => {
            set_def_aspm_entry_latency(c);
            mod8(c.regs, CONFIG3, BEACON_EN, 0);
        }
        24 => {
            set_def_aspm_entry_latency(c);
            mod8(c.regs, CONFIG3, BEACON_EN, 0);
            c.regs.write8(DBG_REG, 0x20);
        }
        25 | 26 | 31 => {
            set_def_aspm_entry_latency(c);
            clock_request(c, false);
        }
        28 => {
            set_def_aspm_entry_latency(c);
            ephy_init(c.regs, &[(0x0b, 0x0000, 0x0048), (0x19, 0x0020, 0x0050), (0x0c, 0x0100, 0x0020), (0x10, 0x0004, 0x0000)]);
            clock_request(c, true);
        }
        29 | 30 => {
            hw_8105e_1(c);
            if ver == 30 {
                ephy_write(c.regs, 0x1e, ephy_read(c.regs, 0x1e) | 0x8000);
            }
        }
        32 | 33 => hw_8168e_1(c),
        34 => hw_8168e_2(c),
        35 | 36 => hw_8168f_1(c),
        37 => hw_8402(c),
        38 => {
            hw_8168f(c);
            l2l3_disable(c.regs);
            ephy_init(c.regs, &[(0x06, 0x00c0, 0x0020), (0x0f, 0xffff, 0x5200), (0x19, 0x0000, 0x0224), (0x00, 0x0000, 0x0008), (0x0c, 0x3df0, 0x0200)]);
        }
        39 => hw_8106(c),
        40 => {
            hw_8168g(c);
            ephy_init(c.regs, &[(0x00, 0x0008, 0x0000), (0x0c, 0x3ff0, 0x0820), (0x1e, 0x0000, 0x0001), (0x19, 0x8000, 0x0000)]);
        }
        42 | 43 => {
            hw_8168g(c);
            ephy_init(
                c.regs,
                &[(0x00, 0x0008, 0x0000), (0x0c, 0x3ff0, 0x0820), (0x19, 0xffff, 0x7c00), (0x1e, 0xffff, 0x20eb), (0x0d, 0xffff, 0x1666), (0x00, 0xffff, 0x10a3), (0x06, 0xffff, 0xf050), (0x04, 0x0000, 0x0010), (0x1d, 0x4000, 0x0000)],
            );
        }
        44 => hw_8411_2(c),
        46 | 48 => hw_8168h_1(c),
        51 => hw_8168ep_3(c),
        52 => hw_8117(c),
        _ => {}
    }
}

fn hw_8168cp_common(c: &Ctx) {
    mod8(c.regs, CONFIG1, 0, SPEED_DOWN);
    mod8(c.regs, CONFIG3, BEACON_EN, 0);
    clock_request(c, false);
}

fn hw_8102e_1(c: &Ctx) {
    let r = c.regs;
    set_def_aspm_entry_latency(c);
    r.write8(DBG_REG, FIX_NAK_1);
    // LEDS1 | LEDS0 | Speed_down | MEMMAP | IOMAP | VPD | PMEnable
    r.write8(CONFIG1, 0x80 | 0x40 | SPEED_DOWN | 0x08 | 0x04 | 0x02 | 0x01);
    mod8(r, CONFIG3, BEACON_EN, 0);
    let cfg1 = r.read8(CONFIG1);
    if cfg1 & 0x40 != 0 && cfg1 & 0x80 != 0 {
        r.write8(CONFIG1, cfg1 & !0x40);
    }
    ephy_init(r, &[(0x01, 0, 0x6e65), (0x02, 0, 0x091f), (0x03, 0, 0xc2f9), (0x06, 0, 0xafb5), (0x07, 0, 0x0e00), (0x19, 0, 0xec80), (0x01, 0, 0x2e65), (0x01, 0, 0x6e65)]);
}

fn hw_8102e_2(c: &Ctx) {
    set_def_aspm_entry_latency(c);
    c.regs.write8(CONFIG1, 0x08 | 0x04 | 0x02 | 0x01); // MEMMAP | IOMAP | VPD | PMEnable
    mod8(c.regs, CONFIG3, BEACON_EN, 0);
}

fn hw_8105e_1(c: &Ctx) {
    let r = c.regs;
    r.write32(MISC, r.read32(MISC) | 0x002800); // FuncEvent: force LAN exit from ASPM if Rx/Tx are not idle
    r.write32(MISC, r.read32(MISC) & !0x010000); // disable the early tally counter
    mod8(r, MCU, 0, EN_NDP | EN_OOB_RESET);
    mod8(r, DLLPR, 0, PFM_EN);
    ephy_init(r, &[(0x07, 0, 0x4000), (0x19, 0, 0x0200), (0x19, 0, 0x0020), (0x1e, 0, 0x2000), (0x03, 0, 0x0001), (0x19, 0, 0x0100), (0x19, 0, 0x0004), (0x0a, 0, 0x0020)]);
    l2l3_disable(r);
}

fn hw_8168e_1(c: &Ctx) {
    let r = c.regs;
    set_def_aspm_entry_latency(c);
    ephy_init(
        r,
        &[(0x00, 0x0200, 0x0100), (0x00, 0x0000, 0x0004), (0x06, 0x0002, 0x0001), (0x06, 0x0000, 0x0030), (0x07, 0x0000, 0x2000), (0x00, 0x0000, 0x0020), (0x03, 0x5800, 0x2000), (0x03, 0x0000, 0x0001), (0x01, 0x0800, 0x1000), (0x07, 0x0000, 0x4000), (0x1e, 0x0000, 0x2000), (0x19, 0xffff, 0xfe6c), (0x0a, 0x0000, 0x0040)],
    );
    clock_request(c, false);
    r.write32(MISC, r.read32(MISC) | TXPLA_RST); // reset the tx FIFO pointer
    r.write32(MISC, r.read32(MISC) & !TXPLA_RST);
    mod8(r, CONFIG5, SPI_EN, 0);
}

fn hw_8168e_2(c: &Ctx) {
    let r = c.regs;
    set_def_aspm_entry_latency(c);
    ephy_init(r, &[(0x09, 0x0000, 0x0080), (0x19, 0x0000, 0x0224), (0x00, 0x0000, 0x0004), (0x0c, 0x3df0, 0x0200)]);
    eri_write(r, 0xc0, 0x3, 0x0000);
    eri_write(r, 0xb8, 0xf, 0x0000);
    set_fifo_size(r, 0x10, 0x10, 0x02, 0x06);
    eri_update(r, 0x1d0, 0, 1 << 1);
    reset_packet_filter(r);
    eri_update(r, 0x1b0, 0, 1 << 4);
    eri_write(r, 0xcc, 0xf, 0x0000_0050);
    eri_write(r, 0xd0, 0xf, 0x07ff_0060);
    clock_request(c, false);
    mod8(r, MCU, NOW_IS_OOB, 0);
    mod8(r, DLLPR, 0, PFM_EN);
    r.write32(MISC, r.read32(MISC) | PWM_EN);
    mod8(r, CONFIG5, SPI_EN, 0);
}

fn hw_8168f(c: &Ctx) {
    let r = c.regs;
    set_def_aspm_entry_latency(c);
    eri_write(r, 0xc0, 0x3, 0x0000);
    eri_write(r, 0xb8, 0xf, 0x0000);
    set_fifo_size(r, 0x10, 0x10, 0x02, 0x06);
    reset_packet_filter(r);
    eri_update(r, 0x1b0, 0, 1 << 4);
    eri_update(r, 0x1d0, 0, (1 << 4) | (1 << 1));
    eri_write(r, 0xcc, 0xf, 0x0000_0050);
    eri_write(r, 0xd0, 0xf, 0x0000_0060);
    clock_request(c, false);
    mod8(r, MCU, NOW_IS_OOB, 0);
    mod8(r, DLLPR, 0, PFM_EN);
    r.write32(MISC, r.read32(MISC) | PWM_EN);
    mod8(r, CONFIG5, SPI_EN, 0);
}

fn hw_8168f_1(c: &Ctx) {
    hw_8168f(c);
    ephy_init(c.regs, &[(0x06, 0x00c0, 0x0020), (0x08, 0x0001, 0x0002), (0x09, 0x0000, 0x0080), (0x19, 0x0000, 0x0224), (0x00, 0x0000, 0x0008), (0x0c, 0x3df0, 0x0200)]);
}

fn hw_8402(c: &Ctx) {
    let r = c.regs;
    set_def_aspm_entry_latency(c);
    r.write32(MISC, r.read32(MISC) | 0x002800);
    mod8(r, MCU, NOW_IS_OOB, 0);
    ephy_init(r, &[(0x19, 0xffff, 0xff64), (0x1e, 0, 0x4000)]);
    set_fifo_size(r, 0x00, 0x00, 0x02, 0x06);
    reset_packet_filter(r);
    eri_write(r, 0xc0, 0x3, 0x0000);
    eri_write(r, 0xb8, 0x3, 0x0000);
    let v = eri_read(r, 0x0d4, 0xf);
    eri_write(r, 0x0d4, 0xf, (v & !0xff00) | 0x0e00);
    eri_write(r, 0x1b0, 0x3, 0x0000); // disable EEE
    l2l3_disable(r);
}

fn hw_8106(c: &Ctx) {
    let r = c.regs;
    r.write32(MISC, r.read32(MISC) | 0x002800);
    r.write32(MISC, (r.read32(MISC) | DISABLE_LAN_EN) & !EARLY_TALLY_EN);
    mod8(r, MCU, 0, EN_NDP | EN_OOB_RESET);
    mod8(r, DLLPR, PFM_EN, 0);
    set_aspm_entry_latency(c, 0x2f);
    eri_write(r, 0x1d0, 0x3, 0x0000);
    eri_write(r, 0x1b0, 0x3, 0x0000);
    l2l3_disable(r);
}

fn hw_8168g(c: &Ctx) {
    let r = c.regs;
    set_fifo_size(r, 0x08, 0x10, 0x02, 0x06);
    pause_thresholds(r, 0x38, 0x48);
    set_def_aspm_entry_latency(c);
    reset_packet_filter(r);
    eri_write(r, 0x2f8, 0x3, 0x1d8f);
    disable_rxdvgate(r);
    eri_write(r, 0xc0, 0x3, 0x0000);
    eri_write(r, 0xb8, 0x3, 0x0000);
    let v = eri_read(r, 0x2fc, 0xf);
    eri_write(r, 0x2fc, 0xf, (v & !0x06) | 0x01);
    eri_update(r, 0x1b0, 1 << 12, 0);
    l2l3_disable(r);
}

/// Firmware image for the MAC of the RTL8411b, loaded into the MCU's RAM at 0xf800 (Linux's `rtl8411b_fix_phy_down`).
const FIX_PHY_DOWN: &[u16] = &[
    0xe008, 0xe00a, 0xe00c, 0xe00e, 0xe027, 0xe04f, 0xe05e, 0xe065, 0xc602, 0xbe00, 0x0000, 0xc502, 0xbd00, 0x074c, 0xc302, 0xbb00, 0x080a, 0x6420, 0x48c2, 0x8c20, 0xc516, 0x64a4, 0x49c0, 0xf009, 0x74a2, 0x8ca5, 0x74a0, 0xc50e, 0x9ca2, 0x1c11, 0x9ca0, 0xe006, 0x74f8, 0x48c4,
    0x8cf8, 0xc404, 0xbc00, 0xc403, 0xbc00, 0x0bf2, 0x0c0a, 0xe434, 0xd3c0, 0x49d9, 0xf01f, 0xc526, 0x64a5, 0x1400, 0xf007, 0x0c01, 0x8ca5, 0x1c15, 0xc51b, 0x9ca0, 0xe013, 0xc519, 0x74a0, 0x48c4, 0x8ca0, 0xc516, 0x74a4, 0x48c8, 0x48ca, 0x9ca4, 0xc512, 0x1b00, 0x9ba0, 0x1b1c,
    0x483f, 0x9ba2, 0x1b04, 0xc508, 0x9ba0, 0xc505, 0xbd00, 0xc502, 0xbd00, 0x0300, 0x051e, 0xe434, 0xe018, 0xe092, 0xde20, 0xd3c0, 0xc50f, 0x76a4, 0x49e3, 0xf007, 0x49c0, 0xf103, 0xc607, 0xbe00, 0xc606, 0xbe00, 0xc602, 0xbe00, 0x0c4c, 0x0c28, 0x0c2c, 0xdc00, 0xc707, 0x1d00,
    0x8de2, 0x48c1, 0xc502, 0xbd00, 0x00aa, 0xe0c0, 0xc502, 0xbd00, 0x0132,
];

fn hw_8411_2(c: &Ctx) {
    let r = c.regs;
    hw_8168g(c);
    ephy_init(
        r,
        &[(0x00, 0x0008, 0x0000), (0x0c, 0x37d0, 0x0820), (0x1e, 0x0000, 0x0001), (0x19, 0x8021, 0x0000), (0x1e, 0x0000, 0x2000), (0x0d, 0x0100, 0x0200), (0x00, 0x0000, 0x0080), (0x06, 0x0000, 0x0010), (0x04, 0x0000, 0x0010), (0x1d, 0x0000, 0x4000)],
    );
    // Realtek's fix for an RX unit that gets confused after the PHY was powered down.
    for reg in [0xfc28, 0xfc2a, 0xfc2c, 0xfc2e, 0xfc30, 0xfc32, 0xfc34, 0xfc36] {
        ocp_write(r, reg, 0);
    }
    platform::delay_us(3000);
    ocp_write(r, 0xfc26, 0);
    for (i, w) in FIX_PHY_DOWN.iter().enumerate() {
        ocp_write(r, 0xf800 + 2 * i as u32, *w as u32);
    }
    ocp_write(r, 0xfc26, 0x8000);
    for (reg, v) in [(0xfc2a, 0x0743), (0xfc2c, 0x0801), (0xfc2e, 0x0be9), (0xfc30, 0x02fd), (0xfc32, 0x0c25), (0xfc34, 0x00a9), (0xfc36, 0x012d)] {
        ocp_write(r, reg, v);
    }
}

/// The 8168h/8111h MAC settings that follow the EPHY setup (shared with the RTL8117).
fn hw_8168h_tail(c: &Ctx, extra_eri_d4: bool) {
    let r = c.regs;
    set_fifo_size(r, 0x08, 0x10, 0x02, 0x06);
    pause_thresholds(r, if extra_eri_d4 { 0x2f } else { 0x38 }, if extra_eri_d4 { 0x5f } else { 0x48 });
    set_def_aspm_entry_latency(c);
    reset_packet_filter(r);
    if extra_eri_d4 {
        eri_update(r, 0xd4, 0, 0x0010);
    } else {
        eri_update(r, 0xdc, 0, 0x001c);
    }
    eri_write(r, 0x5f0, 0x3, 0x4f87);
    disable_rxdvgate(r);
    eri_write(r, 0xc0, 0x3, 0x0000);
    eri_write(r, 0xb8, 0x3, 0x0000);
    mod8(r, DLLPR, PFM_EN, 0);
    mod8(r, MISC_1, PFM_D3COLD_EN, 0);
    mod8(r, DLLPR, TX_10M_PS_EN, 0);
    eri_update(r, 0x1b0, 1 << 12, 0);
    l2l3_disable(r);
    let saw = (phy_read_paged(r, 0x0c42, 0x13) & 0x3fff) as u32;
    if saw > 0 {
        ocp_modify(r, 0xd412, 0x0fff, ((16_000_000 / saw) & 0x0fff) as u16);
    }
}

fn hw_8168h_1(c: &Ctx) {
    let r = c.regs;
    ephy_init(r, &[(0x1e, 0x0800, 0x0001), (0x1d, 0x0000, 0x0800), (0x05, 0xffff, 0x2089), (0x06, 0xffff, 0x5881), (0x04, 0xffff, 0x854a), (0x01, 0xffff, 0x068b)]);
    hw_8168h_tail(c, false);
    ocp_modify(r, 0xe056, 0x00f0, 0x0000);
    ocp_modify(r, 0xe052, 0x6000, 0x8008);
    ocp_modify(r, 0xe0d6, 0x01ff, 0x017f);
    ocp_modify(r, 0xd420, 0x0fff, 0x047f);
    ocp_write(r, 0xe63e, 0x0001);
    ocp_write(r, 0xe63e, 0x0000);
    ocp_write(r, 0xc094, 0x0000);
    ocp_write(r, 0xc09e, 0x0000);
}

fn stop_cmac(r: &Mmio) {
    mod8(r, IBCR2, 0x01, 0);
    let _ = platform::wait_until(50, || r.read8(IBISR0) & 0x02 != 0);
    mod8(r, IBISR0, 0, 0x20);
    mod8(r, IBCR0, 0x01, 0);
}

fn hw_8168ep(c: &Ctx) {
    let r = c.regs;
    stop_cmac(r);
    set_fifo_size(r, 0x08, 0x10, 0x02, 0x06);
    pause_thresholds(r, 0x2f, 0x5f);
    set_def_aspm_entry_latency(c);
    reset_packet_filter(r);
    eri_write(r, 0x5f0, 0x3, 0x4f87);
    disable_rxdvgate(r);
    eri_write(r, 0xc0, 0x3, 0x0000);
    eri_write(r, 0xb8, 0x3, 0x0000);
    let v = eri_read(r, 0x2fc, 0xf);
    eri_write(r, 0x2fc, 0xf, (v & !0x06) | 0x01);
    mod8(r, DLLPR, TX_10M_PS_EN, 0);
    l2l3_disable(r);
}

fn hw_8168ep_3(c: &Ctx) {
    let r = c.regs;
    ephy_init(r, &[(0x00, 0x0000, 0x0080), (0x0d, 0x0100, 0x0200), (0x19, 0x8021, 0x0000), (0x1e, 0x0000, 0x2000)]);
    hw_8168ep(c);
    mod8(r, DLLPR, PFM_EN, 0);
    mod8(r, MISC_1, PFM_D3COLD_EN, 0);
    ocp_modify(r, 0xd3e2, 0x0fff, 0x0271);
    ocp_modify(r, 0xd3e4, 0x00ff, 0x0000);
    ocp_modify(r, 0xe860, 0x0000, 0x0080);
}

fn hw_8117(c: &Ctx) {
    let r = c.regs;
    stop_cmac(r);
    ephy_init(r, &[(0x19, 0x0040, 0x1100), (0x59, 0x0040, 0x1100)]);
    hw_8168h_tail(c, true);
    ocp_modify(r, 0xe056, 0x00f0, 0x0000);
    ocp_write(r, 0xea80, 0x0003);
    ocp_modify(r, 0xe052, 0x0000, 0x0009);
    ocp_modify(r, 0xd420, 0x0fff, 0x047f);
    ocp_write(r, 0xe63e, 0x0001);
    ocp_write(r, 0xe63e, 0x0000);
    ocp_write(r, 0xc094, 0x0000);
    ocp_write(r, 0xc09e, 0x0000);
}

/// Silences "unused" for registers kept for completeness of the table above.
#[allow(dead_code)]
const _UNUSED: (usize, usize, usize) = (CONFIG4, COMBO_LTR_EXTEND, PHYAR);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_common_chips() {
        assert_eq!(identify(0x2c8), (34, "RTL8168evl/8111evl")); // RTL8111evl, the one verified on hardware
        assert_eq!(identify(0x480).0, 35);
        assert_eq!(identify(0x4c0).0, 40); // RTL8111G
        assert_eq!(identify(0x541).0, 46); // RTL8111H
        assert_eq!(identify(0x2c1).0, 32);
        assert_eq!(identify(0x348).0, 7);
        assert_eq!(identify(0x340).0, 10);
        assert_eq!(identify(0x609).0, 61);
        assert_eq!(identify(0xfff).0, 0);
    }

    #[test]
    fn rx_and_tx_config_follow_the_revision() {
        assert_eq!(rx_config(34), 0xc700);
        assert_eq!(rx_config(40), 0xc700 | 0x800);
        assert_eq!(rx_config(17), 0xe700);
        assert_eq!(tx_config(34), 0x0300_0780);
        assert_eq!(tx_config(17), 0x0300_0700);
        assert!(evl_up(34) && evl_up(52) && !evl_up(39) && !evl_up(33));
    }
}
