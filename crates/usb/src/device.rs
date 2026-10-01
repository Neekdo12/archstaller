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
        self.control_in_type(dev, REQ_IN, req, value, index, buf)
    }

    /// Vendor-specific device-to-host request (bmRequestType 0xC0).
    pub fn vendor_in(
        &mut self,
        dev: &Device,
        req: u8,
        value: u16,
        index: u16,
        buf: &mut [u8],
    ) -> Result<usize> {
        self.control_in_type(dev, REQ_IN | 0x40, req, value, index, buf)
    }

    /// Vendor-specific host-to-device request (bmRequestType 0x40).
    pub fn vendor_out(&mut self, dev: &Device, req: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        self.control_out_type(dev, 0x40, req, value, index, data)
    }

    /// Class-specific interface request, device to host (bmRequestType 0xA1).
    pub fn class_in(&mut self, dev: &Device, req: u8, value: u16, index: u16, buf: &mut [u8]) -> Result<usize> {
        self.control_in_type(dev, 0xa1, req, value, index, buf)
    }

    /// Class-specific interface request, host to device (bmRequestType 0x21).
    pub fn class_out(&mut self, dev: &Device, req: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        self.control_out_type(dev, 0x21, req, value, index, data)
    }

    /// Reads string descriptor `index` (US English).
    pub fn string_descriptor(&mut self, dev: &Device, index: u8) -> Option<String> {
        let mut sbuf = [0u8; 255];
        let n = self.control_in(dev, desc::GET_DESCRIPTOR, (desc::DT_STRING as u16) << 8 | index as u16, 0x0409, &mut sbuf).ok()?;
        desc::parse_string(&sbuf[..n])
    }

    fn control_in_type(
        &mut self,
        dev: &Device,
        req_type: u8,
        req: u8,
        value: u16,
        index: u16,
        buf: &mut [u8],
    ) -> Result<usize> {
        if buf.is_empty() {
            return self.control(dev.slot, setup(req_type, req, value, index, 0), 0, 0, true);
        }
        let dma = Dma::new(buf.len(), 64);
        let n = self.control(dev.slot, setup(req_type, req, value, index, buf.len() as u16), dma.phys(), buf.len(), true)?;
        let n = n.min(buf.len());
        buf[..n].copy_from_slice(&dma.as_slice()[..n]);
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
        self.control_out_type(dev, REQ_OUT, req, value, index, data)
    }

    fn control_out_type(
        &mut self,
        dev: &Device,
        req_type: u8,
        req: u8,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<()> {
        if data.is_empty() {
            self.control(dev.slot, setup(req_type, req, value, index, 0), 0, 0, false)?;
            return Ok(());
        }
        let mut dma = Dma::new(data.len(), 64);
        dma.as_mut_slice().copy_from_slice(data);
        self.control(dev.slot, setup(req_type, req, value, index, data.len() as u16), dma.phys(), data.len(), false)?;
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
        hal::log!("usb: port {port} speed {speed}: enumerating");
        self.enumerate_port_inner(port, speed).map_err(|e| {
            hal::log!("usb: port {port}: enumeration failed: {e:?}");
            e
        })
    }

    fn enumerate_port_inner(&mut self, port: u8, speed: u8) -> Result<Vec<Device>> {
        // Devices (phones especially) need time after port reset before they answer.
        drivers::platform::delay_us(50_000);
        let slot = match self.add_device(port, speed) {
            Ok(s) => s,
            Err(e) => {
                hal::log!("usb: port {port}: address device failed ({e:?}), retrying");
                drivers::platform::delay_us(100_000);
                self.add_device(port, speed)?
            }
        };
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
        let mut tries = 0;
        loop {
            match self.control_in(&probe, desc::GET_DESCRIPTOR, (desc::DT_DEVICE as u16) << 8, 0, &mut head) {
                Ok(_) if head[1] == desc::DT_DEVICE => break,
                r => {
                    tries += 1;
                    hal::log!("usb: port {port}: first descriptor read failed ({r:?}), try {tries}");
                    if tries >= 4 {
                        return Err(Error::Io);
                    }
                    drivers::platform::delay_us(50_000);
                }
            }
        }
        // SuperSpeed devices encode bMaxPacketSize0 as a power of two (9 -> 512).
        let mps = if speed >= 4 { 1u16 << head[7].min(10) } else { head[7] as u16 };
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

        hal::log!("usb: port {port}: {vendor:04x}:{product:04x} with {num_configs} configuration(s)");
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

    /// Selects an alternate setting of an interface (standard SET_INTERFACE).
    pub fn set_interface(&mut self, dev: &Device, iface: u8, alt: u8) -> Result<()> {
        self.control(dev.slot, setup(0x01, 11, alt as u16, iface as u16, 0), 0, 0, false)?;
        Ok(())
    }

    /// Configures the bulk endpoints in `eps` (e.g. those of a second interface, or of an
    /// alternate setting selected after `set_configuration`).
    pub fn configure_endpoints(&mut self, dev: &Device, eps: &[desc::EndpointDesc]) -> Result<()> {
        self.open_bulk_endpoints(dev.slot, dev.configuration, eps)
    }

    /// Sends a zero-length packet on a bulk OUT endpoint (terminates a transfer whose
    /// length is a multiple of the endpoint's max packet size).
    pub fn bulk_write_zlp(&mut self, dev: &Device, ep_addr: u8) -> Result<()> {
        let dci = (ep_addr & 0x0f) * 2;
        self.transfer(dev.slot, dci, 0, 0, 5000)?;
        Ok(())
    }

    /// Queues a read on a bulk IN endpoint without waiting; collect it with `bulk_in_poll`.
    /// `buf` must stay alive and untouched until the read completes.
    pub fn bulk_in_submit(&mut self, dev: &Device, ep_addr: u8, buf: &Dma, len: usize) -> Result<()> {
        let dci = (ep_addr & 0x0f) * 2 + 1;
        self.queue_transfer(dev.slot, dci, buf.phys(), len)
    }

    /// Non-blocking: `Some(Ok(bytes))` when the queued read finished, `Some(Err)` when it
    /// failed (the endpoint is reset), `None` while it is still pending. `len` is the length
    /// that was passed to `bulk_in_submit`.
    pub fn bulk_in_poll(&mut self, dev: &Device, ep_addr: u8, len: usize) -> Option<Result<usize>> {
        let dci = (ep_addr & 0x0f) * 2 + 1;
        let (code, residual) = self.poll_transfer(dev.slot, dci)?;
        if code == 1 || code == 13 {
            Some(Ok(len.saturating_sub(residual as usize)))
        } else {
            let _ = self.reset_endpoint(dev.slot, dci);
            Some(Err(Error::Io))
        }
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
        let n = self.transfer(dev.slot, dci, dma.phys(), buf.len(), timeout_ms)?.min(buf.len());
        buf[..n].copy_from_slice(&dma.as_slice()[..n]);
        Ok(n)
    }
}
