//! USB for the installer: a polled xHCI host controller driver and just enough USB core
//! (descriptors, control transfers, bulk transfers) to talk to one directly attached device.
//! No hubs, no hot-plug beyond "present at boot", no interrupt/isochronous endpoints.
//!
//! Everything is synchronous with timeouts, matching the style of the other drivers
//! (see `docs/iphone-tethering.md` for the design this comes from).
#![no_std]

extern crate alloc;

pub mod desc;
mod device;
#[doc(hidden)] // ring internals are public for host-side unit tests
pub mod xhci;

pub use device::Device;
pub use xhci::Controller;

use alloc::vec::Vec;
use hal::{Error, Result};

/// Finds every xHCI controller (PCI class 0x0C/0x03/0x30) and initializes it.
/// Controllers that fail to initialize are skipped.
pub fn controllers() -> Vec<Controller> {
    let mut out = Vec::new();
    for dev in drivers::pci::enumerate() {
        if dev.class != 0x0c || dev.subclass != 0x03 || dev.prog_if != 0x30 {
            continue;
        }
        let Some(regs) = dev.map_bar(0) else { continue };
        dev.enable();
        if let Ok(c) = Controller::new(regs) {
            out.push(c);
        }
    }
    out
}

/// Finds the first attached device matching `vendor`/`product` (0 = wildcard) across all
/// controllers, configures `config` and claims the endpoints of the first interface whose
/// class triple matches `(class, subclass, protocol)` (0xff = wildcard).
pub fn find_device(
    vendor: u16,
    product: u16,
    class: u8,
    subclass: u8,
    protocol: u8,
) -> Result<(Controller, Device)> {
    for mut ctrl in controllers() {
        for mut dev in ctrl.enumerate()? {
            if vendor != 0 && dev.vendor != vendor {
                continue;
            }
            if product != 0 && dev.product != product {
                continue;
            }
            let Some(iface) = dev
                .interfaces
                .iter()
                .find(|i| {
                    (class == 0xff || i.class == class)
                        && (subclass == 0xff || i.subclass == subclass)
                        && (protocol == 0xff || i.protocol == protocol)
                })
                .cloned()
            else {
                continue;
            };
            let configuration = dev.configuration;
            ctrl.set_configuration(&mut dev, configuration, &iface)?;
            return Ok((ctrl, dev));
        }
    }
    Err(Error::Unsupported)
}
