//! Device enumeration and the public control/bulk transfer API.
use crate::desc::{self, InterfaceDesc};
use crate::xhci::Controller;
use alloc::string::String;
use alloc::vec::Vec;
use drivers::dma::Dma;
use hal::{Error, Result};

const REQ_IN: u8 = 0x80;
const REQ_OUT: u8 = 0x00;

/// One attached USB device that has been addressed and had its descriptors read.
pub struct Device {
    /// xHCI slot id (internal).
    pub(crate) slot: u8,
    pub vendor: u16,
    pub product: u16,
    pub serial: String,
    /// bConfigurationValue of the (single) configuration.
    pub configuration: u8,
    pub interfaces: Vec<InterfaceDesc>,
}

impl Device {
    /// Bulk OUT endpoint addresses of interface `iface` (first match wins).
    pub fn bulk_in(&self, iface: &InterfaceDesc) -> Option<u8> {
        iface.endpoints.iter().find(|e| e.is_bulk() && e.is_in()).map(|e| e.addr)
    }
    pub fn bulk_out(&self, iface: &InterfaceDesc) -> Option<u8> {
        iface.endpoints.iter().find(|e| e.is_bulk() && !e.is_in()).map(|e| e.addr)
    }
}

fn setup(req_type: u8, req: u8, value: u16, index: u16, len: u16) -> [u8; 8] {
    let mut s = [0u8; 8];
    s[0] = req_type;
    s[1] = req;
    s[2..4].copy_from_slice(&value.to_le_bytes());
    s[4..6].copy_from_slice(&index.to_le_bytes());
    s[6..8].copy_from_slice(&len.to_le_bytes());
    s
}

impl Controller {
    /// Standard control transfer with a device-to-host data stage.
    pub fn control_in(
        &mut self,
        dev: &Device,
        req: u8,
        value: u16,
        index: u16,
        buf: &mut [u8],
    ) -> Result<usize> {
        if buf.is_empty() {
            return self.control(dev.slot, setup(REQ_IN, req, value, index, 0), 0, 0, true);
        }
        let dma = Dma::new(buf.len(), 64);
        let n = self.control(dev.slot, setup(REQ_IN, req, value, index, buf.len() as u16), dma.phys(), buf.len(), true)?;
        buf.copy_from_slice(&dma.as_slice()[..n.min(buf.len())]);
        Ok(n)
    }

