//! NVMe controller: admin queue + one I/O queue pair, namespace 1, polled.
use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::{platform, Devices};
use alloc::boxed::Box;
use alloc::string::String;
use hal::{BlockDevice, Error, Result};

const CAP: usize = 0x00;
const CC: usize = 0x14;
const CSTS: usize = 0x1c;
const AQA: usize = 0x24;
const ASQ: usize = 0x28;
const ACQ: usize = 0x30;

const QSIZE: u16 = 16;
const PAGE: usize = 4096;
/// Bounce buffer; 32 pages, so PRP2 is a list for larger transfers.
const CHUNK: usize = 128 * 1024;

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.class != 0x01 || dev.subclass != 0x08 || dev.prog_if != 0x02 {
        return;
    }
    let Some(regs) = dev.map_bar(0) else { return };
    dev.enable();
    if let Ok(d) = Nvme::new(regs) {
        out.block.push(Box::new(d));
    }
}

struct Queue {
    sq: Dma,
    cq: Dma,
    tail: u16,
    head: u16,
    phase: bool,
    sq_db: usize,
    cq_db: usize,
    next_cid: u16,
}

impl Queue {
    fn new(qid: u16, dstrd: usize) -> Queue {
        let stride = 4usize << dstrd;
        Queue {
            sq: Dma::new(QSIZE as usize * 64, PAGE),
            cq: Dma::new(QSIZE as usize * 16, PAGE),
            tail: 0,
            head: 0,
            phase: true,
            sq_db: 0x1000 + 2 * qid as usize * stride,
            cq_db: 0x1000 + (2 * qid as usize + 1) * stride,
            next_cid: 1,
        }
    }

    /// Submits `cmd` (dwords 0, 1, 6.. filled by caller; CID assigned here) and waits.
    /// Returns completion dword 0.
    fn exec(&mut self, regs: &Mmio, mut cmd: [u32; 16]) -> Result<u32> {
        let cid = self.next_cid;
        self.next_cid = self.next_cid.wrapping_add(1).max(1);
        cmd[0] = (cmd[0] & 0xffff) | (cid as u32) << 16;
        let slot = Mmio::new(self.sq.as_ptr()).offset(64 * self.tail as usize);
        for (i, v) in cmd.iter().enumerate() {
            slot.write32(4 * i, *v);
        }
        self.tail = (self.tail + 1) % QSIZE;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        regs.write32(self.sq_db, self.tail as u32);

        let entry = Mmio::new(self.cq.as_ptr()).offset(16 * self.head as usize);
        let phase = self.phase;
        platform::wait_until(5000, || (entry.read32(12) >> 16) & 1 == phase as u32)?;
        let dw0 = entry.read32(0);
        let status = entry.read32(12) >> 17;
        self.head += 1;
        if self.head == QSIZE {
            self.head = 0;
            self.phase = !self.phase;
        }
        regs.write32(self.cq_db, self.head as u32);
        if status != 0 {
            return Err(Error::Io);
        }
        Ok(dw0)
    }
}

pub struct Nvme {
    regs: Mmio,
    admin: Queue,
    io: Queue,
    data: Dma,
    prp_list: Dma,
    model: String,
    serial: String,
    lba_size: u32,
    lbas: u64,
}

fn cmd(opcode: u8, nsid: u32) -> [u32; 16] {
    let mut c = [0u32; 16];
    c[0] = opcode as u32;
    c[1] = nsid;
    c
}

fn trimmed(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim_matches(|c: char| c == ' ' || c == '\0').into()
}

impl Nvme {
    fn new(regs: Mmio) -> Result<Nvme> {
        let cap = regs.read64(CAP);
        let dstrd = ((cap >> 32) & 0xf) as usize;
        if (cap >> 48) & 0xf != 0 {
            return Err(Error::Unsupported); // needs >4 KiB pages
        }
        let timeout_ms = (((cap >> 24) & 0xff) + 1) * 500;

        regs.write32(CC, 0);
        platform::wait_until(timeout_ms, || regs.read32(CSTS) & 1 == 0)?;

        let admin = Queue::new(0, dstrd);
        regs.write32(AQA, ((QSIZE as u32 - 1) << 16) | (QSIZE as u32 - 1));
        regs.write64(ASQ, admin.sq.phys());
        regs.write64(ACQ, admin.cq.phys());
        regs.write32(CC, (4 << 20) | (6 << 16) | 1); // CQ entry 16 B, SQ entry 64 B, enable
        platform::wait_until(timeout_ms, || regs.read32(CSTS) & 1 == 1)?;

        let mut d = Nvme {
            regs,
            admin,
            io: Queue::new(1, dstrd),
            data: Dma::new(CHUNK, PAGE),
            prp_list: Dma::new(PAGE, PAGE),
            model: String::new(),
            serial: String::new(),
            lba_size: 512,
            lbas: 0,
        };
        d.setup()?;
        Ok(d)
    }

