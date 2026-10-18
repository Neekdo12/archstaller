//! AMD PCnet32 (Am79C970A / PCnet-PCI II / PCnet-FAST) Ethernet controllers, polled.
//! Standard emulated NIC on many hypervisors and legacy x86 machines.
use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::{Bar, PciDevice};
use crate::{platform, port, Devices};
use alloc::boxed::Box;
use hal::{Error, NetDevice, Result};

// 16-bit I/O Port offsets
pub const APROM0: u16 = 0x00;
pub const RDP: u16 = 0x10;
pub const RAP: u16 = 0x12;
pub const RESET16: u16 = 0x14;
pub const BDP: u16 = 0x16;
pub const RESET32: u16 = 0x18;

// CSR0 bits
pub const CSR0_INIT: u16 = 0x0001;
pub const CSR0_STRT: u16 = 0x0002;
pub const CSR0_STOP: u16 = 0x0004;
pub const CSR0_TDMD: u16 = 0x0008;
pub const CSR0_IDON: u16 = 0x0100;
pub const CSR0_TINT: u16 = 0x0200;
pub const CSR0_RINT: u16 = 0x0400;

// Descriptor Status Bits
pub const DESC_OWN: u16 = 0x8000;
pub const DESC_ERR: u16 = 0x4000;
pub const DESC_STP: u16 = 0x0200;
pub const DESC_ENP: u16 = 0x0100;

pub const RING_SIZE: usize = 32;
pub const BUF_SIZE: usize = 1536;

pub const PCI_VENDOR_AMD: u16 = 0x1022;
pub const PCI_DEVICE_PCNET: u16 = 0x2000;

/// Encodes buffer length as two's complement with top 4 bits set to 1.
#[expect(clippy::cast_sign_loss, reason = "two's complement bit representation in 16-bit register")]
#[inline]
pub fn encode_len(len: usize) -> u16 {
    (-(len as i16) as u16) | 0xf000
}

/// Encodes log2 of ring lengths for the 32-bit initialization block.
#[inline]
pub fn encode_ring_len(tx_len: usize, rx_len: usize) -> u16 {
    let tlen = (tx_len.trailing_zeros() as u16) << 12;
    let rlen = (rx_len.trailing_zeros() as u16) << 4;
    tlen | rlen
}

/// Extracts received frame length (including CRC) from msg_length field.
#[inline]
pub fn decode_rx_len(msg_length: u32) -> usize {
    (msg_length & 0x0fff) as usize
}

/// Whether this driver claims the PCI id (`xtask coverage` counts recognized ids with it).
pub fn recognizes(vendor: u16, device: u16) -> bool {
    vendor == PCI_VENDOR_AMD && device == PCI_DEVICE_PCNET
}

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if !recognizes(dev.vendor, dev.device) || dev.class != 0x02 {
        return;
    }
    let Some(Bar::Io { port, .. }) = dev.bar(0) else {
        return;
    };
    dev.enable();
    if let Ok(d) = Pcnet32::new(port) {
        out.net.push(Box::new(d));
    }
}

pub struct Pcnet32 {
    io: u16,
    init_block: Dma,
    rx_ring: Dma,
    tx_ring: Dma,
    rx_bufs: Dma,
    tx_bufs: Dma,
    rx_next: usize,
    tx_next: usize,
    mac: [u8; 6],
}

impl Pcnet32 {
    fn read_csr(io: u16, idx: u16) -> u16 {
        // SAFETY: RAP register write within device I/O BAR.
        unsafe { port::outw(io + RAP, idx) };
        // SAFETY: RDP register read within device I/O BAR.
        unsafe { port::inw(io + RDP) }
    }

    fn write_csr(io: u16, idx: u16, val: u16) {
        // SAFETY: RAP register write within device I/O BAR.
        unsafe { port::outw(io + RAP, idx) };
        // SAFETY: RDP register write within device I/O BAR.
        unsafe { port::outw(io + RDP, val) };
    }

    fn read_bcr(io: u16, idx: u16) -> u16 {
        // SAFETY: RAP register write within device I/O BAR.
        unsafe { port::outw(io + RAP, idx) };
        // SAFETY: BDP register read within device I/O BAR.
        unsafe { port::inw(io + BDP) }
    }

