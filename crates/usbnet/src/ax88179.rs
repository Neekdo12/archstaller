//! ASIX AX88179 USB 3.0 gigabit Ethernet (vendor-specific protocol, USB configuration 1).
//! Register access is vendor requests with the register address in wValue and the byte count
//! in wIndex; frames carry an 8-byte header on transmit and are batched with a trailing packet
//! table on receive. Written from the layout the Linux `ax88179_178a` driver documents.
use alloc::vec::Vec;

/// Vendor request: MAC register access (reads and writes).
pub const ACCESS_MAC: u8 = 0x01;
/// Vendor request: PHY access (wValue = PHY id, wIndex = register).
pub const ACCESS_PHY: u8 = 0x02;
pub const PHY_ID: u16 = 0x03;
pub const PHY_BMCR: u16 = 0x00;
pub const PHY_PHYSR: u16 = 0x11;

pub const NODE_ID: u16 = 0x10;
pub const RX_CTL: u16 = 0x0b;
pub const PHYPWR_RSTCTL: u16 = 0x26;
pub const CLK_SELECT: u16 = 0x33;
pub const RXCOE_CTL: u16 = 0x34;
pub const TXCOE_CTL: u16 = 0x35;
pub const RX_BULKIN_QCTRL: u16 = 0x2e;
pub const MONITOR_MOD: u16 = 0x24;
pub const MEDIUM_STATUS_MODE: u16 = 0x22;
pub const PAUSE_WATERLVL_LOW: u16 = 0x55;
pub const PAUSE_WATERLVL_HIGH: u16 = 0x54;
/// Physical link status register (bit 2 = USB SuperSpeed, bit 1 = High-Speed).
pub const PHYSICAL_LINK_STATUS: u16 = 0x02;

pub const PHYPWR_IPRL: u16 = 0x0020;
pub const CLK_ACS_BCS: u8 = 0x03;
/// Drop CRC errors, IP header alignment, start, accept physical/all multicast/broadcast.
pub const RX_CTL_RUN: u16 = 0x0100 | 0x0200 | 0x0080 | 0x0020 | 0x0002 | 0x0008;
pub const MONITOR_DEFAULT: u8 = 0x40 | 0x20 | 0x04;
pub const COE_ALL: u8 = 0x01 | 0x02 | 0x04 | 0x20 | 0x40;

pub const MEDIUM_RECEIVE_EN: u16 = 0x0100;
pub const MEDIUM_TXFLOW: u16 = 0x0020;
pub const MEDIUM_RXFLOW: u16 = 0x0010;
pub const MEDIUM_FULL_DUPLEX: u16 = 0x0002;
pub const MEDIUM_GIGA: u16 = 0x0001;
pub const MEDIUM_PS: u16 = 0x0200;
pub const MEDIUM_125MHZ: u16 = 0x0008;

pub const PHYSR_SPEED_MASK: u16 = 0xc000;
pub const PHYSR_GIGA: u16 = 0x8000;
pub const PHYSR_100: u16 = 0x4000;
pub const PHYSR_FULL: u16 = 0x2000;
pub const PHYSR_LINK: u16 = 0x0400;

pub const USB_SS: u8 = 0x04;
pub const USB_HS: u8 = 0x02;

/// RX bulk-in queue settings by (USB speed, link speed): `{ctrl, timer_l, timer_h, size, ifg}`.
const BULKIN: [[u8; 5]; 4] = [
    [7, 0x4f, 0, 0x12, 0xff],  // SuperSpeed, gigabit
    [7, 0x20, 3, 0x16, 0xff],  // High-Speed, gigabit
    [7, 0xae, 7, 0x18, 0xff],  // 100 Mbit on USB 2/3
    [7, 0xcc, 0x4c, 0x18, 8],  // anything slower
];

/// Medium mode and RX queue settings for a link: `(medium, bulkin_ctrl, rx_urb_bytes)`.
/// `physr` is the PHY status register, `link_sts` the physical link status (USB speed).
pub fn link_settings(physr: u16, link_sts: u8) -> (u16, [u8; 5], usize) {
    let mut medium = MEDIUM_RECEIVE_EN | MEDIUM_TXFLOW | MEDIUM_RXFLOW;
    let fast_usb = link_sts & (USB_SS | USB_HS) != 0;
    let q = match physr & PHYSR_SPEED_MASK {
        PHYSR_GIGA => {
            medium |= MEDIUM_GIGA | MEDIUM_125MHZ;
            if link_sts & USB_SS != 0 {
                BULKIN[0]
            } else if link_sts & USB_HS != 0 {
                BULKIN[1]
            } else {
                BULKIN[3]
            }
        }
        PHYSR_100 => {
            medium |= MEDIUM_PS;
            if fast_usb { BULKIN[2] } else { BULKIN[3] }
        }
        _ => BULKIN[3],
    };
    if physr & PHYSR_FULL != 0 {
        medium |= MEDIUM_FULL_DUPLEX;
    }
    (medium, q, 1024 * (q[3] as usize + 2))
}

/// Prepends the 8-byte transmit header. When the transfer would end exactly on a packet
/// boundary the header asks the chip to pad it, so no zero-length packet is needed.
pub fn tx_frame(frame: &[u8], max_packet: usize) -> Vec<u8> {
    let hdr2: u32 = if (frame.len() + 8) % max_packet.max(1) == 0 { 0x8000_8000 } else { 0 };
    let mut b = Vec::with_capacity(8 + frame.len());
    b.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    b.extend_from_slice(&hdr2.to_le_bytes());
    b.extend_from_slice(frame);
    b
}

/// Splits a received transfer into Ethernet frames. The last 4 bytes hold `(offset of the
/// packet header table << 16) | packet count`; each packet sits at an 8-byte aligned offset,
/// starting with 2 bytes of IP alignment padding, and has a 4-byte header `len << 16 | flags`.
pub fn parse_rx(buf: &[u8]) -> Vec<Vec<u8>> {
    const CRC_ERR: u32 = 1 << 29;
    const DROP_ERR: u32 = 1 << 31;
    let mut out = Vec::new();
    if buf.len() < 4 {
        return out;
    }
    let body = &buf[..buf.len() - 4];
    let rx_hdr = u32::from_le_bytes(buf[buf.len() - 4..].try_into().unwrap());
    let (count, hdr_off) = ((rx_hdr & 0xffff) as usize, (rx_hdr >> 16) as usize);
    if count == 0 || count * 4 + hdr_off > body.len() {
        return out;
    }
    let data = &body[..hdr_off];
    let mut pos = 0;
    for i in 0..count {
        let h = u32::from_le_bytes(body[hdr_off + 4 * i..hdr_off + 4 * i + 4].try_into().unwrap());
        let len = ((h >> 16) & 0x1fff) as usize;
        let padded = (len + 7) & !7;
        if len == 0 {
            continue;
        }
        if pos + padded > data.len() {
            break;
        }
        if h & (CRC_ERR | DROP_ERR) == 0 && len >= 2 + 14 {
            out.push(data[pos + 2..pos + len].to_vec());
        }
        pos += padded;
    }
    out
}
