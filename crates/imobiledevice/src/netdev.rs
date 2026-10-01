//! The iPhone's tethering data path as a `hal::NetDevice`. Apple's USB tethering is not
//! a lockdown service: the phone exposes an `ipheth` interface (class ff, subclass fd,
//! protocol 01) whose second alternate setting has one bulk IN and one bulk OUT endpoint
//! carrying plain Ethernet frames (received frames are preceded by 2 bytes of padding).
//! Vendor request 0x00 returns the MAC address the phone expects us to use and 0x45
//! reports the carrier state (0x04 = hotspot link up). The phone only turns this on once
//! the host is paired (trusted), which is what `lib.rs`'s pairing step is for.
use alloc::vec::Vec;
use drivers::dma::Dma;
use hal::{Error, NetDevice, Result};

pub const IPHETH_CLASS: (u8, u8, u8) = (0xff, 0xfd, 0x01);
const CMD_GET_MAC: u8 = 0x00;
const CMD_CARRIER: u8 = 0x45;
const CARRIER_ON: u8 = 0x04;
/// Padding the phone puts in front of every received frame.
const RX_PAD: usize = 2;
const RX_BUF: usize = 2048;

pub struct IphoneNet {
    ctrl: usb::Controller,
    dev: usb::Device,
    ep_in: u8,
    ep_out: u8,
    mac: [u8; 6],
    rx: Dma,
    rx_pending: bool,
}

impl IphoneNet {
    /// Switches the phone's tethering interface on and reads its MAC. Does not wait for
    /// the carrier (`carrier` / `wait_carrier`).
    pub fn open(mut ctrl: usb::Controller, dev: usb::Device) -> Result<IphoneNet> {
        // The data interface has an empty alternate setting 0 and the endpoints in alt 1.
        let Some(iface) = dev
            .interfaces
            .iter()
            .find(|i| (i.class, i.subclass, i.protocol) == IPHETH_CLASS && dev.bulk_in(i).is_some() && dev.bulk_out(i).is_some())
            .cloned()
        else {
            hal::log!(
                "usb: no tethering (ff/fd/01) interface with bulk endpoints; interfaces: {:?}",
                dev.interfaces.iter().map(|i| (i.num, i.alt, i.class, i.subclass, i.protocol, i.endpoints.len())).collect::<Vec<_>>()
            );
            return Err(Error::Unsupported);
        };
        let ep_in = dev.bulk_in(&iface).ok_or(Error::Unsupported)?;
        let ep_out = dev.bulk_out(&iface).ok_or(Error::Unsupported)?;
        ctrl.set_interface(&dev, iface.num, iface.alt)?;
        let eps: Vec<usb::desc::EndpointDesc> = iface.endpoints.iter().filter(|e| e.is_bulk()).cloned().collect();
        ctrl.configure_endpoints(&dev, &eps)?;

        let mut buf = [0u8; 0x40];
        let n = ctrl.vendor_in(&dev, CMD_GET_MAC, 0, 0, &mut buf)?;
        if n < 6 {
            hal::log!("usb: tethering MAC request returned {n} bytes");
            return Err(Error::Io);
        }
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&buf[..6]);
        Ok(IphoneNet { ctrl, dev, ep_in, ep_out, mac, rx: Dma::new(RX_BUF, 64), rx_pending: false })
    }

    /// Whether the phone reports the hotspot link as up.
    pub fn carrier(&mut self) -> bool {
        let mut buf = [0u8; 0x40];
        matches!(self.ctrl.vendor_in(&self.dev, CMD_CARRIER, 0, 0, &mut buf), Ok(n) if n >= 1 && buf[0] == CARRIER_ON)
    }
}

impl NetDevice for IphoneNet {
    fn name(&self) -> &str {
        "iphone-usb"
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        self.carrier()
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        self.ctrl.bulk_write(&self.dev, self.ep_out, frame)
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        if !self.rx_pending {
            self.ctrl.bulk_in_submit(&self.dev, self.ep_in, &self.rx, RX_BUF).ok()?;
            self.rx_pending = true;
        }
        match self.ctrl.bulk_in_poll(&self.dev, self.ep_in, RX_BUF)? {
            Ok(n) => {
                self.rx_pending = false;
                if n <= RX_PAD {
                    return None;
                }
                let frame = &self.rx.as_slice()[RX_PAD..n];
                let len = frame.len().min(buf.len());
                buf[..len].copy_from_slice(&frame[..len]);
                Some(len)
            }
            Err(_) => {
                self.rx_pending = false;
                None
            }
        }
    }
}
