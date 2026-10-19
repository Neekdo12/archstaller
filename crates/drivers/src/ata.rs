//! Legacy ATA (IDE) disks behind a PCI IDE controller, polled. Needed for machines whose SATA
//! controller is set to "IDE" / "compatible" mode in the firmware (class 01:01) instead of AHCI, and
//! for real parallel-ATA disks. Data moves by bus-master DMA (one 32 KiB command at a time) when the
//! controller has it and a test read works, and by PIO otherwise (several MB/s at best, and very slow
//! under virtualization where every port access is a VM exit).
use crate::dma::Dma;
use crate::pci::{Bar, PciDevice};
use crate::platform;
use crate::port::{inb, insw, outb, outl as outl_port, outsw};
use crate::Devices;
use alloc::boxed::Box;
use alloc::string::String;
use hal::{Error, Result};

// Command block registers (offsets from the command base) and control block.
const DATA: u16 = 0;
const SECCNT: u16 = 2;
const LBA_LO: u16 = 3;
const LBA_MID: u16 = 4;
const LBA_HI: u16 = 5;
const DEVICE: u16 = 6;
const STATUS: u16 = 7;
const COMMAND: u16 = 7;

const ST_ERR: u8 = 1;
const ST_DRQ: u8 = 8;
const ST_DF: u8 = 0x20;
const ST_BSY: u8 = 0x80;

const CMD_IDENTIFY: u8 = 0xec;
const CMD_READ: u8 = 0x20;
const CMD_WRITE: u8 = 0x30;
const CMD_READ_EXT: u8 = 0x24;
const CMD_WRITE_EXT: u8 = 0x34;
const CMD_READ_DMA: u8 = 0xc8;
const CMD_WRITE_DMA: u8 = 0xca;
const CMD_READ_DMA_EXT: u8 = 0x25;
const CMD_WRITE_DMA_EXT: u8 = 0x35;
const CMD_FLUSH: u8 = 0xe7;
const CMD_FLUSH_EXT: u8 = 0xea;

/// Sectors per command: 32 KiB, which one physical region descriptor covers and which stays inside one 64 KiB boundary.
const MAX_SECTORS: usize = 64;

// Bus-master registers, offsets from the channel's base (BAR4, +8 for the secondary channel).
const BM_CMD: u16 = 0;
const BM_STATUS: u16 = 2;
const BM_PRD: u16 = 4;
const BM_START: u8 = 1;
const BM_READ: u8 = 8; // direction: device to memory
const BM_ACTIVE: u8 = 1;
const BM_ERROR: u8 = 2;
const BM_INTERRUPT: u8 = 4;

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.class != 0x01 || dev.subclass != 0x01 {
        return;
    }
    dev.enable();
    for channel in 0..2u8 {
        // prog_if bit 0 / bit 2: the primary / secondary channel is in native mode (ports from the BARs);
        // otherwise it answers on the legacy ports.
        let native = dev.prog_if & if channel == 0 { 0x01 } else { 0x04 } != 0;
        let (cmd, ctl) = match (native, channel) {
            (true, c) => match (dev.bar(2 * c), dev.bar(2 * c + 1)) {
                (Some(Bar::Io { port: a, .. }), Some(Bar::Io { port: b, .. })) => (a, b + 2),
                _ => continue,
            },
            (false, 0) => (0x1f0, 0x3f6),
            (false, _) => (0x170, 0x376),
        };
        let bm = match dev.bar(4) {
            Some(Bar::Io { port, .. }) if dev.prog_if & 0x80 != 0 => {
                Some(port + 8 * channel as u16)
            }
            _ => None,
        };
        for drive in 0..2 {
            if let Ok(d) = AtaDisk::new(cmd, ctl, drive, bm) {
                out.block.push(Box::new(d));
            }
        }
    }
}

pub struct AtaDisk {
    cmd: u16,
    ctl: u16,
    drive: u8,
    lba48: bool,
    model: String,
    serial: String,
    sectors: u64,
    dma: Option<DmaEngine>,
}

