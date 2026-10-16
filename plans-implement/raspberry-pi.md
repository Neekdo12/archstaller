# Implementation Spec: Raspberry Pi Support (Pi 3B+ & Pi 5)

**Status:** Milestone 1 implemented (AArch64 target in toolchain, bare-metal `kernel-rpi`, AArch64 HAL with PL011 UART and generic timer, `xtask rpi-build` & `xtask rpi-run`).

**Goal:** Enable bare-metal Arch Linux ARM installation on Raspberry Pi 3 Model B+ (BCM2837B0) and Raspberry Pi 5 (BCM2712 + RP1) without relying on an existing Linux OS or external installation environment.

---

## 1. Scope & Constraints

As established in [`AGENTS.md`](../AGENTS.md), Archstaller runs a custom `no_std` bare-metal installer kernel without a Linux kernel in the installer path.

Supporting Raspberry Pi introduces the first non-`x86_64` architecture (`aarch64-unknown-none`). All existing design rules remain strictly in force:
- **Polling only:** No device interrupts. Polling loops use ARM's `wfi` (Wait For Interrupt) / `hal::idle()` driven by the ARM Generic Timer (`CNTPCT_EL0`).
- **No Linux kernel in installer:** The installer kernel is a bare-metal `no_std` AArch64 binary.
- **Shared contracts:** Config schema (`config/src/lib.rs`), disk formatting (`crates/disk`), package database parsing (`crates/pkg`), and networking (`crates/net`) are shared across architectures.
- **Target OS:** Installs official **Arch Linux ARM** (`aarch64`), pulling packages from ALARM mirrors and deploying the appropriate kernel package (`linux-rpi` for Pi 3, `linux-rpi-16k` for Pi 5).

---

## 2. Hardware Comparison: Pi 3B+ vs. Pi 5

| Feature | Raspberry Pi 3 Model B+ | Raspberry Pi 5 |
| :--- | :--- | :--- |
| **SoC** | Broadcom BCM2837B0 | Broadcom BCM2712 |
| **CPU** | 4x Cortex-A53 @ 1.4 GHz (ARMv8.0-A) | 4x Cortex-A76 @ 2.4 GHz (ARMv8.2-A) |
| **Page Size** | 4 KiB | 4 KiB or 16 KiB (kernel default is 16k) |
| **I/O Subsystem** | Direct SoC MMIO (`0x3F000000`) | Raspberry Pi RP1 southbridge via PCIe 2.0 x4 (`0x1000000000`) |
| **Console UART** | PL011 UART0 (`0x3F201000`) / Mini UART | RP1 UART0 / BCM2712 Debug UART |
| **Storage** | Arasan SDHCI / Broadcom eMMC (`0x3F300000`) | BCM2712 SD/eMMC controller + PCIe M.2 NVMe |
| **Ethernet** | Microchip LAN7515 (USB 2.0 via DWC2 OTG) | BCM54213PE Gigabit PHY via MACB/GEM |
| **Firmware Boot** | VideoCore IV (`bootcode.bin`, `start.elf`) | On-board SPI EEPROM + VideoCore VII (`bootloader`) |
| **Interrupt Controller**| BCM2836 ARM Control Interrupts | ARM GIC-400 (GICv2) |

---

## 3. Boot Architecture & Firmware Contract

### 3.1 Firmware Boot Flow
Unlike PC x86 systems (BIOS/UEFI), the Raspberry Pi boots via its VideoCore GPU firmware:
1. **Pi 3B+:** Reads `bootcode.bin` and `start.elf` from the first FAT32 partition of the SD card.
2. **Pi 5:** Executes bootloader code from onboard SPI flash, reads `config.txt` and hardware overlays from the FAT32 boot partition.
3. Firmware loads `config.txt`, parses overlays, loads the Device Tree (`bcm2710-rpi-3-b-plus.dtb` or `bcm2712-rpi-5-b.dtb`), and loads the 64-bit kernel binary into physical RAM (standard address `0x00080000`).
4. Firmware places the pointer to the prepared Device Tree Blob (DTB) into CPU register `x0` and jumps to the kernel entry point at Exception Level 2 (EL2) or Exception Level 1 (EL1).