    fn write_bcr(io: u16, idx: u16, val: u16) {
        // SAFETY: RAP register write within device I/O BAR.
        unsafe { port::outw(io + RAP, idx) };
        // SAFETY: BDP register write within device I/O BAR.
        unsafe { port::outw(io + BDP, val) };
    }

    fn new(io: u16) -> Result<Pcnet32> {
        // Dual-width reset to safely handle either 16-bit or 32-bit previous state
        // SAFETY: Reading reset ports triggers hardware reset of the controller.
        unsafe {
            let _ = port::inl(io + RESET32);
        }
        // SAFETY: Reading 16-bit reset port.
        unsafe {
            let _ = port::inw(io + RESET16);
        }
        platform::delay_us(100);

        // Stop controller
        Self::write_csr(io, 0, CSR0_STOP);

        // Read physical MAC address from APROM
        let mut mac = [0u8; 6];
        for (i, byte) in mac.iter_mut().enumerate() {
            // SAFETY: Reading APROM at io + i yields MAC byte i.
            *byte = unsafe { port::inb(io + APROM0 + i as u16) };
        }

        // Configure BCR20: SWSTYLE = 2 (PCnet-PCI 32-bit software style with 32-bit DMA)
        Self::write_bcr(io, 20, 0x0002);

        // Configure BCR18: Enable burst read/write for bus mastering
        let bcr18 = Self::read_bcr(io, 18);
        Self::write_bcr(io, 18, bcr18 | 0x0060);

        // Mask all interrupts in CSR3 (IDONM, TINTM, RINTM, MERRM, MISSM, BABLM)
        Self::write_csr(io, 3, 0x5f00);

        let init_block = Dma::new(28, 16);
        let rx_ring = Dma::new(RING_SIZE * 16, 16);
        let tx_ring = Dma::new(RING_SIZE * 16, 16);
        let rx_bufs = Dma::new(RING_SIZE * BUF_SIZE, 16);
        let tx_bufs = Dma::new(RING_SIZE * BUF_SIZE, 16);

        // Chip uses 32-bit physical pointers
        if (init_block.phys() + init_block.len() as u64) >> 32 != 0
            || (rx_ring.phys() + rx_ring.len() as u64) >> 32 != 0
            || (tx_ring.phys() + tx_ring.len() as u64) >> 32 != 0
            || (rx_bufs.phys() + rx_bufs.len() as u64) >> 32 != 0
            || (tx_bufs.phys() + tx_bufs.len() as u64) >> 32 != 0
        {
            return Err(Error::Unsupported);
        }

        let dev = Pcnet32 {
            io,
            init_block,
            rx_ring,
            tx_ring,
            rx_bufs,
            tx_bufs,
            rx_next: 0,
            tx_next: 0,
            mac,
        };

        // Initialize Receive Descriptors
        for i in 0..RING_SIZE {
            let buf_phys = dev.rx_bufs.phys_at(i * BUF_SIZE) as u32;
            let d = Mmio::new(dev.rx_ring.as_ptr()).offset(i * 16);
            d.write32(0, buf_phys);
            d.write16(4, encode_len(BUF_SIZE));
            d.write16(6, DESC_OWN);
            d.write32(8, 0);
            d.write32(12, 0);
        }

        // Initialize Transmit Descriptors
        for i in 0..RING_SIZE {
            let d = Mmio::new(dev.tx_ring.as_ptr()).offset(i * 16);
            d.write32(0, 0);
            d.write16(4, 0);
            d.write16(6, 0);
            d.write32(8, 0);
            d.write32(12, 0);
        }

        // Populate Initialization Block (28 bytes)
        let init = Mmio::new(dev.init_block.as_ptr());
        init.write16(0, 0); // mode
        init.write16(2, encode_ring_len(RING_SIZE, RING_SIZE)); // tlen_rlen
        for (i, b) in mac.iter().enumerate() {
            init.write8(4 + i, *b);
        }
        init.write16(10, 0); // reserved
        init.write32(12, 0); // filter[0]
        init.write32(16, 0); // filter[1]
        init.write32(20, dev.rx_ring.phys() as u32);
        init.write32(24, dev.tx_ring.phys() as u32);

        // Write Initialization Block address to CSR1 and CSR2
        let init_phys = dev.init_block.phys() as u32;
        Self::write_csr(io, 1, (init_phys & 0xffff) as u16);
        Self::write_csr(io, 2, ((init_phys >> 16) & 0xffff) as u16);

        // Trigger initialization
        Self::write_csr(io, 0, CSR0_INIT);
        platform::wait_until(1000, || Self::read_csr(io, 0) & CSR0_IDON != 0)?;

        // Acknowledge IDON
        Self::write_csr(io, 0, CSR0_IDON);

        // Start controller
        Self::write_csr(io, 0, CSR0_STRT);

        Ok(dev)
    }
}

