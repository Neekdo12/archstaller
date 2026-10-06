//! VMware vmxnet3 virtual NIC (VMware Workstation, Player, Fusion, ESXi; QEMU's `vmxnet3` too), polled.
//! The device is configured through a "shared area" the guest hands over, one TX and one RX queue.
//! Layout and sequence follow iPXE's `vmxnet3` driver.
use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::{platform, Devices};
use alloc::boxed::Box;
use hal::{Error, NetDevice, Result};

const VENDOR: u16 = 0x15ad;
const DEVICE: u16 = 0x07b0;

// BAR0 ("PT") and BAR1 ("VD") registers.
const PT_TXPROD: usize = 0x600;
const PT_RXPROD: usize = 0x800;
const VD_VRRS: usize = 0x00;
const VD_UVRS: usize = 0x08;
const VD_DSAL: usize = 0x10;
const VD_DSAH: usize = 0x18;
const VD_CMD: usize = 0x20;
const VD_MACL: usize = 0x28;
const VD_MACH: usize = 0x30;

const CMD_ACTIVATE_DEV: u32 = 0xcafe_0000;
const CMD_RESET_DEV: u32 = 0xcafe_0002;
const CMD_GET_LINK: u32 = 0xf00d_0002;
const CMD_GET_PERM_MAC_LO: u32 = 0xf00d_0003;
const CMD_GET_PERM_MAC_HI: u32 = 0xf00d_0004;

const NUM: usize = 32; // descriptors and completions, each ring
const MTU: usize = 1514 + 4 + 4;
const BUF: usize = 2048;
const RX_FILL: usize = 8;

// Offsets inside the single DMA area (rings aligned to 512, then queues, then the shared area).
const TX_DESC: usize = 0;
const TX_COMP: usize = 512;
const RX_DESC: usize = 1024;
const RX_COMP: usize = 1536;
const QUEUES: usize = 2048;
const SHARED: usize = 2560;
const DMA_LEN: usize = 3328;

const TXF_GEN: u32 = 0x0000_4000;
const TXF_EOP: u32 = 0x0000_1000;
const TXF_CQ: u32 = 0x0000_2000;
const TXCF_GEN: u32 = 0x8000_0000;
const RXF_GEN: u32 = 0x8000_0000;
const RXCF_GEN: u32 = 0x8000_0000;

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.vendor != VENDOR || dev.device != DEVICE {
        return;
    }
    let (Some(pt), Some(vd)) = (dev.map_bar(0), dev.map_bar(1)) else { return };
    dev.enable();
    match Vmxnet3::new(pt, vd) {
        Ok(d) => out.net.push(Box::new(d)),
        Err(e) => hal::info!("vmxnet3: not brought up ({e:?})"),
    }
}

pub struct Vmxnet3 {
    pt: Mmio,
    vd: Mmio,
    dma: Dma,
    rx_bufs: Dma,
    tx_bufs: Dma,
    mac: [u8; 6],
    tx_prod: u32,
    tx_cons: u32,
    rx_prod: u32,
    rx_fill: u32,
    rx_cons: u32,
}

impl Vmxnet3 {
    fn command(vd: &Mmio, cmd: u32) -> u32 {
        vd.write32(VD_CMD, cmd);
        vd.read32(VD_CMD)
    }