enum Data<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

/// Bus-master DMA state of one channel.
struct DmaEngine {
    bm: u16,
    prd: Dma,
    data: Dma,
}

fn ata_string(words: &[u16]) -> String {
    let mut bytes = alloc::vec::Vec::new();
    for w in words {
        bytes.push((w >> 8) as u8);
        bytes.push(*w as u8);
    }
    String::from_utf8_lossy(&bytes).trim().into()
}

/// Returns true if word 83 is valid (bits 15:14 == 01b) and indicates 48-bit LBA support (bit 10).
pub fn is_lba48_supported(w83: u16) -> bool {
    (w83 & 0xc000 == 0x4000) && (w83 & (1 << 10) != 0)
}

impl AtaDisk {
    fn reg_in(&self, r: u16) -> u8 {
        // SAFETY: Reading 8-bit register from ATA command I/O port.
        unsafe { inb(self.cmd + r) }
    }
    fn reg_out(&self, r: u16, v: u8) {
        // SAFETY: Writing 8-bit register to ATA command I/O port.
        unsafe { outb(self.cmd + r, v) }
    }
    /// The alternate status register: reading it does not clear a pending interrupt, and four reads
    /// give the device its 400 ns to settle after a drive select or a command.
    fn alt_status(&self) -> u8 {
        // SAFETY: Reading ATA control / alternate status I/O port.
        unsafe { inb(self.ctl) }
    }
    fn settle(&self) {
        for _ in 0..4 {
            self.alt_status();
        }
    }

    fn select(&self, lba_top: u8) {
        self.reg_out(DEVICE, 0xe0 | self.drive << 4 | (lba_top & 0x0f));
        self.settle();
    }

    fn wait_not_busy(&self, timeout_ms: u64) -> Result<u8> {
        let mut st = 0;
        platform::wait_until(timeout_ms, || {
            st = self.alt_status();
            st & ST_BSY == 0
        })?;
        Ok(st)
    }

    /// Waits for the data request, or fails on an error status.
    fn wait_drq(&self) -> Result<()> {
        let mut st = 0;
        platform::wait_until(5000, || {
            st = self.alt_status();
            st & ST_BSY == 0 && (st & (ST_DRQ | ST_ERR | ST_DF) != 0)
        })?;
        if st & (ST_ERR | ST_DF) != 0 {
            return Err(Error::Io);
        }
        Ok(())
    }