### 3.2 Binary Delivery: `kernel8.img`
Rather than an ISO9660 image (which Pis cannot boot directly), `cargo xtask` will generate an SD card image (`.img`) structured as:
- **Partition 1 (FAT32, ~64 MiB):** Contains firmware blobs (`bootcode.bin`, `start.elf` for Pi 3; `config.txt` for both), device trees, and Archstaller's `kernel8.img` (containing the bundled payload: configuration + installer kernel).
- **Partition 2 (Optional scratch partition):** For logs and local mirrors if running offline.

### 3.3 AArch64 Boot Contract (`crates/bootinfo`)
Extend `crates/bootinfo` with conditional fields for AArch64:
```rust
#[repr(C)]
pub struct Aarch64BootInfo {
    pub dtb_ptr: u64,
    pub dtb_size: u64,
    pub ram_base: u64,
    pub ram_size: u64,
    pub framebuffer: FramebufferInfo,
    pub uart_mmio_base: u64,
}
```

---

## 4. Architectural Refactoring: Multi-Arch `crates/hal`

Currently, `crates/hal` assumes x86_64 (e.g. port I/O, `hlt`, CR3). We separate architectural primitives:

```
crates/hal/
├── Cargo.toml
└── src/
    ├── lib.rs          # Unified traits: Console, BlockDevice, NetDevice, Timer, MemoryMap
    ├── x86_64/         # Existing x86 primitives (port I/O, GDT, IDT, CR3, hlt)
    └── aarch64/        # New ARM primitives:
        ├── mmu.rs      # TCR_EL1, MAIR_EL1, TTBR0_EL1, 4-level page tables
        ├── exceptions.rs # VBAR_EL1 vector table, ESR_EL1 / FAR_EL1 decoding
        ├── timer.rs    # CNTPCT_EL0, CNTFRQ_EL0 (1 kHz tick emulation)
        └── barrier.rs  # isb(), dsb(), dmb()
```

### 4.1 Memory Management (MMU)
- **Identity Map Early:** Map physical RAM, MMIO peripherals (device memory: `nGnRnE`), and kernel text/data.
- **Page Granule:** Support 4 KiB pages for Pi 3 and Pi 5; ensure memory attributes are configured for non-cacheable DMA descriptors used in networking and storage.

---

## 5. Storage Drivers

### 5.1 Pi 3B+: Arasan SDHCI / Broadcom EMMC
- Location: MMIO `0x3F300000`.
- Polling implementation implementing `hal::BlockDevice`:
  - Standard SD command/response loop (CMD0, CMD8, ACMD41, CMD2, CMD3, CMD9, CMD7).
  - High-speed block transfers using polling data register `SDHCI_BUFFER`.
  - Block size: 512 bytes.

### 5.2 Pi 5: BCM2712 SD & PCIe NVMe
- **SD Controller:** Standard SDHCI controller in BCM2712 with UHS-I support.
- **NVMe on Pi 5:** The existing `crates/drivers/src/nvme.rs` driver can be reused directly over PCIe on Pi 5 when an M.2 HAT is attached.

---

## 6. Network Drivers

### 6.1 Pi 3B+: SMSC LAN7515 (USB)
- The Ethernet port on Pi 3B+ is connected internally through the Synopsys DesignWare Hi-Speed USB 2.0 OTG Controller (DWC2) at MMIO `0x3F980000`.
- Requires a polling DWC2 host-mode driver (`crates/drivers/src/dwc2.rs`) and a driver for the LAN7515 CDC-Ethernet/vendor protocol.

### 6.2 Pi 5: BCM54213PE via MACB / Genet
- Pi 5 routes Gigabit Ethernet directly to an internal MAC controller.
- Driver: A polling MACB/GEM driver with DMA descriptor ring polling, matching the polling style of `crates/drivers/src/e1000.rs`.

---

## 7. Arch Linux ARM Target Packages & Partitioning

1. **Mirror Architecture:** Use Arch Linux ARM mirrors (e.g. `http://mirror.archlinuxarm.org/aarch64/$repo`).
2. **Keyring:** `archlinuxarm-keyring` (different master keys from Arch x86_64).
3. **Installed Kernel:**
   - Pi 3B+: `linux-rpi`
   - Pi 5: `linux-rpi-16k` (16 KiB page size kernel recommended for Cortex-A76 performance)
