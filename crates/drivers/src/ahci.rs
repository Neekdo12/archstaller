//! AHCI SATA disks, one command slot, polled.
use crate::dma::Dma;
use crate::mmio::Mmio;
use crate::pci::PciDevice;
use crate::{platform, Devices};
use alloc::boxed::Box;
use alloc::string::String;
use hal::{BlockDevice, Error, Result};

const GHC: usize = 0x04;
const PI: usize = 0x0c;
const CAP: usize = 0x00;
const GHC_AE: u32 = 1 << 31;
const CAP_S64A: u32 = 1 << 31;

const P_CLB: usize = 0x00;
const P_FB: usize = 0x08;
const P_IS: usize = 0x10;
const P_CMD: usize = 0x18;
const P_TFD: usize = 0x20;
const P_SIG: usize = 0x24;
const P_SSTS: usize = 0x28;
const P_SERR: usize = 0x30;
const P_CI: usize = 0x38;

const CMD_ST: u32 = 1;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;

const TFD_ERR: u32 = 1;
const TFD_DRQ: u32 = 1 << 3;
const TFD_DF: u32 = 1 << 5;
const TFD_BSY: u32 = 1 << 7;
const IS_TFES: u32 = 1 << 30;

const SIG_ATA: u32 = 0x0000_0101;

const ATA_READ_DMA_EXT: u8 = 0x25;
const ATA_WRITE_DMA_EXT: u8 = 0x35;
const ATA_FLUSH_EXT: u8 = 0xea;
const ATA_IDENTIFY: u8 = 0xec;

const CHUNK: usize = 128 * 1024;

pub fn probe(dev: &PciDevice, out: &mut Devices) {
    if dev.class != 0x01 || dev.subclass != 0x06 || dev.prog_if != 0x01 {
        return;
    }
    let Some(hba) = dev.map_bar(5) else { return };
    dev.enable();
    hba.write32(GHC, hba.read32(GHC) | GHC_AE);
    let s64 = hba.read32(CAP) & CAP_S64A != 0;
    let implemented = hba.read32(PI);
    for n in 0..32 {
        if implemented & (1 << n) == 0 {
            continue;
        }
        let port = hba.offset(0x100 + 0x80 * n);
        if let Ok(d) = AhciDisk::new(port, s64) {
            out.block.push(Box::new(d));
        }
    }
}

pub struct AhciDisk {
    port: Mmio,
    /// 32 command headers (1 KiB) followed by the 256-byte received-FIS area.
    _lists: Dma,
    /// Command table: CFIS at 0, PRDT at 0x80.
    table: Dma,
    data: Dma,
    model: String,
    serial: String,
    sector_size: u32,
    sectors: u64,
}

fn ata_string(words: &[u16]) -> String {
    let mut bytes = alloc::vec::Vec::new();
    for w in words {
        bytes.push((w >> 8) as u8);
        bytes.push(*w as u8);
    }
    String::from_utf8_lossy(&bytes).trim().into()
}

impl AhciDisk {
    fn new(port: Mmio, s64: bool) -> Result<AhciDisk> {
        let ssts = port.read32(P_SSTS);
        if ssts & 0xf != 3 || (ssts >> 8) & 0xf != 1 || port.read32(P_SIG) != SIG_ATA {
            return Err(Error::Unsupported);
        }
        stop(&port)?;
        let lists = Dma::new(1024 + 256, 1024);
        let table = Dma::new(0x80 + 16, 128);
        let data = Dma::new(CHUNK, 4096);
        if !s64 && (lists.phys() | table.phys() | data.phys()) >> 32 != 0 {
            return Err(Error::Unsupported);
        }
        port.write64(P_CLB, lists.phys());
        port.write64(P_FB, lists.phys_at(1024));
        port.write32(P_SERR, 0xffff_ffff);
        port.write32(P_IS, 0xffff_ffff);
        port.write32(P_CMD, port.read32(P_CMD) | CMD_FRE);
        port.write32(P_CMD, port.read32(P_CMD) | CMD_ST);

        let mut d = AhciDisk {
            port,
            _lists: lists,
            table,
            data,
            model: String::new(),
            serial: String::new(),
            sector_size: 512,
            sectors: 0,
        };
        d.identify()?;
        Ok(d)
    }