    fn new(cmd: u16, ctl: u16, drive: u8, bm: Option<u16>) -> Result<AtaDisk> {
        let mut d = AtaDisk {
            cmd,
            ctl,
            drive,
            lba48: false,
            model: String::new(),
            serial: String::new(),
            sectors: 0,
            dma: None,
        };
        // A floating bus reads 0xff (or 0x7f on some boards): no controller, no device.
        let st = d.alt_status();
        if st == 0xff || st == 0x7f {
            return Err(Error::Unsupported);
        }
        d.select(0);
        // SAFETY: Setting nIEN bit in the ATA control port to disable device interrupts.
        unsafe { outb(ctl, 0x02) }; // nIEN: no interrupts, we poll
        if d.reg_in(STATUS) == 0xff {
            return Err(Error::Unsupported);
        }
        for r in [SECCNT, LBA_LO, LBA_MID, LBA_HI] {
            d.reg_out(r, 0);
        }
        d.reg_out(COMMAND, CMD_IDENTIFY);
        d.settle();
        if d.alt_status() == 0 {
            return Err(Error::Unsupported); // nothing is attached
        }
        d.wait_not_busy(3000)?;
        // A device that is not plain ATA leaves a signature in the LBA registers: ATAPI (CD/DVD) 14/eb,
        // a SATA device behind an emulated port 3c/c3. Skip them.
        if d.reg_in(LBA_MID) != 0 || d.reg_in(LBA_HI) != 0 {
            return Err(Error::Unsupported);
        }
        d.wait_drq()?;
        let mut w = [0u16; 256];
        // SAFETY: Reading 256 16-bit words (512 bytes) from ATA data port into `w`.
        unsafe { insw(cmd + DATA, w.as_mut_ptr(), 256) };
        d.serial = ata_string(&w[10..20]);
        d.model = ata_string(&w[27..47]);
        d.lba48 = is_lba48_supported(w[83]);
        d.sectors = if d.lba48 {
            w[100] as u64 | (w[101] as u64) << 16 | (w[102] as u64) << 32 | (w[103] as u64) << 48
        } else {
            w[60] as u64 | (w[61] as u64) << 16
        };
        // Only 512-byte logical sectors (word 106 bit 12 says they are larger).
        if d.sectors == 0 || (w[106] & 0xc000 == 0x4000 && w[106] & (1 << 12) != 0) {
            return Err(Error::Unsupported);
        }
        // DMA, if the drive says it can (word 49 bit 8) and a real transfer works; else PIO.
        if let (Some(bm), true) = (bm, w[49] & (1 << 8) != 0) {
            let eng = DmaEngine {
                bm,
                prd: Dma::new(16, 64),
                data: Dma::new(MAX_SECTORS * 512, 65536),
            };
            if (eng.prd.phys() | eng.data.phys()) >> 32 == 0 {
                d.dma = Some(eng);
                let mut probe = [0u8; 512];
                if d.dma_transfer(0, Data::Read(&mut probe)).is_err() {
                    d.dma = None;
                    d.reset();
                }
            }
        }
        hal::info!(
            "ata: {} ({} sectors) at {:#x}/{}, {}",
            d.model,
            d.sectors,
            cmd,
            drive,
            if d.dma.is_some() {
                "bus-master DMA"
            } else {
                "PIO"
            }
        );
        Ok(d)
    }

    /// Soft reset of the channel (after a failed DMA test), then wait for the device to come back.
    fn reset(&self) {
        // SAFETY: Writing 0x06 to ATA control port asserts the software reset (SRST).
        unsafe { outb(self.ctl, 0x06) };
        platform::delay_us(10);
        // SAFETY: Writing 0x02 to ATA control port deasserts SRST with interrupts disabled (nIEN).
        unsafe { outb(self.ctl, 0x02) };
        platform::delay_us(5000);
        let _res = self.wait_not_busy(5000);
    }

