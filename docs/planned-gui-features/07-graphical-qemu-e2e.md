# Planned Feature: Graphical QEMU E2E Test Launcher

## Overview
This specification details a graphical QEMU End-to-End (E2E) launcher accessible directly from the Archstaller GUI (Build & Flash ISO / Output views). It provides an interactive testing environment where developers and users can visually watch the installer boot, partition, download packages, and boot the newly installed system inside a graphical QEMU window.

## Context & Motivation
Archstaller includes comprehensive automated E2E test scripts (`cargo xtask e2e`, `cargo xtask run`), which typically run in headless or serial-only mode. When designing UI branding, verifying console fonts, or troubleshooting display manager behavior (Ly, SDDM, GDM, LightDM), visual inspection in a graphical window (`-display gtk` or `-display sdl`) is essential. Integrating this into the GUI allows one-click test runs immediately after configuring an ISO.

## Specification

### 1. GUI Interface (Build ISO / Flash Dialog)
- **"Run in QEMU" Launcher Button**:
  - Located beside the "Build ISO" and "Flash to Drive" action buttons.
  - Opens a test configuration popover or dialog:
    - *Firmware*: UEFI (OVMF) vs. Legacy BIOS (SeaBIOS).
    - *Disk Controller*: VirtIO-Block, NVMe, AHCI (SATA), or IDE (PIIX3).
    - *Disk Size*: Spinbutton (default: 20 GiB sparse qcow2 image).
    - *Network Interface*: VirtIO-Net, Intel e1000, Realtek rtl8139, Intel e100, AMD PCnet32, or None.
    - *RAM & CPU*: Configurable RAM (default: 4096 MiB) and CPU cores (default: 2).
    - *Display Backend*: GTK window (`-display gtk,gl=on`), SDL (`-display sdl`), or VNC viewer.
    - *Keep Disk Image*: Checkbox to preserve the installed virtual disk after test termination for subsequent boots.
- **Interactive Control**:
  - Live QEMU process status (PID, runtime, CPU load).
  - Stop / Terminate / Reset buttons.
  - Live serial console output viewer in an expandable drawer beneath the QEMU status.

### 2. Backend Orchestration
- Spawns `cargo xtask e2e --gui` or directly invokes `qemu-system-x86_64` using the compiled ISO image (`target/archstaller.iso`) and a temporary sparse disk in `/tmp` or the checkout target directory.
- Captures serial output from stdio/socket for diagnostic analysis.
