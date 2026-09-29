//! Polling drivers: PCI enumeration, virtio, AHCI, NVMe and NICs.
#![no_std]

extern crate alloc;

pub mod ahci;
pub mod dma;
pub mod e1000;
pub mod mmio;
pub mod nvme;
pub mod pci;
pub mod platform;
pub mod port;
pub mod r8169;
pub mod virtio;

use alloc::{boxed::Box, vec::Vec};
use hal::{BlockDevice, NetDevice};

#[derive(Default)]
pub struct Devices {
    pub block: Vec<Box<dyn BlockDevice>>,
    pub net: Vec<Box<dyn NetDevice>>,
}

/// Enumerates PCI and initializes every supported device.
pub fn probe_all() -> Devices {
    let mut devs = Devices::default();
    for dev in pci::enumerate() {
        virtio::probe(&dev, &mut devs);
        ahci::probe(&dev, &mut devs);
        nvme::probe(&dev, &mut devs);
        e1000::probe(&dev, &mut devs);
        r8169::probe(&dev, &mut devs);
    }
    devs
}