    /// One DMA command of up to `MAX_SECTORS` sectors through the bounce buffer. For a write the
    /// data in `buf` is copied in first; for a read it is copied out afterwards.
    fn dma_transfer(&mut self, lba: u64, mut data: Data) -> Result<()> {
        let write = matches!(data, Data::Write(_));
        let len = match &data {
            Data::Read(b) => b.len(),
            Data::Write(b) => b.len(),
        };
        let n = len / 512;
        let (cmd_byte, bm) = {
            let e = self.dma.as_ref().ok_or(Error::Unsupported)?;
            (
                if self.lba48 {
                    if write {
                        CMD_WRITE_DMA_EXT
                    } else {
                        CMD_READ_DMA_EXT
                    }
                } else if write {
                    CMD_WRITE_DMA
                } else {
                    CMD_READ_DMA
                },
                e.bm,
            )
        };
        {
            let e = self.dma.as_mut().ok_or(Error::Unsupported)?;
            if let Data::Write(b) = &data {
                let Some(dest) = e.data.as_mut_slice().get_mut(..len) else {
                    return Err(Error::InvalidArgument);
                };
                dest.copy_from_slice(b);
            }
            // One region descriptor: address, byte count (0 would mean 64 KiB), end-of-table flag.
            let prd = e.prd.as_ptr() as *mut u32;
            // SAFETY: Writing PRD entry base physical address.
            unsafe { prd.write_volatile(e.data.phys() as u32) };
            // SAFETY: Calculating pointer to PRD entry byte count/flags field.
            let prd_len = unsafe { prd.add(1) };
            // SAFETY: Writing PRD entry byte count and end-of-table flag (0x8000_0000).
            unsafe { prd_len.write_volatile(len as u32 | 0x8000_0000) };
            // SAFETY: Programming PRD table physical address into the Bus Master PRD register.
            unsafe { outl_port(bm + BM_PRD, e.prd.phys() as u32) };
            // SAFETY: Setting transfer direction in the Bus Master command register.
            unsafe { outb(bm + BM_CMD, if write { 0 } else { BM_READ }) };
            // SAFETY: Reading Bus Master status register.
            let st = unsafe { inb(bm + BM_STATUS) };
            // SAFETY: Clearing error and interrupt status bits by writing 1s to Bus Master status register.
            unsafe { outb(bm + BM_STATUS, (st & 0x60) | BM_ERROR | BM_INTERRUPT) };
        }
        self.issue_dma(lba, n, cmd_byte)?;
        // SAFETY: Starting Bus Master DMA transfer on BM command port.
        unsafe { outb(bm + BM_CMD, (if write { 0 } else { BM_READ }) | BM_START) };
        let done = platform::wait_until(10_000, || {
            // SAFETY: Polling Bus Master status register for active transfer flag.
            let st = unsafe { inb(bm + BM_STATUS) };
            st & BM_ACTIVE == 0 && self.alt_status() & ST_BSY == 0
        });
        // SAFETY: Reading Bus Master status after DMA completion.
        let bm_status = unsafe { inb(bm + BM_STATUS) };
        // SAFETY: Stopping Bus Master DMA on command port.
        unsafe { outb(bm + BM_CMD, 0) };
        done?;
        let ata = self.alt_status();
        if bm_status & BM_ERROR != 0 || ata & (ST_ERR | ST_DF) != 0 {
            return Err(Error::Io);
        }
        if let Data::Read(b) = &mut data {
            let Some(dma) = self.dma.as_ref() else {
                return Err(Error::Unsupported);
            };
            let Some(src) = dma.data.as_slice().get(..len) else {
                return Err(Error::Io);
            };
            b.copy_from_slice(src);
        }
        Ok(())
    }

    fn issue_dma(&self, lba: u64, count: usize, cmd: u8) -> Result<()> {
        self.wait_not_busy(1000)?;
        if self.lba48 {
            self.select(0);
            self.reg_out(SECCNT, (count >> 8) as u8);
            self.reg_out(LBA_LO, (lba >> 24) as u8);
            self.reg_out(LBA_MID, (lba >> 32) as u8);
            self.reg_out(LBA_HI, (lba >> 40) as u8);
            self.reg_out(SECCNT, count as u8);
            self.reg_out(LBA_LO, lba as u8);
            self.reg_out(LBA_MID, (lba >> 8) as u8);
            self.reg_out(LBA_HI, (lba >> 16) as u8);
        } else {
            self.select((lba >> 24) as u8);
            self.reg_out(SECCNT, count as u8);
            self.reg_out(LBA_LO, lba as u8);
            self.reg_out(LBA_MID, (lba >> 8) as u8);
            self.reg_out(LBA_HI, (lba >> 16) as u8);
        }
        self.reg_out(COMMAND, cmd);
        self.settle();
        Ok(())
    }

    /// Starts a read or write of `count` sectors at `lba` and waits until the device took the command.
    fn issue(&self, lba: u64, count: usize, write: bool) -> Result<()> {
        self.wait_not_busy(1000)?;
        if self.lba48 {
            self.select(0);
            // High bytes first, then low: the 48-bit register protocol.
            self.reg_out(SECCNT, (count >> 8) as u8);
            self.reg_out(LBA_LO, (lba >> 24) as u8);
            self.reg_out(LBA_MID, (lba >> 32) as u8);
            self.reg_out(LBA_HI, (lba >> 40) as u8);
            self.reg_out(SECCNT, count as u8);
            self.reg_out(LBA_LO, lba as u8);
            self.reg_out(LBA_MID, (lba >> 8) as u8);
            self.reg_out(LBA_HI, (lba >> 16) as u8);
            self.reg_out(COMMAND, if write { CMD_WRITE_EXT } else { CMD_READ_EXT });
        } else {
            self.select((lba >> 24) as u8);
            self.reg_out(SECCNT, count as u8);
            self.reg_out(LBA_LO, lba as u8);
            self.reg_out(LBA_MID, (lba >> 8) as u8);
            self.reg_out(LBA_HI, (lba >> 16) as u8);
            self.reg_out(COMMAND, if write { CMD_WRITE } else { CMD_READ });
        }
        self.settle();
        Ok(())
    }

