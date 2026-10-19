# Planned Feature: Graphical QEMU E2E Test Launcher

## Overview
Interactive visual testing mode directly inside the GTK desktop application that spins up QEMU with a display window (`-display gtk` or SDL) rather than headless serial redirection.

## Key Capabilities
- **One-Click Test Launch**: Launch a live visual virtual machine from the HeaderBar or Flash dialog running the generated ISO against a transient disk image.
- **Visual Framebuffer Observation**: Watch the kernel's bootsplash banner, driver initialization, partition formatting, package downloads, and first-boot display manager live.
- **Configurable Hardware Emulation**: Switch between BIOS/UEFI, virtio/NVMe/AHCI/IDE disks, and various NIC models (e1000, virtio-net, pcnet32, e100) directly from the GUI test settings.