    fn setup(&mut self) -> Result<()> {
        let id = Dma::new(PAGE, PAGE);
        let mut c = cmd(0x06, 0);
        c[6] = id.phys() as u32;
        c[7] = (id.phys() >> 32) as u32;
        c[10] = 1; // CNS: controller
        self.admin.exec(&self.regs, c)?;
        self.serial = trimmed(&id.as_slice()[4..24]);
        self.model = trimmed(&id.as_slice()[24..64]);

        let mut c = cmd(0x06, 1);
        c[6] = id.phys() as u32;
        c[7] = (id.phys() >> 32) as u32;
        c[10] = 0; // CNS: namespace
        self.admin.exec(&self.regs, c)?;
        let ns = id.as_slice();
        self.lbas = u64::from_le_bytes(ns[0..8].try_into().unwrap());
        let fmt = (ns[26] & 0xf) as usize;
        let lbaf = u32::from_le_bytes(ns[128 + 4 * fmt..132 + 4 * fmt].try_into().unwrap());
        let shift = (lbaf >> 16) & 0xff;
        if shift != 9 && shift != 12 {
            return Err(Error::Unsupported);
        }
        self.lba_size = 1 << shift;

        let mut c = cmd(0x05, 0); // create I/O CQ
        c[6] = self.io.cq.phys() as u32;
        c[7] = (self.io.cq.phys() >> 32) as u32;
        c[10] = ((QSIZE as u32 - 1) << 16) | 1;
        c[11] = 1; // physically contiguous, no interrupts
        self.admin.exec(&self.regs, c)?;

        let mut c = cmd(0x01, 0); // create I/O SQ
        c[6] = self.io.sq.phys() as u32;
        c[7] = (self.io.sq.phys() >> 32) as u32;
        c[10] = ((QSIZE as u32 - 1) << 16) | 1;
        c[11] = (1 << 16) | 1; // CQ 1, contiguous
        self.admin.exec(&self.regs, c)?;
        Ok(())
    }

    fn rw(&mut self, write: bool, lba: u64, len: usize) -> Result<()> {
        let nlb = (len / self.lba_size as usize) as u32;
        let mut c = cmd(if write { 0x01 } else { 0x02 }, 1);
        let base = self.data.phys();
        c[6] = base as u32;
        c[7] = (base >> 32) as u32;
        if len > PAGE {
            if len <= 2 * PAGE {
                let p2 = base + PAGE as u64;
                c[8] = p2 as u32;
                c[9] = (p2 >> 32) as u32;
            } else {
                let pages = len.div_ceil(PAGE);
                let list = Mmio::new(self.prp_list.as_ptr());
                for i in 1..pages {
                    list.write64(8 * (i - 1), base + (i * PAGE) as u64);
                }
                let lp = self.prp_list.phys();
                c[8] = lp as u32;
                c[9] = (lp >> 32) as u32;
            }
        }
        c[10] = lba as u32;
        c[11] = (lba >> 32) as u32;
        c[12] = nlb - 1;
        self.io.exec(&self.regs, c).map(|_| ())
    }

    fn check(&self, lba: u64, len: usize) -> Result<()> {
        let ss = self.lba_size as usize;
        if len % ss != 0 || lba.checked_add((len / ss) as u64).map_or(true, |e| e > self.lbas) {
            return Err(Error::InvalidArgument);
        }
        Ok(())
    }
}

impl BlockDevice for Nvme {
    fn model(&self) -> &str {
        &self.model
    }
    fn serial(&self) -> &str {
        &self.serial
    }
    fn sector_size(&self) -> u32 {
        self.lba_size
    }
    fn sector_count(&self) -> u64 {
        self.lbas
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        let ss = self.lba_size as usize;
        for (i, chunk) in buf.chunks_mut(CHUNK).enumerate() {
            self.rw(false, lba + (i * CHUNK / ss) as u64, chunk.len())?;
            chunk.copy_from_slice(&self.data.as_slice()[..chunk.len()]);
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        let ss = self.lba_size as usize;
        for (i, chunk) in buf.chunks(CHUNK).enumerate() {
            self.data.as_mut_slice()[..chunk.len()].copy_from_slice(chunk);
            self.rw(true, lba + (i * CHUNK / ss) as u64, chunk.len())?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.io.exec(&self.regs, cmd(0x00, 1)).map(|_| ())
    }
}
