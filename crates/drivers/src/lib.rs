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

/// The driver that claims a PCI network controller id, without touching hardware. This is what
/// `cargo xtask coverage` counts, so it must list exactly the ids the `probe` functions accept.
pub fn net_driver_for(vendor: u16, device: u16) -> Option<&'static str> {
    #[cfg(feature = "virtio-net")]
    if vendor == 0x1af4 && matches!(device, 0x1000 | 0x1041) {
        return Some("virtio-net");
    }
    #[cfg(feature = "e1000")]
    if e1000::recognizes(vendor, device) {
        return Some("e1000");
    }
    #[cfg(feature = "igb")]
    if igb::recognizes(vendor, device) {
        return Some("igb");
    }
    #[cfg(feature = "r8169")]
    if r8169::recognizes(vendor, device) {
        return Some("r8169");
    }
    #[cfg(feature = "rtl8139")]
    if rtl8139::recognizes(vendor, device) {
        return Some("rtl8139");
    }
    #[cfg(feature = "alx")]
    if alx::recognizes(vendor, device) {
        return Some("alx");
    }
    #[cfg(feature = "vmxnet3")]
    if vmxnet3::recognizes(vendor, device) {
        return Some("vmxnet3");
    }
    let _ = (vendor, device);
    None
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
