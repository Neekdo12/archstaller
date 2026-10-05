# OVERVIEW.md

Current architecture of archstaler. Kept in sync with the repo by rule (see `AGENTS.md`).

## What it is

An Arch Linux installer that boots without any Linux kernel. Our own single-purpose boot loader (`boot/`, BIOS + UEFI) loads a custom
`no_std` Rust mini-kernel that installs Arch onto disk: partitions, downloads and verifies packages over
HTTPS, writes the filesystem, and hands off to a first-boot service that runs real pacman/systemd steps
that need a running Linux.

## Boot/install flow

1. **Boot loader** (`boot/uefi` for UEFI, `boot/bios-s1` + `boot/bios` for BIOS, both finishing in `crates/loadcore`)
   finds the payload bundle (`payload.bin`: the kernel image plus modules `config.bin`, `keyring.bin`,
   `tiny-init`, `bootx64.efi`, `bios-boot`, each deflated; layout in `crates/bootinfo`), unpacks it, builds
   page tables (direct map at `0xffff800000000000`, kernel at `0xffffffff80000000`), and enters the kernel with
   a `bootinfo::BootInfo` (memory map, framebuffer, modules) in `rdi`. No config, no menu, no CLI.
2. **kernel** (`kernel/src/main.rs`, `install.rs`) runs, single core, polling only:
   - enumerates disks/NICs via `crates/drivers::probe_all()` (PCI scan). With the `usb-tethering` kernel
     feature it also brings up a tethered phone (iPhone via `imobiledevice::tether()`, Android via
     `usbnet::probe()`) and adds it as one more NIC when no wired NIC has link.
   - selects the target disk per `Config.disk` (serial match, or `auto_largest`); writes nothing until
     this succeeds.
   - DHCP, then downloads `core.db`/`extra.db` over HTTPS (`crates/net`).
   - resolves packages (`crates/pkg`): pacman-compatible `vercmp`, versioned deps, soname provides,
     groups, repo priority, conflicts.
   - partitions (`crates/disk`): GPT + protective MBR, BIOS boot partition, ESP (FAT32, `/boot`), root.
   - writes root as ext4 (`crates/ext4w`), write-once, with an internal journal (inode 8, a valid jbd2 superblock, 4-64 MiB depending on size, none under 32 MiB) created together with the filesystem.
   - streams each package into `/var/cache/pacman/pkg` on the target, verifying SHA-256 and PGP signature
     (`crates/pgp-lite` + `keyring.bin`) before extracting.
   - writes `/etc` (fstab, hostname, locale, mirrorlist, ...), builds an initramfs (`crates/initrd` +
     `tiny-init`), installs the boot loader on the target (UEFI: our `BOOTX64.EFI` + `archstaler.cfg` on the ESP; BIOS: stage 1 in the MBR, stage 2 in the BIOS boot partition, told where `vmlinuz-linux` and the initramfs lie), reboots. That first-boot loader only starts the Arch kernel (UEFI: `LoadImage` of the EFI-stub `vmlinuz` with the command line and an initrd `LoadFile2`; BIOS: Linux 64-bit boot protocol).
3. **First boot** (`firstboot/`): `archstaler-firstboot.target`/`.service` runs `pacman-key --init/
   --populate`, `pacman -U --overwrite '*'` on the cached packages (runs real scriptlets/hooks, writes
   the local pacman DB), creates users, enables services, builds the real
   initramfs (without the `kms` hook, so a failing GPU driver cannot block the boot), replaces our loader with GRUB (`grub-install` for UEFI removable path + BIOS, hand-written `grub.cfg`; `grub` must be in `packages`), reboots into the installed system.

## Repository layout