    /// Standard control transfer with a host-to-device (or no) data stage.
    pub fn control_out(
        &mut self,
        dev: &Device,
        req: u8,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<()> {
        if data.is_empty() {
            self.control(dev.slot, setup(REQ_OUT, req, value, index, 0), 0, 0, false)?;
            return Ok(());
        }
        let mut dma = Dma::new(data.len(), 64);
        dma.as_mut_slice().copy_from_slice(data);
        self.control(dev.slot, setup(REQ_OUT, req, value, index, data.len() as u16), dma.phys(), data.len(), false)?;
        Ok(())
    }

    fn get_descriptor(&mut self, dev: &Device, ty: u8, index: u8, buf: &mut [u8]) -> Result<usize> {
        self.control_in(dev, desc::GET_DESCRIPTOR, (ty as u16) << 8 | index as u16, 0, buf)
    }

    /// Enumerates every enabled port and returns addressed, descriptor-read devices.
    pub fn enumerate(&mut self) -> Result<Vec<Device>> {
        let mut out = Vec::new();
        for (port, speed) in self.enabled_ports() {
            match self.enumerate_port(port, speed) {
                Ok(devices) => out.extend(devices),
                Err(_) => continue, // a port that won't enumerate is not fatal
            }
        }
        Ok(out)
    }

    fn enumerate_port(&mut self, port: u8, speed: u8) -> Result<Vec<Device>> {
        let slot = self.add_device(port, speed)?;
        let probe = Device {
            slot,
            vendor: 0,
            product: 0,
            serial: String::new(),
            configuration: 1,
            interfaces: Vec::new(),
        };
        // First 8 bytes are enough to fix up EP0's real max packet size.
        let mut head = [0u8; 8];
        self.control_in(&probe, desc::GET_DESCRIPTOR, (desc::DT_DEVICE as u16) << 8, 0, &mut head)?;
        if head[1] != desc::DT_DEVICE {
            return Err(Error::Io);
        }
        let mps = head[7] as u16;
        if mps != 0 {
            self.set_ep0_mps(slot, mps)?;
        }
        let mut ddesc = [0u8; 18];
        self.get_descriptor(&probe, desc::DT_DEVICE, 0, &mut ddesc)?;
        let vendor = u16::from_le_bytes([ddesc[8], ddesc[9]]);
        let product = u16::from_le_bytes([ddesc[10], ddesc[11]]);
        let num_configs = ddesc[17];

        let mut serial = String::new();
        let iserial = ddesc[16];
        if iserial != 0 {
            let mut sbuf = [0u8; 255];
            if self.get_descriptor(&probe, desc::DT_STRING, iserial, &mut sbuf).is_ok() {
                if let Some(s) = desc::parse_string(&sbuf) {
                    serial = s;
                }
            }
        }

        if num_configs == 0 {
            return Err(Error::Unsupported);
        }
        let mut devices = Vec::new();
        for config_index in 0..num_configs {
            let mut chead = [0u8; 9];
            if self.get_descriptor(&probe, desc::DT_CONFIG, config_index, &mut chead).is_err()
                || chead[1] != desc::DT_CONFIG
            {
                continue;
            }
            let total = u16::from_le_bytes([chead[2], chead[3]]) as usize;
            if total < 9 || total > 4096 {
                continue;
            }
            let mut cblob = alloc::vec![0u8; total];
            if self.get_descriptor(&probe, desc::DT_CONFIG, config_index, &mut cblob).is_err()
                || cblob[1] != desc::DT_CONFIG
            {
                continue;
            }
            devices.push(Device {
                slot,
                vendor,
                product,
                serial: serial.clone(),
                configuration: cblob[5],
                interfaces: desc::parse_config(&cblob),
            });
        }
        if devices.is_empty() {
            return Err(Error::Unsupported);
        }
        Ok(devices)
    }

    /// SET_CONFIGURATION + xHCI Configure Endpoint for the interface's bulk endpoints.
    pub fn set_configuration(
        &mut self,
        dev: &mut Device,
        config: u8,
        iface: &InterfaceDesc,
    ) -> Result<()> {
        self.control_out(dev, desc::SET_CONFIGURATION, config as u16, 0, &[])?;
        let eps: Vec<desc::EndpointDesc> =
            iface.endpoints.iter().filter(|e| e.is_bulk()).cloned().collect();
        if !eps.is_empty() {
            self.open_bulk_endpoints(dev.slot, config, &eps)?;
        }
        Ok(())
    }

    /// Writes `data` to a bulk OUT endpoint. Blocks until the device takes it.
    pub fn bulk_write(&mut self, dev: &Device, ep_addr: u8, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let mut dma = Dma::new(data.len(), 64);
        dma.as_mut_slice().copy_from_slice(data);
        let dci = (ep_addr & 0x0f) * 2;
        let n = self.transfer(dev.slot, dci, dma.phys(), data.len(), 5000)?;
        if n != data.len() {
            return Err(Error::Io);
        }
        Ok(())
    }

    /// Reads up to `buf.len()` bytes from a bulk IN endpoint; returns the actual count
    /// (a short packet is a normal end of transfer). Times out after `timeout_ms`.
    pub fn bulk_read(
        &mut self,
        dev: &Device,
        ep_addr: u8,
        buf: &mut [u8],
        timeout_ms: u64,
    ) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let dma = Dma::new(buf.len(), 64);
        let dci = (ep_addr & 0x0f) * 2 + 1;
        let n = self.transfer(dev.slot, dci, dma.phys(), buf.len(), timeout_ms)?;
        buf.copy_from_slice(&dma.as_slice()[..n]);
        Ok(n)
    }
}
