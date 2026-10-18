//! Intel 8255x series (e100 / Intel PRO/100) Fast Ethernet controllers, polled.
//! Supports 82557, 82558, 82559, 82550, 82551, and ICH integrated LAN controllers.
use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::{platform, Devices};
use alloc::boxed::Box;
use hal::{Error, NetDevice, Result};

// CSR Register byte offsets
pub const SCB_STATUS: usize = 0x00;
pub const SCB_STAT_ACK: usize = 0x01;
pub const SCB_CMD_LO: usize = 0x02;
pub const SCB_CMD_HI: usize = 0x03;
pub const SCB_GEN_PTR: usize = 0x04;
pub const SCB_PORT: usize = 0x08;
pub const SCB_EEPROM: usize = 0x0e;

// SCB Command Unit (CU) commands
pub const CUC_START: u8 = 0x10;
pub const CUC_LOAD_BASE: u8 = 0x60;

// SCB Receive Unit (RU) commands
pub const RUC_START: u8 = 0x01;
pub const RUC_RESUME: u8 = 0x02;
pub const RUC_LOAD_BASE: u8 = 0x06;

// SCB Status masks
pub const RUS_SUSPENDED: u8 = 0x04;
pub const RUS_READY: u8 = 0x10;

// SCB Interrupt mask
pub const IRQ_MASK_ALL: u8 = 0x01;

// Port commands
pub const PORT_SOFTWARE_RESET: u32 = 0x0000_0000;
pub const PORT_SELECTIVE_RESET: u32 = 0x0000_0002;

// Command Block (CB) status and command bits
pub const CB_STATUS_C: u16 = 0x8000; // Complete
pub const CB_STATUS_OK: u16 = 0x2000; // OK
pub const CB_CMD_TRANSMIT: u16 = 0x0004;
pub const CB_CMD_CONFIG: u16 = 0x0002;
pub const CB_CMD_IAADDR: u16 = 0x0001;
pub const CB_CMD_S: u16 = 0x4000; // Suspend
pub const CB_CMD_EL: u16 = 0x8000; // End of list

// EEPROM bits
pub const EE_SK: u8 = 0x01;
pub const EE_CS: u8 = 0x02;
pub const EE_DI: u8 = 0x04;
pub const EE_DO: u8 = 0x08;

pub const RX_RING_SIZE: usize = 16;
pub const BUF_SIZE: usize = 1536;
pub const RFD_SIZE: usize = 16 + BUF_SIZE;

/// Known Intel PRO/100 device IDs from Linux's `e100` driver and PCI specifications.
pub const IDS_E100: &[u16] = &[
    0x1029, 0x1030, 0x1031, 0x1032, 0x1033, 0x1034, 0x1038, 0x1039, 0x103a, 0x103b, 0x103c,
    0x103d, 0x103e, 0x1050, 0x1051, 0x1052, 0x1053, 0x1054, 0x1055, 0x1056, 0x1057, 0x1059,
    0x1064, 0x1065, 0x1066, 0x1067, 0x1068, 0x1069, 0x106a, 0x106b, 0x1091, 0x1092, 0x1093,
    0x1094, 0x1095, 0x10fe, 0x1209, 0x1229, 0x2449, 0x2459, 0x245d, 0x27dc,
];

/// Encodes frame length for simplified TCB (with EOF bit 15 set).
#[inline]
pub fn encode_tcb_count(len: usize) -> u16 {
    (len as u16) | 0x8000
}

/// Decodes received packet byte count from RFD actual_size field, stripping 4-byte CRC.
#[inline]
pub fn decode_rfd_len(actual_size: u16) -> Option<usize> {
    if actual_size & 0x8000 == 0 {
        // EOF not set
        return None;
    }
    let raw = (actual_size & 0x3fff) as usize;
    if !(4..=BUF_SIZE + 4).contains(&raw) {
        return None;
    }
    Some(raw - 4)
}