| Path | Contents |
|---|---|
| `kernel/` | the installer kernel: console (`fb.rs`, `serial.rs`, `console.rs`), heap (`heap.rs`: the largest usable region below 4 GiB, capped at 512 MiB, because the bootloader's direct map is only guaranteed there; it prints the region before using it), exceptions (`idt.rs`: all 256 vectors, spurious PIC IRQ7/15 handled), randomness (`rng.rs`: `RDRAND`, or on older CPUs a weaker timing-jitter generator hashed with SHA-256, which the hardware test reports as a warning), hardware test (`hwtest.rs`), driver log digest (`digest.rs`: keeps `hal::log!` lines so the hardware test reprints them at the end), idle tick (`idle.rs`: 1 kHz PIT via the legacy PIC, LAPIC LINT0 unmasked as ExtINT, so polling loops can `hlt` through `hal::idle()`; falls back to spinning if no tick arrives), paging (`paging.rs`), TSC clock (`time.rs`), install flow (`install.rs`), block-letter banners (`banner.rs`: "ARCHSTALLER" at start, "INSTALLED" plus an 8 s pause before the reboot when the install succeeds; the same art is written into the installed system's `/etc/issue` and `/etc/motd`, and, when `ly` is installed, as a gzip DurDraw file `/etc/ly/archstaler.dur` that `firstboot.sh` sets as ly's `animation = dur_file` background above the login box), entry point and `reboot()` (`main.rs`: 8042 pulse, then port 0xCF9, then triple fault) |
| `xtask/` | host tooling: `iso.rs` (build kernel + assemble ISO), `lua.rs` (re-export of `hostcfg::lua`; config.lua -> config.bin, including shared preset modules on Windows), `payload` bundle building and ISO assembly in `iso.rs`, `bios.rs` (build the BIOS stages, patch them into the ISO), `keyring.rs` (keyring blob + pin), `qemu.rs` (run in QEMU), `e2e.rs` (install + boot test), `presets.rs`, `linux_test.rs`, `main.rs` (CLI: `--profile`, `--progress json`, `--workdir`) |
| `config/` | `Config`/`Disk`/`User`/`UserFile`/`UserArchive`/`Script` types shared by `xtask` and `kernel`, plus validation |
| `crates/hostcfg` | host-only (std) library used by `xtask` and `gui/`: `lua.rs` (evaluates a config into `config::Config` plus the host-only `HostConfig`), `host.rs` (`build.profile` super-small/large with precedence CLI `--profile` > deprecated `--small` > config > default super-small; `extra-large` reserved; the installer driver catalogue and its validation; `aur_packages` reserved), `scripts.rs` (built-in first-boot scripts from `firstboot/scripts/`, local-file embedding), `writer.rs` (deterministic Lua generator, header `-- generated by archstaler-gui`), `resolve.rs` (the installer's resolver on host sync DBs or DBs fetched from a mirror), `password.rs` (`$6$` hashes), `progress.rs` (JSON-lines build events printed by `xtask --progress json`) |
| `gui/` | `archstaler-gui` (egui/eframe, cross-platform): `model.rs` (document, validation through hostcfg, area mapping), `build.rs` (spawns `cargo xtask build ... --progress json --workdir target/gui-builds/<id>`, cancel kills it and removes only that workspace), `media.rs` (`MediaTarget` trait, `FolderCopy` with free-space check, temp name + rename, fsync and SHA-256 read-back; Ventoy detection by its own files), `app.rs` ("From preset" menu: the repository's `presets/*.lua`, evaluated like the CLI and opened as an unsaved form; tabs: System, Disk, Mirrors & packages with a resolve preview, Users with password hashing, Services & kernel, Build & drivers, Scripts, Lua preview, Build ISO + copy). Raw flash and AUR are not implemented |
| `crates/hal` | traits: `BlockDevice`, `NetDevice`, `Clock`, `Rng` |
| `crates/drivers` | each driver is a cargo feature (`virtio-blk virtio-net ahci nvme e1000 igb r8169 rtl8139`, all default; `xtask` builds the kernel with only `installer_drivers`), PCI enumeration (`pci.rs`), virtio blk/net, AHCI, NVMe, Intel e1000/e1000e/igb(+igc), Realtek r8169/r8125/rtl8139 (r8169 has the RTL8168evl/8111evl setup, 256-entry RX ring, verified on one real board) — all polled, no IRQs |
| `crates/usb` | xHCI host controller driver (polled, takes ownership from the firmware via the legacy-support capability, logs each enumeration step through `hal::log!`): command/event/transfer rings, enumeration of enabled ports and USB configurations, descriptors, control + bulk transfers. One controller, no hubs |
| `crates/imobiledevice` | iPhone USB tethering: plist codec (reads binary and XML, writes XML), the device-side usbmux protocol over the phone's mux interface (`mux.rs`: version handshake, small TCP-like connections), lockdownd QueryType/GetValue/Pair (`lockdown.rs`, `pair.rs`, `cert.rs`: generated RSA-2048 host identity and certs), and the phone's `ipheth` tethering interface as a `hal::NetDevice` (`netdev.rs`). Unit-tested on the host only; needs a real phone end-to-end |
| `crates/usbnet` | Android phones and USB Ethernet dongles as a `hal::NetDevice`: RNDIS (`rndis.rs`), CDC-ECM, CDC-NCM (`ncm.rs`) and the vendor-specific ASIX AX88179 (`ax88179.rs`) |
| `crates/net` | `smoltcp` stack glue (`stack.rs`; 256 KiB TCP receive buffer with window scaling: a 64 KiB window caps a download at 64 KiB / round-trip time, about 3 MiB/s at 20 ms, while a window much larger than the NIC rings lets a fast sender overflow them while the CPU decrypts), DNS resolvers: the DHCP ones first, then 1.1.1.1 and 8.8.8.8 as fallbacks (a campus resolver that never answered was seen), an ARP probe of the leased address before using it (RFC 5227; a host already using it gets a DHCPDECLINE and the DHCP exchange restarts, up to 4 times), a gratuitous ARP after DHCP, an ICMP ping helper and ARP/ethertype tracing for diagnostics, HTTP/1.1 client (`http.rs`, `client.rs`; logs each stage of a download), TLS via `rustls` + `rustls-rustcrypto` + `webpki-roots` (`tls.rs`). TLS reads the socket in 16 KiB chunks |
| `crates/pgp-lite` | OpenPGP v4 signature verification (RSA, EdDSA) against an embedded keyring blob |
| `crates/pkg` | sync DB parser (`desc.rs`, `db.rs`), `vercmp` port, dependency resolver (`resolve.rs`), tar/zstd/gzip readers (`tar.rs`, `compress.rs`, `io.rs`) |
| `crates/ext4w` | write-once ext4 writer (`writer.rs`): extents, xattrs, symlinks, hardlinks; an internal journal and `metadata_csum`; no `dir_index` hashing at install time |
| `crates/disk` | GPT + protective MBR (`gpt.rs`), FAT32 writer (`fat32.rs`), CRC32 (`crc32.rs`), region helpers (`region.rs`) |
| `crates/initrd` | cpio newc writer (`cpio.rs`), kernel module dependency resolution from ELF `.modinfo` (`modules.rs`) |
| `boot/uefi` | `BOOTX64.EFI` (`x86_64-unknown-uefi`, raw UEFI FFI, ~23 KiB): with `\payload.bin` on its volume it picks the GOP framebuffer, allocates the loader arena, exits boot services and calls `loadcore`; with `\archstaler.cfg` (target ESP) it starts the Linux EFI stub kernel |
| `boot/bios-s1` | BIOS stage 1, 512 bytes (MBR or El Torito boot sector): INT 13h extended reads of stage 2 to `0x8000`; sector size from function 48h (CD = 2048) |
| `boot/bios` | BIOS stage 2: `entry.s` (real mode: A20 by BIOS/KBC/fast, E820 with E801/88h fallback, VBE mode choice from EDID, reads items into 16 MiB+ through protected mode, long mode with 4 GiB identity map), `main.rs` (memory map, `loadcore` for the installer), `linux.rs` (Linux 64-bit boot protocol for the first boot) |
| `crates/loadcore` | firmware independent loader end: memory map sanitizing (`map.rs`), page tables (`paging.rs`), payload unpacking (miniz inflate), trampoline that switches CR3 and enters the kernel |
| `crates/bootinfo` | loader/kernel contract: `BootInfo`, memory kinds, payload bundle layout, BIOS stage patch format |
| `tiny-init/` | the initramfs `/init`: raw syscalls only, no libc, `no_std`; loads modules, mounts root, `switch_root` |
| `firstboot/` | systemd unit files + `firstboot.sh`, embedded into the image at install time, run on first boot |
| `presets/`, `examples/` | Lua configs; `presets/common.lua` holds shared defaults (firmware: `linux-firmware-{intel,realtek,amdgpu,radeon}`; desktop presets use full `linux-firmware`) and supports opt-in `dry_run` mode, `presets/tester.lua` is read-only hardware testing (probes, DHCP, mirror and package resolution, pings of the gateway, the DHCP DNS server and 1.1.1.1 plus a direct TCP connect to 1.1.1.1:443 (separating "no internet" from "DNS only"), and, if all of that passed, a throughput test that downloads up to 24 MiB of the largest resolved package and prints MiB/s and Mbit/s; then it reprints the driver/device log lines (NIC identification, link and receive diagnostics) and reboots after 60 s (300 s after a failed run) via `reboot()`: 8042, port 0xCF9, triple fault), `examples/config.lua` is the documented example, `examples/e2e.lua` is used by `xtask e2e` |
| `docs/` | `wifi.md` (spec for a not-implemented feature), `iphone-tethering.md` (spec the tethering crates were written from), `implement-gui.md` (host desktop GUI: phases 1-4 implemented; direct flash and AUR not) |
| `sizes.md` | measured ISO size breakdown and size-reduction options |
| `PLAN.md` | original design plan/decision log |
| `HANDOFF.md` | temporary status notes for a new session (branches, hardware reports, what is unverified); delete when no longer useful |

## Config (`config.lua` -> `config.bin`)

Evaluated on the build host by `xtask` (`mlua`), serialized with `postcard`, embedded as the `config.bin` payload module.
Schema in `config/src/lib.rs` (installer part) and `crates/hostcfg/src/host.rs` (host-only `build`, `installer_drivers`, `aur_packages`, never serialized into the ISO): `hostname`, `timezone`, `locale`, `keymap`, `disk` (selector + `esp_mib`),
`mirrors`, `packages`, `providers` (dependency -> chosen package), `root_password_hash`, `users`
(`password_hash` is SHA-512 crypt, never plaintext), `services`, `kernel_params`, `user_files`,
`user_archives`, `scripts` (built-in id, local file embedded at build time, or https URL pinned by sha256; run as root in order at the end of the first boot by `firstboot.sh`). `xtask` validates values strictly (they end up in shell-read files, unit files, boot
loader config).

## Trust model

- Keyring blob (`boot/keyring.bin`) is derived at build time from a pinned `archlinux-keyring` package
  (`xtask/keyring.pin`, sha256-checked), reduced to fingerprint + pubkey + expiry per packager key that
  has web-of-trust certification from the main Arch signing keys (`xtask/src/keyring.rs`). The host-side
  GnuPG home is mode `0700` on Unix; on Windows it uses the inherited filesystem ACL.
- Package signatures come from the sync DB's `%PGPSIG%` field, verified against that blob
  (`crates/pgp-lite`) before extraction; SHA-256 from `%SHA256SUM%` is checked too.
- Sync databases themselves are not signed; their integrity relies on HTTPS.
- `cargo xtask update-keyring` moves the pin forward and rebuilds the blob.
- First boot's `pacman -U` re-verifies everything with real GnuPG as a backstop.

## ISO layout (assembled by `xtask/src/iso.rs`)

ISO9660 (via `xorriso`, hybrid El Torito BIOS + UEFI, also bootable when written to a USB stick) with just two
files: `boot/bios.img` (stage 1 padded to 2048 bytes, then stage 2) and `boot/efi.img` (a FAT12 image with
`EFI/BOOT/BOOTX64.EFI` and `payload.bin`). The BIOS stages read the payload straight out of `efi.img` inside
the ISO: after `xorriso` finishes, `xtask` (`bios.rs`) finds both files in the ISO 9660 tree and patches stage 1
(into the boot image and the hybrid MBR) and stage 2's header with their byte offsets. The payload is always compressed. About 0.77 MiB total; see `sizes.md`.

## Testing

- Host unit tests per crate (`cargo test -p <crate> --features std`) check against real tools/data:
  `vercmp` vs `/usr/bin/vercmp`, resolution vs `pacman -Sp`, `ext4w` images vs `e2fsck`/`debugfs`, GPT vs
  `sfdisk`/`fdisk`, FAT32 vs `fsck.fat`.
- `cargo xtask linux-test`: boots the host Linux kernel with our initramfs + an ext4w-built root.
- `cargo xtask e2e [--uefi] [--disk ...] [--nic ...] [--config FILE]`: full install in QEMU onto a blank
  disk, then boots the result twice.
- `cargo xtask run --usb`: adds `qemu-xhci` + `usb-storage` and builds the `usb-selftest` kernel, which enumerates the USB device and runs a SCSI INQUIRY over bulk endpoints.
- `cargo xtask size [--profile super-small|large] [--limit BYTES]`: ISO content breakdown + size budget check.
- `cargo xtask check-presets`: resolves every preset against local pacman sync DBs.

## Debug output

Driver and network tracing (`usb:`, `net:`, `r8169:` trace lines, ARP traces, idle-tick info, per-frame logs) is behind the
kernel feature `debug` (`hal::DEBUG`, `hal::log!`), set by `cargo xtask build --debug` (also `run`, `presets`, `e2e`) and
always on for a dry-run config such as `presets/tester.lua`. In a normal build `hal::log!` compiles to nothing. Always on:
`hal::info!` messages (failures such as a failed DNS lookup or USB step, an address conflict, the DHCP lease) and the
install progress the user sees (memory and heap size, disks, resolved package count and size, `[i/n] name version (KiB) P%`
per package, a download summary).

## USB tethering (optional)

Off in a plain build; build with `--features usb-tethering` on the kernel (`cargo xtask build --tethering`). `cargo xtask presets` turns it on for every preset unless given `--no-tethering`.
`kernel/src/main.rs` (`usb_tether`) runs after `probe_all()`, but only when no wired NIC has link. It scans
all xHCI controllers once (`usb::Scan`: waits up to 2.5 s for ports to report a connection, enumerates every
configuration of every device), then:
- If an Apple device with the mux interface is present, it runs the iPhone flow below and stops.
- Otherwise `usbnet::probe()` looks for an Android tethering function or a USB Ethernet dongle.
- If a phone-like device is attached but none tethers (an Android in file-transfer mode; devices that only
  expose storage, HID, hub, audio, video or Bluetooth interfaces do not count), it prints a hint and re-scans
  every 3 s, six times, because the phone re-enumerates once the user switches tethering on.
`install::run` uses the first NIC with link, so a wired NIC wins if present. Every step logs a `usb:` line.

### iPhone (verified on real hardware)

1. `Muxer::find` takes the phone's USB configuration that has both the mux interface (ff/fe/02) and the
   tethering interface (ff/fd/01).
2. `mux.rs` does the usbmux version handshake (v1 then v2 header) and opens a TCP-like connection to
   lockdownd (port 62078).
3. `lockdown.rs`: QueryType, GetValue `DevicePublicKey`, then `Pair` with a freshly generated pair record. The
   user taps "Trust" (and enters the passcode); the request is repeated while the phone answers
   "dialog pending"/"password protected", for up to 120 s. A successful reply may carry only an `EscrowBag`
   and no `Result` key. The record is not persisted.
4. `netdev.rs`: `SET_INTERFACE` to the tethering interface's data alternate setting, vendor request 0x00 for the
   MAC, vendor request 0x45 until the carrier is up (hotspot on), then bulk IN/OUT carry plain Ethernet
   frames (received frames have 2 padding bytes). `crates/net` is untouched.

### Android phones and USB Ethernet dongles (`crates/usbnet`)

`usbnet::probe` takes the first non-Apple device with one of four network functions and brings it up as a NIC
(a known vendor chip is preferred over class functions the same device also offers):
- **RNDIS** (control interface class e0/01/03, 02/02/ff or ef/04/01 plus a CDC data interface; what nearly all
  Android phones use): INITIALIZE, QUERY the permanent MAC (with a 48-byte information buffer, like Linux's
  `rndis_host`; devices reject an empty one), SET the packet filter, then bulk transfers wrap each Ethernet frame
  in a 44-byte RNDIS packet header (several may share one transfer).
- **CDC-ECM** (02/06/00): MAC from the Ethernet functional descriptor's string, packet filter request (a stall is
  tolerated), plain frames.
- **CDC-NCM** (02/0d/00, `ncm.rs`): GET_NTB_PARAMETERS, MAC as for ECM, frames in 16-bit NCM Transfer Blocks
  (one datagram per block on transmit, any number on receive). NTB32 and NCM1 (datagram CRC) are not handled.
- **ASIX AX88179** (`ax88179.rs`, vendor-specific interface ff/ff/00; the Axagon ADE-SG and other
  AX88179-compatible gigabit dongles, matched by USB id; verified on a real ADE-SG): PHY power-cycle and clock select, MAC read, RX/TX
  control, auto-negotiation restart, wait for link (up to 15 s, so a cable must be plugged in), then the RX queue
  and medium mode follow the negotiated speed. Transmit frames carry an 8-byte header; received transfers hold
  several packets plus a trailing packet table. Other vendor-specific chips (ASIX AX88772, Realtek RTL8152/8153,
  ...) are not supported unless they also offer a CDC configuration, which the scan finds because it looks at
  every configuration of every device.
There is no pairing for any of them: for a phone the user switches "USB tethering" on in the settings and the
phone runs DHCP on the link; a dongle just needs a cable to a router.

### Testing

`cargo xtask presets` builds `target/isos/archstaler-<preset>.iso` for all six presets (the tester included). The xHCI driver and the whole Android path run in QEMU: `cargo xtask run --usb` (`qemu-xhci` + `usb-storage`,
bulk-only INQUIRY) and `cargo xtask run --selftest --headless --nic usb-rndis` (QEMU's `usb-net` offers RNDIS in
configuration 2 and ECM in configuration 1; the selftest then does DHCP, an HTTPS download and 1 MB HTTP
transfers over the USB NIC). `--nic none` removes all NICs. The iPhone parts need a real phone: the usbmux and
lockdown framing has byte-level unit tests, the rest was verified by hand on hardware. `QEMU_EXTRA` adds raw
QEMU arguments to `xtask run` (e.g. `-trace usb_*`).

## Out of scope

Wi-Fi, USB devices other than tethering phones and Ethernet dongles, SMP, Secure Boot, an interactive
installer-kernel UI, filesystems other than ext4, architectures other than x86_64. A host desktop GUI is
implemented in `gui/` for phases 1-4 of `docs/implement-gui.md` (raw flash and AUR are not). A spec exists for Wi-Fi in `docs/wifi.md`; it is
not implemented.