impl NetDevice for Pcnet32 {
    fn name(&self) -> &str {
        "pcnet32"
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        // In PCnet-PCI II / PCnet-FAST, BCR4 provides link status.
        // A value other than 0x00c0 indicates active carrier / link up.
        Self::read_bcr(self.io, 4) != 0x00c0
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        if frame.is_empty() || frame.len() > BUF_SIZE {
            return Err(Error::InvalidArgument);
        }

        let slot = self.tx_next;
        let d = Mmio::new(self.tx_ring.as_ptr()).offset(slot * 16);

        platform::wait_until(1000, || d.read16(6) & DESC_OWN == 0)?;

        let off = slot * BUF_SIZE;
        let Some(dest) = self.tx_bufs.as_mut_slice().get_mut(off..off + frame.len()) else {
            return Err(Error::InvalidArgument);
        };
        dest.copy_from_slice(frame);

        d.write32(0, self.tx_bufs.phys_at(off) as u32);
        d.write16(4, encode_len(frame.len()));
        d.write32(8, 0);
        d.write32(12, 0);

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        d.write16(6, DESC_OWN | DESC_STP | DESC_ENP);

        // Trigger transmit demand
        Self::write_csr(self.io, 0, CSR0_TDMD);

        self.tx_next = (self.tx_next + 1) % RING_SIZE;

        // Poll until transmission finishes
        platform::wait_until(1000, || d.read16(6) & DESC_OWN == 0)
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let slot = self.rx_next;
        let d = Mmio::new(self.rx_ring.as_ptr()).offset(slot * 16);
        let status = d.read16(6);

        if status & DESC_OWN != 0 {
            return None;
        }

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        let msg_len = d.read32(8);
        let res = if status & DESC_ERR == 0 && (status & (DESC_STP | DESC_ENP)) == (DESC_STP | DESC_ENP) {
            let raw_len = decode_rx_len(msg_len);
            if raw_len >= 4 {
                let len = (raw_len - 4).min(buf.len());
                let off = slot * BUF_SIZE;
                if let (Some(src), Some(dest)) = (
                    self.rx_bufs.as_slice().get(off..off + len),
                    buf.get_mut(..len),
                ) {
                    dest.copy_from_slice(src);
                    Some(len)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Re-arm descriptor
        d.write16(4, encode_len(BUF_SIZE));
        d.write32(8, 0);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        d.write16(6, DESC_OWN);

        // Acknowledge RINT
        Self::write_csr(self.io, 0, CSR0_RINT);

        self.rx_next = (self.rx_next + 1) % RING_SIZE;
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_amd_pcnet32_devices() {
        assert!(recognizes(0x1022, 0x2000));

        // Rejects non-AMD vendor
        assert!(!recognizes(0x8086, 0x2000));
        assert!(!recognizes(0x10ec, 0x2000));

        // Rejects unknown device ID
        assert!(!recognizes(0x1022, 0x2001));
        assert!(!recognizes(0x1022, 0x1000));
    }

    #[test]
    fn encodes_twos_complement_length() {
        for len in [60, 1500, 1514, 1536] {
            let encoded = encode_len(len);
            assert_eq!(encoded & 0xf000, 0xf000);
            assert_eq!((encoded as i16), -(len as i16));
        }
    }

    #[test]
    fn encodes_ring_lengths() {
        assert_eq!(encode_ring_len(16, 16), 0x4040);
        assert_eq!(encode_ring_len(32, 32), 0x5050);
        assert_eq!(encode_ring_len(64, 64), 0x6060);
    }

    #[test]
    fn decodes_rx_packet_length() {
        assert_eq!(decode_rx_len(64), 64);
        assert_eq!(decode_rx_len(1518), 1518);
        // High bits masked out
        assert_eq!(decode_rx_len(0x8000_0000 | 1518), 1518);
    }
}