    fn check(&self, lba: u64, len: usize) -> Result<()> {
        if !len.is_multiple_of(512)
            || lba
                .checked_add((len / 512) as u64)
                .is_none_or(|e| e > self.sectors)
            || (!self.lba48 && lba + (len / 512) as u64 > 1 << 28)
        {
            return Err(Error::InvalidArgument);
        }
        Ok(())
    }
}

impl hal::BlockDevice for AtaDisk {
    fn model(&self) -> &str {
        &self.model
    }
    fn serial(&self) -> &str {
        &self.serial
    }
    fn sector_size(&self) -> u32 {
        512
    }
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        let mut done = 0;
        while done < buf.len() / 512 {
            let n = (buf.len() / 512 - done).min(MAX_SECTORS);
            let start = done * 512;
            let end = (done + n) * 512;
            if self.dma.is_some() {
                let Some(sub) = buf.get_mut(start..end) else {
                    return Err(Error::InvalidArgument);
                };
                self.dma_transfer(
                    lba + done as u64,
                    Data::Read(sub),
                )?;
                done += n;
                continue;
            }
            self.issue(lba + done as u64, n, false)?;
            for s in 0..n {
                self.wait_drq()?;
                let at = (done + s) * 512;
                let Some(sector_buf) = buf.get_mut(at..at + 512) else {
                    return Err(Error::InvalidArgument);
                };
                // SAFETY: insw reads 256 16-bit words (512 bytes) from ATA data port into sector_buf.
                unsafe {
                    insw(
                        self.cmd + DATA,
                        sector_buf.as_mut_ptr() as *mut u16,
                        256,
                    )
                };
            }
            done += n;
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        let mut done = 0;
        while done < buf.len() / 512 {
            let n = (buf.len() / 512 - done).min(MAX_SECTORS);
            let start = done * 512;
            let end = (done + n) * 512;
            if self.dma.is_some() {
                let Some(sub) = buf.get(start..end) else {
                    return Err(Error::InvalidArgument);
                };
                self.dma_transfer(
                    lba + done as u64,
                    Data::Write(sub),
                )?;
                done += n;
                continue;
            }
            self.issue(lba + done as u64, n, true)?;
            for s in 0..n {
                self.wait_drq()?;
                let at = (done + s) * 512;
                let Some(sector_buf) = buf.get(at..at + 512) else {
                    return Err(Error::InvalidArgument);
                };
                // SAFETY: outsw writes 256 16-bit words (512 bytes) from sector_buf to ATA data port.
                unsafe {
                    outsw(
                        self.cmd + DATA,
                        sector_buf.as_ptr() as *const u16,
                        256,
                    )
                };
            }
            // The device is done with the last sector when BSY drops.
            let st = self.wait_not_busy(5000)?;
            if st & (ST_ERR | ST_DF) != 0 {
                return Err(Error::Io);
            }
            done += n;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.wait_not_busy(1000)?;
        self.select(0);
        self.reg_out(COMMAND, if self.lba48 { CMD_FLUSH_EXT } else { CMD_FLUSH });
        self.settle();
        let st = self.wait_not_busy(30_000)?;
        if st & (ST_ERR | ST_DF) != 0 {
            return Err(Error::Io);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lba48_word83_validation() {
        assert!(!is_lba48_supported(0xffff));
        assert!(!is_lba48_supported(0x0000));
        assert!(!is_lba48_supported(0x4000));
        assert!(!is_lba48_supported(0x8000 | (1 << 10)));
        assert!(is_lba48_supported(0x4000 | (1 << 10)));
    }
}