4. **Boot Partition Contents:**
   - Write Raspberry Pi firmware files (`overlays/`, `bcm2710-*.dtb`, `bcm2712-*.dtb`, `config.txt`, `cmdline.txt`) to `/boot` FAT32 partition.
   - `cmdline.txt`: `root=PARTUUID=<uuid> rw rootwait console=serial0,115200 console=tty1 fsck.repair=yes`.

---

## 8. Development & Verification Plan

1. **Milestone 1 — Toolchain & QEMU AArch64 Skeleton:**
   - Add `aarch64-unknown-none` target to `rust-toolchain.toml`.
   - Implement `crates/hal/src/aarch64/` with PL011 UART output and system timer.
   - Boot and verify in `qemu-system-aarch64 -M raspi3b` and `-M virt`.
   - Automated end-to-end check in `cargo xtask rpi-test` (verifying EL3->EL1, MIDR_EL1 CPU detection, timer, UART console output).
2. **Milestone 2 — Polling SD Card Driver:**
   - Implement Arasan SDHCI driver. Verify block read/write and MBR/GPT partitioning on emulated SD card.
3. **Milestone 3 — Networking:**
   - Hook up `smoltcp` via polling Ethernet driver.
   - Verify DHCP and ALARM repository HTTP package downloads.
4. **Milestone 4 — Real Hardware Test (Pi 3B+):**
   - Flash `.img` to SD card, boot Pi 3B+, verify end-to-end Arch Linux ARM install.
5. **Milestone 5 — Pi 5 Support:**
   - Add BCM2712 MMIO mappings and GEM Ethernet driver.
   - Verify installation on Pi 5 with SD card and NVMe SSD.

---

## 9. GUI Architecture & Integration (`archstaller-gui`)

In the desktop GUI, Raspberry Pi support connects through three primary modules:

### 9.1 Hardware Target & Board Selector (`gui/src/pages/build_page.rs`)
- In addition to the build profile (`super-small` vs `large`) and driver selections, a **Target Board / Architecture** option group allows choosing:
  - `x86_64 PC (BIOS / UEFI)` — produces `archstaller.iso`.
  - `Raspberry Pi 3B+ (AArch64 / BCM2837)` — produces `archstaller-rpi3.img`.
  - `Raspberry Pi 5 (AArch64 / BCM2712)` — produces `archstaller-rpi5.img`.
- Model (`gui/src/model.rs` & `hostcfg`): Stored under `host.target_arch` / `host.board` in the host settings.

### 9.2 Build Pipeline & Artifact Output (`gui/src/build.rs`)
- Raspberry Pi hardware **cannot boot ISO9660**. It requires an SD card disk image (`.img`) containing an MBR partition table with a FAT32 boot partition (`bootcode.bin`, `start.elf`, `config.txt`, device trees, `kernel8.img`).
- For x86_64: `gui/src/build.rs` invokes `cargo xtask build --config ... --out .../archstaller.iso --progress json`.
- For Raspberry Pi: `gui/src/build.rs` invokes `cargo xtask rpi-image --board <rpi3b|rpi5> --config ... --out .../archstaller-rpi.img --progress json`.
- Structured progress events (`@archstaller-event`) continue streaming to the GUI progress bar.

### 9.3 Flashing to Removable Storage (`gui/src/pages/iso.rs` & `gui/src/flash.rs`)
- **Ventoy vs Direct Flash:** Ventoy cannot boot bare-metal VideoCore ARM firmware payloads. When Raspberry Pi target is active, the Ventoy file copy option is hidden or replaced by **Direct SD Card / USB Flashing**.
- **Device Recognition (`gui/src/flash.rs`):**
  - Detects SD cards (`/dev/mmcblk0`) and USB card readers (`/dev/sdX`) via `lsblk` and sysfs.
  - Eligibility checks require removable/hotplug media, sufficient size, unmounted file systems, and polkit authorization.
- **Source Inspection:**
  - `Source::of` is generalized: instead of requiring only ISO9660 (`CD001` magic at offset `0x8000`), it accepts raw partitioned disk images (`.img` / MBR partition table with FAT32 boot partition).
- Writing and verification:
  - Unmounts SD card partitions via UDisks2.
  - Writes the raw `.img` stream from sector 0.
  - `fsync` and SHA-256 read-back verification.
