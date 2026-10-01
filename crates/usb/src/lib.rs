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

/// Every device found on every xHCI controller, enumerated once. A device may expose several
/// configurations (each is its own `Device`, all on the same physical device); `take` picks
/// one, configures it and hands over its controller. Controllers are reset when scanned, so
/// scan once and let each consumer try in turn.
pub struct Scan {
    ctrls: Vec<Option<Controller>>,
    found: Vec<(usize, Device)>,
}

impl Scan {
    pub fn new() -> Scan {
        let mut ctrls: Vec<Option<Controller>> = controllers().into_iter().map(Some).collect();
        let mut found = Vec::new();
        for (ci, ctrl) in ctrls.iter_mut().enumerate() {
            let Some(ctrl) = ctrl else { continue };
            match ctrl.enumerate() {
                Ok(devs) => {
                    for dev in devs {
                        hal::log!(
                            "usb: device {:04x}:{:04x} config {} interfaces {:?}",
                            dev.vendor,
                            dev.product,
                            dev.configuration,
                            dev.interfaces.iter().map(|i| (i.num, i.alt, i.class, i.subclass, i.protocol)).collect::<Vec<_>>()
                        );
                        found.push((ci, dev));
                    }
                }
                Err(e) => hal::log!("usb: enumerating controller {ci} failed: {e:?}"),
            }
        }
        Scan { ctrls, found }
    }

    /// The (device, configuration) entries not yet taken.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.found.iter().map(|(_, d)| d)
    }

    /// Takes a device for which `target` returns the interface to claim (its bulk endpoints
    /// are opened); among matches `prefer` wins, otherwise the first. Selects the
    /// device's configuration. `Err(Unsupported)` when nothing matches.
    pub fn take(
        &mut self,
        target: impl Fn(&Device) -> Option<desc::InterfaceDesc>,
        prefer: impl Fn(&Device) -> bool,
    ) -> Result<(Controller, Device)> {
        let usable = |ci: usize| self.ctrls[ci].is_some();
        let matches: Vec<usize> = self
            .found
            .iter()
            .enumerate()
            .filter(|(_, (ci, d))| usable(*ci) && target(d).is_some())
            .map(|(i, _)| i)
            .collect();
        let Some(&pick) = matches.iter().find(|&&i| prefer(&self.found[i].1)).or(matches.first()) else {
            return Err(Error::Unsupported);
        };
        let (ci, mut dev) = self.found.swap_remove(pick);
        let iface = target(&dev).ok_or(Error::Unsupported)?;
        let mut ctrl = self.ctrls[ci].take().ok_or(Error::Unsupported)?;
        let configuration = dev.configuration;
        hal::log!("usb: using configuration {configuration} of {:04x}:{:04x}", dev.vendor, dev.product);
        ctrl.set_configuration(&mut dev, configuration, &iface)?;
        Ok((ctrl, dev))
    }
}

impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}