/// Whether this driver claims the PCI id (`xtask coverage` counts recognized ids with it).
pub fn recognizes(vendor: u16, device: u16) -> bool {
    vendor == 0x8086 && IDS_E100.contains(&device)
}

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if !recognizes(dev.vendor, dev.device) || dev.class != 0x02 {
        return;
    }
    let Some(regs) = dev.map_bar(0) else { return };
    dev.enable();
    if let Ok(d) = E100::new(regs) {
        out.net.push(Box::new(d));
    }
}

pub struct E100 {
    regs: Mmio,
    tx_cb: Dma,
    rx_ring: Dma,
    rx_head: usize,
    mac: [u8; 6],
}

impl E100 {
    fn scb_wait(&self) -> Result<()> {
        platform::wait_until(100, || self.regs.read8(SCB_CMD_LO) == 0)
    }

    fn eeprom_read(&self, addr_len: &mut u16, addr: u16) -> u16 {
        let op_read = 0x06u32;
        let cmd_addr_data = ((op_read << *addr_len) | (addr as u32)) << 16;
        let mut data: u16 = 0;

        // Chip select
        self.regs.write8(SCB_EEPROM, EE_CS | EE_SK);
        platform::delay_us(4);

        for i in (0..=31).rev() {
            let ctrl = if cmd_addr_data & (1 << i) != 0 {
                EE_CS | EE_DI
            } else {
                EE_CS
            };
            self.regs.write8(SCB_EEPROM, ctrl);
            platform::delay_us(4);

            self.regs.write8(SCB_EEPROM, ctrl | EE_SK);
            platform::delay_us(4);

            let in_ctrl = self.regs.read8(SCB_EEPROM);
            if in_ctrl & EE_DO == 0 && i > 16 {
                let len = u16::try_from(i - 16).unwrap_or(0);
                *addr_len = addr_len.saturating_sub(len);
            }
            data = (data << 1) | if in_ctrl & EE_DO != 0 { 1 } else { 0 };
        }

        // Chip deselect
        self.regs.write8(SCB_EEPROM, 0);
        platform::delay_us(4);
        data
    }

