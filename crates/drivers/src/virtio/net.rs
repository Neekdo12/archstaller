use super::{Transport, VirtQueue, DESC_F_WRITE};
use crate::dma::Dma;
use crate::platform;
use hal::{Error, NetDevice, Result};

const F_MAC: u32 = 1 << 5;
const F_STATUS: u32 = 1 << 16;

/// virtio_net_hdr with VERSION_1 (includes num_buffers).
const HDR: usize = 12;
const BUF: usize = 2048;
const RX_BUFS: u16 = 32;

pub struct VirtioNet {
    t: Transport,
    rx: VirtQueue,
    tx: VirtQueue,
    rx_mem: Dma,
    tx_mem: Dma,
    mac: [u8; 6],
    has_status: bool,
}

impl VirtioNet {
    pub fn new(t: Transport) -> Result<VirtioNet> {
        let accepted = t.negotiate(F_MAC | F_STATUS)?;
        if accepted & F_MAC == 0 {
            return Err(Error::Unsupported);
        }
        let mut rx = t.queue(0)?;
        let tx = t.queue(1)?;
        let mut mac = [0u8; 6];
        for (i, b) in mac.iter_mut().enumerate() {
            *b = t.device_cfg.read8(i);
        }
        let n = RX_BUFS.min(rx.size());
        let rx_mem = Dma::new(BUF * n as usize, 4096);
        for i in 0..n {
            rx.set_desc(i, rx_mem.phys_at(BUF * i as usize), BUF as u32, DESC_F_WRITE, 0);
            rx.submit(i);
        }
        t.driver_ok();
        Ok(VirtioNet {
            t,
            rx,
            tx,
            rx_mem,
            tx_mem: Dma::new(BUF, 16),
            mac,
            has_status: accepted & F_STATUS != 0,
        })
    }
}

impl NetDevice for VirtioNet {
    fn name(&self) -> &str {
        "virtio-net"
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        !self.has_status || self.t.device_cfg.read16(6) & 1 != 0
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() + HDR > BUF {
            return Err(Error::InvalidArgument);
        }
        let m = self.tx_mem.as_mut_slice();
        m[..HDR].fill(0);
        m[HDR..HDR + frame.len()].copy_from_slice(frame);
        self.tx.set_desc(0, self.tx_mem.phys(), (HDR + frame.len()) as u32, 0, 0);
        self.tx.submit(0);
        platform::wait_until(1000, || self.tx.pop_used().is_some())
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let (id, len) = self.rx.pop_used()?;
        let off = BUF * id as usize;
        let total = len as usize;
        let n = total.saturating_sub(HDR).min(buf.len());
        buf[..n].copy_from_slice(&self.rx_mem.as_slice()[off + HDR..off + HDR + n]);
        self.rx.submit(id);
        Some(n)
    }
}