    fn new(pt: Mmio, vd: Mmio) -> Result<Vmxnet3> {
        // Tell the device which versions we speak, then reset it.
        vd.write32(VD_VRRS, 1);
        vd.write32(VD_UVRS, 1);
        Self::command(&vd, CMD_RESET_DEV);
        let lo = Self::command(&vd, CMD_GET_PERM_MAC_LO);
        let hi = Self::command(&vd, CMD_GET_PERM_MAC_HI);
        let mac = [lo as u8, (lo >> 8) as u8, (lo >> 16) as u8, (lo >> 24) as u8, hi as u8, (hi >> 8) as u8];
        if mac == [0; 6] {
            return Err(Error::Unsupported);
        }

        let dma = Dma::new(DMA_LEN, 512);
        let rx_bufs = Dma::new(NUM * BUF, 4096);
        let tx_bufs = Dma::new(NUM * BUF, 4096);
        let d = Mmio::new(dma.as_ptr());
        let base = dma.phys();

        // Queue descriptors: one TX queue at +0, one RX queue at +256.
        let q = d.offset(QUEUES);
        q.write64(16, base + TX_DESC as u64); // tx.cfg.desc_address
        q.write64(32, base + TX_COMP as u64); // tx.cfg.comp_address
        q.write32(56, NUM as u32); // tx.cfg.num_desc
        q.write32(64, NUM as u32); // tx.cfg.num_comp
        q.write64(256 + 16, base + RX_DESC as u64); // rx.cfg.desc_address[0]
        q.write64(256 + 32, base + RX_COMP as u64); // rx.cfg.comp_address
        q.write32(256 + 56, NUM as u32); // rx.cfg.num_desc[0]
        q.write32(256 + 64, NUM as u32); // rx.cfg.num_comp

        // Shared area.
        let s = d.offset(SHARED);
        s.write32(0, 0xbabe_fee1); // magic
        s.write32(8, 0x6950_5845); // misc.version
        s.write32(8 + 8, 1); // version_support
        s.write32(8 + 12, 1); // upt_version_support
        s.write64(8 + 32, base + QUEUES as u64); // queue_desc_address
        s.write32(8 + 44, 512); // queue_desc_len
        s.write32(8 + 48, MTU as u32); // mtu
        s.write8(8 + 54, 1); // num_tx_queues
        s.write8(8 + 55, 1); // num_rx_queues
        s.write8(81, 1); // interrupt.num_intrs
        s.write32(108, 1); // interrupt.control: disable all interrupts
        s.write32(120, 0x01 | 0x04 | 0x08); // rx_filter.mode: unicast, broadcast, all multicast

        // MAC address, then hand over the shared area and activate.
        vd.write32(VD_MACL, lo);
        vd.write32(VD_MACH, hi & 0xffff);
        let shared = base + SHARED as u64;
        vd.write32(VD_DSAL, shared as u32);
        vd.write32(VD_DSAH, (shared >> 32) as u32);
        let status = Self::command(&vd, CMD_ACTIVATE_DEV);
        if status != 0 {
            hal::info!("vmxnet3: activation failed, status {status:#x}");
            return Err(Error::Io);
        }
        let mut nic = Vmxnet3 { pt, vd, dma, rx_bufs, tx_bufs, mac, tx_prod: 0, tx_cons: 0, rx_prod: 0, rx_fill: 0, rx_cons: 0 };
        nic.refill_rx();
        Ok(nic)
    }

    fn d(&self) -> Mmio {
        Mmio::new(self.dma.as_ptr())
    }

    fn refill_rx(&mut self) {
        let d = self.d();
        let mut any = false;
        while (self.rx_fill as usize) < RX_FILL {
            let idx = self.rx_prod as usize % NUM;
            let gen = if self.rx_prod as usize & NUM != 0 { 0 } else { RXF_GEN };
            let desc = d.offset(RX_DESC + 16 * idx);
            desc.write64(0, self.rx_bufs.phys_at(idx * BUF));
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            desc.write32(8, gen | MTU as u32);
            self.rx_prod += 1;
            self.rx_fill += 1;
            any = true;
        }
        if any {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            self.pt.write32(PT_RXPROD, self.rx_prod % NUM as u32);
        }
    }
}

impl NetDevice for Vmxnet3 {
    fn name(&self) -> &str {
        "vmxnet3"
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        Self::command(&self.vd, CMD_GET_LINK) & 1 != 0
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() > BUF || frame.is_empty() {
            return Err(Error::InvalidArgument);
        }
        let d = self.d();
        let idx = self.tx_prod as usize % NUM;
        let gen = if self.tx_prod as usize & NUM != 0 { 0 } else { TXF_GEN };
        self.tx_bufs.as_mut_slice()[idx * BUF..idx * BUF + frame.len()].copy_from_slice(frame);
        let desc = d.offset(TX_DESC + 16 * idx);
        desc.write64(0, self.tx_bufs.phys_at(idx * BUF));
        desc.write32(12, TXF_CQ | TXF_EOP);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        desc.write32(8, gen | frame.len() as u32);
        self.tx_prod += 1;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.pt.write32(PT_TXPROD, self.tx_prod % NUM as u32);
        // Wait for its completion.
        let want = self.tx_cons + 1;
        let r = platform::wait_until(1000, || {
            let c = self.d().offset(TX_COMP + 16 * (self.tx_cons as usize % NUM));
            let gen = if self.tx_cons as usize & NUM != 0 { 0 } else { TXCF_GEN };
            c.read32(12) & TXCF_GEN == gen
        });
        if r.is_ok() {
            self.tx_cons = want;
        }
        r
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let d = self.d();
        let comp = d.offset(RX_COMP + 16 * (self.rx_cons as usize % NUM));
        let gen = if self.rx_cons as usize & NUM != 0 { 0 } else { RXCF_GEN };
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        if comp.read32(12) & RXCF_GEN != gen {
            return None;
        }
        self.rx_cons += 1;
        let idx = comp.read32(0) as usize % NUM;
        let len = (comp.read32(8) & 0x3fff) as usize;
        self.rx_fill = self.rx_fill.saturating_sub(1);
        let n = len.min(buf.len());
        buf[..n].copy_from_slice(&self.rx_bufs.as_slice()[idx * BUF..idx * BUF + n]);
        self.refill_rx();
        Some(n)
    }
}
