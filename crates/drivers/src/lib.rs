//! Polling drivers: PCI enumeration, virtio, AHCI, legacy ATA (IDE mode), NVMe and NICs.
#![no_std]

extern crate alloc;

#[cfg(feature = "ahci")]
pub mod ahci;
#[cfg(feature = "alx")]
pub mod alx;
#[cfg(feature = "ata")]
pub mod ata;
pub mod dma;
#[cfg(feature = "e1000")]
pub mod e1000;
#[cfg(feature = "igb")]
pub mod igb;
pub mod mmio;
#[cfg(feature = "nvme")]
pub mod nvme;
pub mod pci;
pub mod platform;
pub mod port;
#[cfg(feature = "r8169")]
pub mod r8169;
#[cfg(feature = "vmxnet3")]
pub mod vmxnet3;
#[cfg(feature = "r8169")]
mod r8169_chip;
#[cfg(feature = "rtl8139")]
pub mod rtl8139;
#[cfg(any(feature = "virtio-blk", feature = "virtio-net"))]
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
        #[cfg(any(feature = "virtio-blk", feature = "virtio-net"))]
        virtio::probe(&dev, &mut devs);
        #[cfg(feature = "ahci")]
        ahci::probe(&dev, &mut devs);
        #[cfg(feature = "alx")]
        alx::probe(&dev, &mut devs);
        #[cfg(feature = "ata")]
        ata::probe(&dev, &mut devs);
        #[cfg(feature = "nvme")]
        nvme::probe(&dev, &mut devs);
        #[cfg(feature = "e1000")]
        e1000::probe(&dev, &mut devs);
        #[cfg(feature = "igb")]
        igb::probe(&dev, &mut devs);
        #[cfg(feature = "r8169")]
        r8169::probe(&dev, &mut devs);
        #[cfg(feature = "vmxnet3")]
        vmxnet3::probe(&dev, &mut devs);
        #[cfg(feature = "rtl8139")]
        rtl8139::probe(&dev, &mut devs);
    }
    devs
}
