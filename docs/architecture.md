# Architecture

An Arch Linux installer written in Rust that does not run on top of Linux.

## Overview

- The ISO boots through a small boot loader of its own (`boot/`, BIOS and UEFI, a few tens of KiB) into a small `no_std` kernel of its own.
- It runs on a single CPU core, uses polling drivers, and has no device interrupts. The only interrupt is a 1 kHz timer tick that lets idle loops halt the CPU instead of spinning.
- That kernel reads a configuration baked into the ISO, downloads packages from an Arch mirror over HTTPS, verifies them, lays down GPT + FAT32 + ext4, and writes the system to disk.
- Anything that needs a running Linux, such as `pacman` scriptlets, ALPM hooks, `mkinitcpio`, and user creation, is deferred to the first boot of the installed system.

## Design goals

- The installer is unattended by design.
- There is no interactive UI: the config decides everything.
- The disk to erase is chosen either by serial number or, if you opt in, simply as the largest one.

> **Warning.** With `disk.auto_largest = true` (the setting in the presets and in `configs/config.lua`),
> booting the ISO **erases the largest disk in the machine without asking.** Only boot these ISOs on
> hardware or VMs where that is what you want.

This is a proof of concept for people who reinstall Arch often and want it automated. It has no prompts,
countdowns, or abort keys on purpose.

## Install steps

1. **Boot.** The boot loader unpacks the payload (the kernel plus modules: `config.bin`, the signing keyring,
   `tiny-init`, the loader's own files for the target) and enters the kernel.
2. **Disk.** The target disk is selected. Nothing is written before this succeeds.
3. **Network.** DHCP, then `core.db` and `extra.db` over HTTPS (TLS 1.2/1.3 via `rustls` with a pure-Rust
   crypto provider and the bundled Mozilla roots).
4. **Resolve.** A pacman-compatible resolver (libalpm `vercmp`, versioned deps, soname provides, groups,
   repo priority, conflicts). Where the config leaves a provider choice open the first candidate by repo
   priority and name is used, like pacman's default answer, and the choice is logged.
5. **Partition and format.** GPT with a protective MBR: 1 MiB BIOS boot partition, ESP, root. The root is a
   write-once ext4 image (extents, xattrs, `metadata_csum`, an internal journal) built by `ext4w`.
6. **Packages.** Each package is streamed into `/var/cache/pacman/pkg` on the target while its SHA-256 and
   PGP signature are checked (the signature comes from the database's `%PGPSIG%`, verified against a keyring
   blob derived from the pinned `archlinux-keyring`). Only after verification is it extracted.
7. **Boot setup.** `/etc` files, a first-boot service and a small initramfs (`tiny-init` plus the kernel
   modules it needs) are written; our boot loader (first boot only) goes onto the ESP and, for BIOS, into the MBR and BIOS boot partition.
8. **First boot.** The system boots into `archstaller-firstboot.target`: it initializes the pacman keyring,
   runs `pacman -U` on the cached packages (which runs every scriptlet and hook), re-applies the
   configuration, creates users, enables services, builds the real initramfs, writes
   GRUB (`grub-install`, from the `grub` package, replacing our loader) and reboots into the installed system.

## Trust model and limits

- The trust anchor is the build host: the keyring blob is derived from a pinned `archlinux-keyring` package
  (sha256-checked) when the ISO is built, using GnuPG's web of trust (a packager key needs certifications
  from at least 3 of the main keys).
- A package signed by a packager who joined after the ISO was built fails with an unknown-key error. The
  fix is to rebuild the ISO; in practice, rebuild every month or two (Arch publishes a new keyring roughly
  that often). A key revoked after the build is still trusted by that ISO until it is rebuilt.
- The keyring is pinned in `xtask/keyring.pin` (`<version> <sha256>`). `cargo xtask update-keyring` moves the
  pin to the newest `archlinux-keyring` in the mirror's `core.db`: it downloads the package, checks its
  SHA-256 against the database, tries to verify its signature with the previously pinned keyring (and says
  so if it cannot), rewrites the pin and rebuilds the keyring blob. Then `cargo xtask presets` rebuilds
  the ISOs. Nothing else in the ISO goes stale: package databases and packages are fetched at install
  time, so each install gets whatever the mirror has that day.
- Arch's sync databases are not signed; their integrity relies on HTTPS.
- Out of scope: Wi-Fi, USB devices other than tethering phones and Ethernet dongles, SMP, Secure Boot, an interactive UI, filesystems other than ext4, architectures
  other than x86_64.

## Repository layout

| Path | Contents |
|---|---|
| `kernel/` | the `no_std` installer kernel (`x86_64-unknown-none`): console, heap, exceptions, idle timer tick, random numbers, TSC clock, paging for MMIO, the install flow, the hardware test |
| `xtask/` | host tooling: Lua evaluation, keyring blob and its pin (`keyring.pin`), payload and ISO assembly, BIOS stage patching, QEMU runs, tests |
| `config/` | config types shared by `xtask` and the kernel, plus validation |
| `crates/hal` | `BlockDevice`, `NetDevice`, `Clock`, `Rng` traits |
| `crates/drivers` | PCI, virtio-blk/net, AHCI, legacy ATA (IDE mode), NVMe, Intel e1000/e1000e/igb/igc, Realtek r8101/r8168/r8169/r8125/rtl8139, Atheros alx/atl1c, VMware vmxnet3 (all polled) |
| `crates/usb` | xHCI driver with control and bulk transfers (used only for tethering phones and USB dongles) |
| `crates/imobiledevice` | iPhone USB tethering: plist, usbmuxd, pairing, lockdownd, `NetDevice` adapter |
| `crates/usbnet` | Android tethering and USB Ethernet dongles: RNDIS, CDC-ECM, CDC-NCM and ASIX AX88179 `NetDevice` |
| `crates/net` | smoltcp stack (DHCP with address probe, DNS fallbacks), HTTP/1.1 client, TLS |
| `crates/pgp-lite` | OpenPGP v4 signature verification (RSA, EdDSA) |
| `crates/pkg` | sync database parser, `vercmp`, resolver, tar/zstd/gzip readers |
| `crates/ext4w` | write-once ext4 writer |
| `crates/disk` | GPT, FAT32 writer |
| `crates/initrd` | cpio writer, kernel module dependency resolution |
| `boot/` | the boot loader: `uefi/` (`BOOTX64.EFI`), `bios-s1/` (512-byte MBR/El Torito sector), `bios/` (stage 2) |
| `crates/bootinfo`, `crates/loadcore` | loader/kernel contract and the firmware-independent loader core |
| `tiny-init/` | the initramfs `init` (raw syscalls, no libc) |
| `firstboot/` | systemd units and script for the first boot |
| `crates/aurbuild` | host-only AUR support: search, review, pin and plan (the packages are built on the installed system, see `plans-implement/aur.md`) |
| `crates/hostcfg` | host-only config model shared by `xtask` and the GUI: Lua loading, build profiles, driver and script catalogues, Lua writer, package resolution preview, build progress events |
| `gui/` | `archstaller-gui`, the GTK4 desktop app (Linux): config editor, Lua source editor, ISO build, Ventoy copy, raw USB flash |
| `configs/` | Lua configs: example, e2e, presets, shared modules, generated LuaLS types |
