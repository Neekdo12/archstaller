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
        let Some(regs) = dev.map_bar(0) else {
            hal::log!("usb: xHCI {:04x}:{:04x}: BAR0 not mappable", dev.vendor, dev.device);
            continue;
        };
        dev.enable();
        match Controller::new(regs) {
            Ok(c) => {
                hal::log!("usb: xHCI {:04x}:{:04x} initialised", dev.vendor, dev.device);
                out.push(c);
            }
            Err(e) => hal::log!("usb: xHCI {:04x}:{:04x} init failed: {e:?}", dev.vendor, dev.device),
        }
    }
    out
}

/// Finds an attached device matching `vendor`/`product` (0 = wildcard) across all
/// controllers, configures its configuration and claims the endpoints of the first
/// interface whose class triple matches `(class, subclass, protocol)` (0xff = wildcard).
///
/// A device may expose several configurations (each is returned by `enumerate` as its own
/// `Device`). When `also` is given, a configuration that additionally contains an interface
/// of that class triple with bulk endpoints is preferred; otherwise the first match wins.
pub fn find_device(
    vendor: u16,
    product: u16,
    class: u8,
    subclass: u8,
    protocol: u8,
    also: Option<(u8, u8, u8)>,
) -> Result<(Controller, Device)> {
    let triple = |i: &desc::InterfaceDesc, c: u8, s: u8, p: u8| {
        (c == 0xff || i.class == c) && (s == 0xff || i.subclass == s) && (p == 0xff || i.protocol == p)
    };
    let mut ctrls = controllers();
    let mut found: Vec<(usize, Device)> = Vec::new();
    for (ci, ctrl) in ctrls.iter_mut().enumerate() {
        for dev in ctrl.enumerate()? {
            hal::log!(
                "usb: device {:04x}:{:04x} config {} interfaces {:?}",
                dev.vendor,
                dev.product,
                dev.configuration,
                dev.interfaces.iter().map(|i| (i.num, i.alt, i.class, i.subclass, i.protocol)).collect::<Vec<_>>()
            );
            if (vendor == 0 || dev.vendor == vendor)
                && (product == 0 || dev.product == product)
                && dev.interfaces.iter().any(|i| triple(i, class, subclass, protocol))
            {
                found.push((ci, dev));
            }
        }
    }
    let has_also = |d: &Device| {
        also.is_none_or(|(c, s, p)| {
            d.interfaces.iter().any(|i| triple(i, c, s, p) && d.bulk_in(i).is_some() && d.bulk_out(i).is_some())
        })
    };
    let pick = found.iter().position(|(_, d)| has_also(d)).or(if found.is_empty() { None } else { Some(0) });
    let Some(pick) = pick else {
        hal::log!("usb: no device matched vendor {vendor:04x} class {class:02x}/{subclass:02x}/{protocol:02x}");
        return Err(Error::Unsupported);
    };
    let (ci, mut dev) = found.swap_remove(pick);
    let mut ctrl = ctrls.swap_remove(ci);
    let iface = dev.interfaces.iter().find(|i| triple(i, class, subclass, protocol)).cloned().ok_or(Error::Unsupported)?;
    let configuration = dev.configuration;
    hal::log!("usb: using configuration {configuration} of {:04x}:{:04x}", dev.vendor, dev.product);
    ctrl.set_configuration(&mut dev, configuration, &iface)?;
    Ok((ctrl, dev))
}