    fn identify(&mut self) -> Result<()> {
        self.command(ATA_IDENTIFY, 0, 0, 512, false)?;
        let mut w = [0u16; 256];
        for (i, v) in w.iter_mut().enumerate() {
            *v = u16::from_le_bytes([self.data.as_slice()[2 * i], self.data.as_slice()[2 * i + 1]]);
        }
        self.serial = ata_string(&w[10..20]);
        self.model = ata_string(&w[27..47]);
        self.sectors = if w[83] & (1 << 10) != 0 {
            w[100] as u64 | (w[101] as u64) << 16 | (w[102] as u64) << 32 | (w[103] as u64) << 48
        } else {
            w[60] as u64 | (w[61] as u64) << 16
        };
        if w[106] & 0xc000 == 0x4000 && w[106] & (1 << 12) != 0 {
            let words = w[117] as u32 | (w[118] as u32) << 16;
            self.sector_size = words * 2;
        }
        if self.sector_size != 512 && self.sector_size != 4096 {
            return Err(Error::Unsupported);
        }
        Ok(())
    }

    /// Issues one command in slot 0 using the bounce buffer for `len` bytes of data.
    fn command(&mut self, cmd: u8, lba: u64, count: u32, len: usize, write: bool) -> Result<()> {
        let p = self.port;
        platform::wait_until(1000, || p.read32(P_TFD) & (TFD_BSY | TFD_DRQ) == 0)?;

        let t = Mmio::new(self.table.as_ptr());
        for i in 0..5 {
            t.write32(4 * i, 0);
        }
        t.write8(0, 0x27); // register H2D FIS
        t.write8(1, 0x80); // command bit
        t.write8(2, cmd);
        t.write8(4, lba as u8);
        t.write8(5, (lba >> 8) as u8);
        t.write8(6, (lba >> 16) as u8);
        t.write8(7, 0x40); // LBA mode
        t.write8(8, (lba >> 24) as u8);
        t.write8(9, (lba >> 32) as u8);
        t.write8(10, (lba >> 40) as u8);
        t.write8(12, count as u8);
        t.write8(13, (count >> 8) as u8);

        let prdtl = if len > 0 { 1u32 } else { 0 };
        if len > 0 {
            t.write64(0x80, self.data.phys());
            t.write32(0x88, 0);
            t.write32(0x8c, (len as u32 - 1) | 1 << 31);
        }
        let h = Mmio::new(self._lists.as_ptr());
        h.write32(0, 5 | if write { 1 << 6 } else { 0 } | prdtl << 16);
        h.write32(4, 0);
        h.write64(8, self.table.phys());

        p.write32(P_IS, 0xffff_ffff);
        p.write32(P_CI, 1);
        platform::wait_until(5000, || p.read32(P_CI) & 1 == 0 || p.read32(P_IS) & IS_TFES != 0)?;
        if p.read32(P_IS) & IS_TFES != 0 || p.read32(P_TFD) & (TFD_ERR | TFD_DF) != 0 {
            return Err(Error::Io);
        }
        Ok(())
    }

    fn check(&self, lba: u64, len: usize) -> Result<()> {
        let ss = self.sector_size as usize;
        if len % ss != 0 || lba.checked_add((len / ss) as u64).map_or(true, |e| e > self.sectors) {
            return Err(Error::InvalidArgument);
        }
        Ok(())
    }
}

fn stop(port: &Mmio) -> Result<()> {
    port.write32(P_CMD, port.read32(P_CMD) & !CMD_ST);
    platform::wait_until(500, || port.read32(P_CMD) & CMD_CR == 0)?;
    port.write32(P_CMD, port.read32(P_CMD) & !CMD_FRE);
    platform::wait_until(500, || port.read32(P_CMD) & CMD_FR == 0)
}

impl BlockDevice for AhciDisk {
    fn model(&self) -> &str {
        &self.model
    }
    fn serial(&self) -> &str {
        &self.serial
    }
    fn sector_size(&self) -> u32 {
        self.sector_size
    }
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        let ss = self.sector_size as usize;
        for (i, chunk) in buf.chunks_mut(CHUNK).enumerate() {
            let l = lba + (i * CHUNK / ss) as u64;
            self.command(ATA_READ_DMA_EXT, l, (chunk.len() / ss) as u32, chunk.len(), false)?;
            chunk.copy_from_slice(&self.data.as_slice()[..chunk.len()]);
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        let ss = self.sector_size as usize;
        for (i, chunk) in buf.chunks(CHUNK).enumerate() {
            let l = lba + (i * CHUNK / ss) as u64;
            self.data.as_mut_slice()[..chunk.len()].copy_from_slice(chunk);
            self.command(ATA_WRITE_DMA_EXT, l, (chunk.len() / ss) as u32, chunk.len(), true)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.command(ATA_FLUSH_EXT, 0, 0, 0, false)
    }
}