    fn new(regs: Mmio) -> Result<E100> {
        // Selective reset then software reset via Port register
        regs.write32(SCB_PORT, PORT_SELECTIVE_RESET);
        platform::delay_us(20);
        regs.write32(SCB_PORT, PORT_SOFTWARE_RESET);
        platform::delay_us(20);

        // Disable device interrupts
        regs.write8(SCB_CMD_HI, IRQ_MASK_ALL);

        let tx_cb = Dma::new(RFD_SIZE, 16);
        let rx_ring = Dma::new(RX_RING_SIZE * RFD_SIZE, 16);

        if (tx_cb.phys() + RFD_SIZE as u64) >> 32 != 0
            || (rx_ring.phys() + (RX_RING_SIZE * RFD_SIZE) as u64) >> 32 != 0
        {
            return Err(Error::Unsupported);
        }

        let mut dev = E100 {
            regs,
            tx_cb,
            rx_ring,
            rx_head: 0,
            mac: [0; 6],
        };

        // Read MAC address from EEPROM
        let mut addr_len = 8u16;
        let w0 = dev.eeprom_read(&mut addr_len, 0);
        let w1 = dev.eeprom_read(&mut addr_len, 1);
        let w2 = dev.eeprom_read(&mut addr_len, 2);
        dev.mac = [
            w0 as u8,
            (w0 >> 8) as u8,
            w1 as u8,
            (w1 >> 8) as u8,
            w2 as u8,
            (w2 >> 8) as u8,
        ];

        // Load CU Base and RU Base (0)
        dev.scb_wait()?;
        regs.write32(SCB_GEN_PTR, 0);
        regs.write8(SCB_CMD_LO, CUC_LOAD_BASE);
        dev.scb_wait()?;

        regs.write32(SCB_GEN_PTR, 0);
        regs.write8(SCB_CMD_LO, RUC_LOAD_BASE);
        dev.scb_wait()?;

        // Configure controller Action CB
        {
            let cb = Mmio::new(dev.tx_cb.as_ptr());
            cb.write16(0, 0); // status = 0
            cb.write16(2, CB_CMD_CONFIG | CB_CMD_EL | CB_CMD_S);
            cb.write32(4, 0); // link = 0

            // 22 configuration bytes at offset 8
            cb.write8(8, 22); // byte count
            cb.write8(9, 0x08); // rx fifo limit
            cb.write8(10, 0);
            cb.write8(11, 0x01); // MWWI
            cb.write8(12, 0);
            cb.write8(13, 0);
            cb.write8(14, 0x32); // direct rx
            cb.write8(15, 0);
            cb.write8(16, 0x01); // 503/MII
            cb.write8(23, 0xc8); // broadcast enable, CRC, IFS
            cb.write8(27, 0x40); // full duplex pin control

            regs.write32(SCB_GEN_PTR, dev.tx_cb.phys() as u32);
            regs.write8(SCB_CMD_LO, CUC_START);
            dev.scb_wait()?;
            platform::wait_until(1000, || {
                let s = cb.read16(0);
                s & CB_STATUS_C != 0
            })?;
        }

        // Set Individual Address (MAC) Action CB
        {
            let cb = Mmio::new(dev.tx_cb.as_ptr());
            cb.write16(0, 0);
            cb.write16(2, CB_CMD_IAADDR | CB_CMD_EL | CB_CMD_S);
            cb.write32(4, 0);
            for (i, byte) in dev.mac.iter().enumerate() {
                cb.write8(8 + i, *byte);
            }

            regs.write32(SCB_GEN_PTR, dev.tx_cb.phys() as u32);
            regs.write8(SCB_CMD_LO, CUC_START);
            dev.scb_wait()?;
            platform::wait_until(1000, || {
                let s = cb.read16(0);
                s & CB_STATUS_C != 0
            })?;
        }

        // Initialize circular RFD ring
        let rx_phys = dev.rx_ring.phys();
        for i in 0..RX_RING_SIZE {
            let next_i = (i + 1) % RX_RING_SIZE;
            let next_phys = (rx_phys + (next_i * RFD_SIZE) as u64) as u32;
            let rfd = Mmio::new(dev.rx_ring.as_ptr()).offset(i * RFD_SIZE);
            rfd.write16(0, 0); // status
            rfd.write16(2, 0); // command
            rfd.write32(4, next_phys); // link
            rfd.write32(8, 0xffff_ffff); // rbd = simplified mode
            rfd.write16(12, 0); // actual_size
            rfd.write16(14, BUF_SIZE as u16); // size
        }

        // Start Receive Unit
        regs.write32(SCB_GEN_PTR, dev.rx_ring.phys() as u32);
        regs.write8(SCB_CMD_LO, RUC_START);
        dev.scb_wait()?;

        Ok(dev)
    }
}

impl NetDevice for E100 {
    fn name(&self) -> &str {
        "e100"
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        // e100 SCB status RU state shows active when carrier is ready
        let scb_status = self.regs.read8(SCB_STATUS);
        let ru_state = (scb_status >> 2) & 0x0f;
        ru_state == (RUS_READY >> 2) || ru_state == (RUS_SUSPENDED >> 2)
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        if frame.is_empty() || frame.len() > BUF_SIZE {
            return Err(Error::InvalidArgument);
        }

        self.scb_wait()?;

        let cb = Mmio::new(self.tx_cb.as_ptr());
        cb.write16(0, 0); // status = 0
        cb.write16(2, CB_CMD_TRANSMIT | CB_CMD_EL | CB_CMD_S);
        cb.write32(4, 0); // link = 0
        cb.write32(8, 0xffff_ffff); // tbd_array = simplified mode
        cb.write16(12, encode_tcb_count(frame.len()));
        cb.write8(14, 0xe0); // threshold
        cb.write8(15, 0); // tbd_number

        let Some(dest) = self.tx_cb.as_mut_slice().get_mut(16..16 + frame.len()) else {
            return Err(Error::InvalidArgument);
        };
        dest.copy_from_slice(frame);

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        self.regs.write32(SCB_GEN_PTR, self.tx_cb.phys() as u32);
        self.regs.write8(SCB_CMD_LO, CUC_START);
        self.scb_wait()?;

        platform::wait_until(1000, || {
            let s = cb.read16(0);
            s & CB_STATUS_C != 0
        })
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let off = self.rx_head * RFD_SIZE;
        let rfd = Mmio::new(self.rx_ring.as_ptr()).offset(off);
        let status = rfd.read16(0);

        if status & CB_STATUS_C == 0 {
            return None;
        }

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        let actual_size = rfd.read16(12);
        let res = if status & CB_STATUS_OK != 0 {
            decode_rfd_len(actual_size).and_then(|len| {
                let n = len.min(buf.len());
                let src = self.rx_ring.as_slice().get(off + 16..off + 16 + n)?;
                let dest = buf.get_mut(..n)?;
                dest.copy_from_slice(src);
                Some(n)
            })
        } else {
            None
        };

        // Reset RFD
        rfd.write16(0, 0);
        rfd.write16(12, 0);

        // Acknowledge interrupts
        self.regs.write8(SCB_STAT_ACK, 0xf0);

        // Resume RU if suspended
        let scb_status = self.regs.read8(SCB_STATUS);
        if (scb_status & 0x3c) == RUS_SUSPENDED && self.scb_wait().is_ok() {
            self.regs.write8(SCB_CMD_LO, RUC_RESUME);
        }

        self.rx_head = (self.rx_head + 1) % RX_RING_SIZE;
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_intel_pro100_devices() {
        // Standard PRO/100 and ER
        assert!(recognizes(0x8086, 0x1229));
        assert!(recognizes(0x8086, 0x1209));
        // InBusiness and Basic
        assert!(recognizes(0x8086, 0x1029));
        assert!(recognizes(0x8086, 0x1030));
        // ICH3/4/5/6/7 variants
        assert!(recognizes(0x8086, 0x1031));
        assert!(recognizes(0x8086, 0x1039));
        assert!(recognizes(0x8086, 0x103e));
        assert!(recognizes(0x8086, 0x1050));
        assert!(recognizes(0x8086, 0x1064));
        assert!(recognizes(0x8086, 0x1091));
        assert!(recognizes(0x8086, 0x2449));
        assert!(recognizes(0x8086, 0x27dc));

        // Non-Intel vendors must be rejected
        assert!(!recognizes(0x10ec, 0x1229));
        assert!(!recognizes(0x1022, 0x1229));

        // Non-e100 Intel devices must be rejected
        assert!(!recognizes(0x8086, 0x100e)); // e1000
        assert!(!recognizes(0x8086, 0x1539)); // igb
        assert!(!recognizes(0x8086, 0xffff));
    }

    #[test]
    fn encodes_simplified_tcb_count() {
        assert_eq!(encode_tcb_count(60), 60 | 0x8000);
        assert_eq!(encode_tcb_count(1500), 1500 | 0x8000);
        assert_eq!(encode_tcb_count(1514), 1514 | 0x8000);
    }

    #[test]
    fn decodes_rfd_actual_size() {
        // Normal valid frames with 4-byte CRC included
        assert_eq!(decode_rfd_len(0x8000 | (64 + 4)), Some(64));
        assert_eq!(decode_rfd_len(0x8000 | (1500 + 4)), Some(1500));
        assert_eq!(decode_rfd_len(0x8000 | (1514 + 4)), Some(1514));

        // Missing EOF bit 15
        assert_eq!(decode_rfd_len(64 + 4), None);

        // Invalid length (< 4 or > BUF_SIZE + 4)
        assert_eq!(decode_rfd_len(0x8000 | 2), None);
        assert_eq!(decode_rfd_len(0x8000 | 2000), None);
    }

    #[test]
    fn rfd_and_buffer_sizes_aligned() {
        assert_eq!(BUF_SIZE, 1536);
        assert_eq!(RFD_SIZE, 1552);
        assert_eq!(RFD_SIZE % 16, 0); // 16-byte alignment
    }
}
