# archstaler

An Arch Linux installer written in Rust that does **not** run on top of Linux.

The ISO boots through a small boot loader of its own (`boot/`, BIOS and UEFI, a few tens of KiB) into a small
`no_std` kernel of its own: one CPU core, polling drivers, no device interrupts (the only interrupt is a 1 kHz timer tick that lets idle loops halt the CPU instead of spinning). That
kernel reads a configuration that was baked into the ISO, downloads packages from an Arch mirror over
HTTPS, verifies them, lays down GPT + FAT32 + ext4 and writes the system to disk. Anything that needs a
running Linux (pacman scriptlets, alpm hooks, `mkinitcpio`, user creation) is deferred to the first boot of
the installed system.

The installer is unattended by design. There is no interactive UI: the config decides everything, and the
disk to erase is chosen either by serial number or, if you opt in, simply as the largest one.

> **Warning.** With `disk.auto_largest = true` (the setting in `presets/` and in `examples/config.lua`),
> booting the ISO **erases the largest disk in the machine without asking.** Only boot these ISOs on
> hardware or VMs where that is what you want.

This is a proof of concept for people who reinstall Arch often and want it automated. It has no prompts,
countdowns or abort keys on purpose.

## Quick start

Prerequisites on the build host: a nightly Rust toolchain with `rust-src`, the `x86_64-unknown-none` target,
`cc`, `xorriso`, `mtools`, `gpg`, `curl`, `tar` with zstd support, and for testing `qemu-system-x86_64` with
OVMF (`/usr/share/edk2/x64`). `rust-toolchain.toml` selects the toolchain.

```sh
cargo xtask build --debug    # verbose driver/network tracing (always on for the tester preset)
cargo xtask presets          # one ISO per preset -> target/isos/archstaler-<preset>.iso (with USB tethering; --no-tethering leaves it out)
cargo xtask build            # a single ISO from examples/config.lua -> target/archstaler.iso
cargo xtask build --profile large   # regular release build, panic messages kept (the default profile is `super-small`; or set `build.profile` in the config)
cargo run -p archstaler-gui         # desktop app: edit a config, check it, build the ISO, copy it to a Ventoy stick
cargo xtask build --config path/to/my.lua --out my.iso
```

### Presets

| ISO | What you get | Download | Disk |
|---|---|---|---|
| `archstaler-minimal.iso` | console system, systemd-networkd | ~0.4 GiB | 4 GiB+ |
| `archstaler-server.iso` | minimal + OpenSSH, htop, tmux, rsync, vim | ~0.4 GiB | 4 GiB+ |
| `archstaler-i3.iso` | Xorg + i3, NetworkManager, PipeWire, Firefox | ~1.0 GiB | 10 GiB+ |
| `archstaler-hyprland.iso` | Hyprland (Wayland) with kitty, rofi, waybar, quickshell, hyprlock/hypridle, Neovim (LazyVim) and Nerd fonts (FantasqueSansM, JetBrains Mono, Iosevka), plus the downloaded config zip | ~1.4 GiB | 15 GiB+ |
| `archstaler-plasma.iso` | KDE Plasma (Wayland session), same extras | ~1.3 GiB | 20 GiB+ |

Every preset installs the `ly` login manager (`ly@tty2.service`) and creates the user `passwd_is_passwd`
with the password `passwd` in group `wheel` (sudo works; root is locked). **Change that password** before
the machine is reachable from a network, especially with the server preset, which enables SSH. On the
console-only presets (minimal, server) `ly` lists both `shell` and `xinitrc` sessions; pick `shell`,
because `xinitrc` needs X and `xauth`. The shared defaults live in `presets/common.lua`; `cargo xtask
check-presets` resolves every preset against your local pacman sync databases and reports package counts,
download sizes and provider choices. All presets install the wired-NIC firmware
(`linux-firmware-intel`, `linux-firmware-realtek`) and the AMD GPU firmware (`linux-firmware-amdgpu`,
`linux-firmware-radeon`, about 30 MiB) or, on the desktop presets, the full `linux-firmware`. The first boot
builds the initramfs without the `kms` hook, so a GPU that cannot initialise (missing firmware, an unsupported
chip) does not stop the boot before the root file system is mounted.

The Hyprland preset also pulls a Hyprland config from a server during installation: `FRIEND_CONFIG` at the
top of `presets/hyprland.lua` is the `https://` URL of a zip whose contents are laid out relative to the
home directory (`.config/hypr/hyprland.lua`, `.config/hypr/modules/...`). It is extracted into every user's
home on first boot, as that user. Set it to an empty string to skip it. If the server is down, or does not
answer with a zip, the archive is skipped with a warning and Hyprland keeps its defaults. **Only point it at a
server you trust**: a Hyprland config can run arbitrary commands when the session starts, and the archive is
checked by nothing but HTTPS. The preset installs the programs the current config launches (kitty, rofi,
waybar, quickshell, awww, swaync, swayosd, hyprlock, hypridle, cliphist, Neovim with the tools LazyVim needs,
and the three Nerd fonts). Things the config refers to that are **not** installed or not in the zip: a
quickshell config, `zen-browser` and the `macOS` cursor theme (AUR only; Firefox is installed instead),
`code`, `kitty-themes`, the wallpaper `~/Images/Wallpapers/special.jpg` and `~/.local/bin/satty-screenshot`.

`presets/tester.lua` is not an installer: it is a read-only hardware test (`archstaler-tester.iso`). It probes the
machine, brings up the network, downloads the package databases, pings the gateway and 1.1.1.1, runs a download speed
test, prints PASS/FAIL with full debug output, and reboots. It never writes a disk. Use it to check whether a machine's
NIC, tethering phone or dongle works before installing.

The presets use `disk.auto_largest = true`. For a machine with several disks, use `disk.confirm_serial`
in your own config instead (see below).

### Trying it in QEMU

```sh
cargo xtask run --bios --headless          # or --uefi; adds a scratch disk with serial TESTDISK0
cargo xtask run --uefi --disk nvme --nic e1000e
```

`xtask run` builds the ISO from `examples/config.lua` and attaches `target/test-disk.img`. Options:
`--disk virtio|ahci|nvme`, `--nic virtio|e1000|e1000e|igb|rtl8139|usb-rndis|none`, `--config FILE`, `--headless`.

### Virtual machines and USB sticks

- Firmware: BIOS or UEFI both work; **Secure Boot must be off** (neither the ISO nor the installed system
  supports it). Only x86_64 is supported; there is no ARM or Raspberry Pi support.
- The installer has **no USB storage drivers**. It sees NVMe, AHCI/SATA and virtio disks only, so an installer USB
  stick is never a candidate for `auto_largest`. It also means it cannot install onto a USB disk. The only
  USB code is the optional phone tethering described under Networking.
- Everything the installer needs is loaded into RAM by the boot loader before the kernel starts, and
  packages come from the network, so the stick can be pulled once the installer's first log lines have
  appeared. **Pull it before the final reboot**: if the firmware still prefers the stick, the machine boots
  the installer again and, with `auto_largest`, erases the system that was just installed.
- Networking: wired PCI NICs, plus optional iPhone and Android USB tethering and USB Ethernet dongles (below). No Wi-Fi. The installer recognizes 71 PCI device IDs in these driver families:
  virtio-net (2 IDs), Intel e1000 (14) and e1000e (10), Intel igb (28, the 82576/82580/I350/I210/I211
  generations) and igc (10, I225/I226), Realtek RTL8168/8169 (4) and RTL8125/8126 (2), and the old Realtek
  RTL8139 (1). Tested in QEMU's emulated NICs: virtio, e1000, e1000e, igb and rtl8139, each with an
  ARP exchange, DHCP, a TLS download from the Arch mirror and a 1 MB HTTP transfer, and for igb and
  rtl8139 also the start of a real installation. **Real hardware**: the RTL8168evl/8111evl (xid 0x2c9, Gigabyte GA-F2A88XM-D3H) works, including DHCP, the
  package downloads and a full install, on a network that needed the DHCP address probe (below); the AX88179 USB
  dongle works too. **Never run**: igc, the other RTL8168/8169 revisions and RTL8125/8126, because QEMU has no
  models for them; they follow the Linux drivers' setup and may not work on a given chip revision (the RTL8125/8126
  in particular need chip-specific tuning in Linux). Some PCI IDs, especially
  igc's, were written from memory and are unchecked. Newer Intel chips that share an ID (for example the
  PCH-integrated I217/I219) may need setup the e1000e driver does not do. Not supported: Broadcom `tg3`,
  Marvell/Aquantia `atlantic`, VMware `vmxnet3` and 10 Gbit NICs.
- DHCP behaviour: the offered address is ARP-probed before use, like Linux clients do. If another host already uses it
  (a static device inside a DHCP range), the installer sends a DHCPDECLINE and asks again, up to 4 times. DNS falls
  back to 1.1.1.1 and 8.8.8.8 if the DHCP-provided servers do not answer. Old CPUs without `RDRAND` get a weaker
  timing-jitter random generator (the tester reports a warning).
- USB tethering (optional; off in a plain `cargo xtask build`, on in `cargo xtask presets` unless `--no-tethering`): build with `cargo xtask build --tethering`. It only runs when no
  wired NIC has link, and the phone then appears as one more NIC. It uses an xHCI controller (`crates/usb`).
  - **iPhone** (worked on one real iPhone): plug in an unlocked iPhone with Personal Hotspot on and tap "Trust"
    (and enter the passcode) when it asks; the installer waits up to 120 s. It pairs through lockdownd over
    the phone's usbmux interface (`crates/imobiledevice`) and then uses the phone's standard tethering
    interface, the one Linux's `ipheth` driver uses. Pairing is redone on every run.
  - **Android** (worked on one real phone, RNDIS or ECM not recorded; also tested against QEMU's emulated
    adapters): switch "USB tethering" on in the phone's settings (the phone must be unlocked), then plug it in. The
    installer scans again for about 20 s, so enabling tethering after plugging in also works. No pairing is needed.
  - **USB Ethernet dongles** (`crates/usbnet`): class-compliant adapters that offer CDC-ECM or CDC-NCM, and
    ASIX AX88179 gigabit adapters (for example the Axagon ADE-SG; vendor-specific protocol, matched by USB id).
    Plug a cable into a router first: the AX88179 driver waits up to 15 s for link. The AX88179 works on real
    hardware (link, DHCP, downloads). RNDIS and ECM have run against QEMU's emulated adapter; the NCM framing
    has host unit tests only. Other vendor-specific chips (ASIX AX88772, Realtek RTL8152/8153) are not
    supported unless the dongle also offers an ECM or NCM configuration.
  Every step prints a `usb:` log line, so a failure shows how far it got.

### Ventoy

The ISOs boot from [Ventoy](https://www.ventoy.net): copy the `archstaler-*.iso` files to the Ventoy data
partition, boot the stick, pick an ISO, and choose **Boot in normal mode** (the first entry of the boot
mode menu that Ventoy shows; grub2 and memdisk mode are not needed).

This was tested with Ventoy 1.1.17 in QEMU (while the ISO still booted through Limine; the custom boot loader that replaced it has not been tried under Ventoy yet), in both BIOS and UEFI mode, using an image laid out like a
Ventoy stick (MBR, Ventoy's boot code and EFI partition, FAT32 data partition) attached as a USB drive:
the menu lists the ISOs, the installer starts, sees only the blank target disk, and keeps installing after
the virtual stick is removed. Not tested: real hardware, an exFAT data partition (Ventoy's default; Ventoy
reads FAT32 and exFAT alike), other Ventoy modes and Ventoy's Secure Boot mode.

## Configuration

Configs are Lua files evaluated on the **build host**; the result is serialized into the ISO. See
`examples/config.lua` for a documented example and `presets/*.lua` for real ones (presets share code via
`dofile(CONFIG_DIR .. "/common.lua")`).

| Field | Meaning |
|---|---|
| `hostname`, `timezone`, `locale`, `keymap` | system identity and localization |
| `disk.auto_largest` | opt in: erase and install onto the largest disk; a tie is refused |
| `disk.confirm_serial`, `disk.model` | the safe mode: the selector must match exactly one disk and its serial must equal `confirm_serial`, otherwise nothing is written |
| `disk.esp_mib` | size of the FAT32 EFI system partition, mounted at `/boot` |
| `mirrors` | base URLs; `$repo` and `$arch` are substituted |
| `packages` | package or group names installed explicitly |
| `providers` | `{ {dep, package}, ... }`: which package provides an ambiguous dependency |
| `users` | `{ name, password_hash, groups, shell }`; hashes are SHA-512 crypt (`openssl passwd -6`), never plaintext |
| `root_password_hash` | optional; root is locked if absent |
| `user_files` | `{ { url, dest }, ... }`: `https://` files (max 1 MiB) downloaded by the installer and copied on first boot to `~/<dest>` of every configured user, owned by that user. A server that is down, a non-200 answer or an oversized file only prints a warning; the installation continues without that file |
| `user_archives` | `{ { url }, ... }`: `https://` zip archives (max 16 MiB) downloaded by the installer and extracted on first boot into the home directory of every configured user (paths inside the zip are relative to the home, e.g. `.config/hypr/hyprland.lua`), as that user. Same failure handling as `user_files`; an answer that is not a zip is skipped too |
| `services` | units enabled on first boot |
| `kernel_params` | appended to the kernel command line |
| `scripts` | `{ { id, args?, file? \| url?+sha256? }, ... }`: run as root, in order, at the end of the first boot. An `id` alone is a built-in script (`enable-sshd`, `enable-fstrim`); `file` is a local script embedded at build time (max 64 KiB); `url` must be `https://` and pinned by `sha256` (the installer fails on a mismatch). A failing script only logs a warning |
| `build.profile`, `build.tethering` | host-only: `super-small` (default) or `large` installer build; include USB tethering. `extra-large` is reserved |
| `installer_drivers` | host-only: installer drivers to include (`virtio-blk ahci nvme virtio-net e1000 igb r8169 rtl8139`); all if absent, at least one storage and one network driver (or `build.tethering`) otherwise |

Config values end up in shell-read data files, unit files and boot loader configuration, so `xtask`
validates them strictly at build time and rejects anything with unexpected characters.

## How it works

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
8. **First boot.** The system boots into `archstaler-firstboot.target`: it initializes the pacman keyring,
   runs `pacman -U` on the cached packages (which runs every scriptlet and hook), re-applies the
   configuration, creates users, enables services, builds the real initramfs, writes
   GRUB (`grub-install`, from the `grub` package, replacing our loader) and reboots into the installed system.

### Trust model and limits

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
| `crates/drivers` | PCI, virtio-blk/net, AHCI, NVMe, Intel e1000/e1000e/igb/igc, Realtek r8169/r8125/rtl8139 (all polled) |
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
| `crates/hostcfg` | host-only config model shared by `xtask` and the GUI: Lua loading, build profiles, driver and script catalogues, Lua writer, package resolution preview, build progress events |
| `gui/` | `archstaler-gui`, the desktop app (egui): config editor, ISO build, Ventoy copy |
| `presets/`, `examples/` | Lua configs |

## Testing

```sh
cargo test --release -p pgp-lite --features std -p pkg -p ext4w -p disk -p initrd -p hostcfg -p archstaler-gui -p loadcore
cargo xtask linux-test                       # boots the host kernel with our initramfs and an ext4w root
cargo xtask e2e [--uefi] [--disk ahci --nic e1000]   # install onto a blank disk, then boot twice
cargo xtask e2e --config presets/i3.lua      # the same for a preset
cargo xtask run --usb --headless            # xHCI smoke test: qemu-xhci + usb-storage, SCSI INQUIRY
cargo xtask run --selftest --headless --nic usb-rndis   # DHCP + HTTPS + 1 MB over an emulated Android RNDIS adapter
cargo xtask size [--profile large] [--limit BYTES]   # ISO contents and size limit check
cargo xtask check-presets                    # resolve every preset against the local pacman databases
cargo xtask update-keyring                   # move the keyring pin to the newest release
```

The host-side tests check the crates against real tools and data: `vercmp` against `/usr/bin/vercmp`,
resolution against `pacman -Sp`, `ext4w` images with `e2fsck`/`debugfs`, GPT with `sfdisk`/`fdisk`, FAT32
with `fsck.fat`. `xtask e2e` downloads
several hundred MiB from the Arch mirror. `xtask run` recreates its scratch disk `target/test-disk.img` on
every run (a partition table left by an earlier run would make the firmware try the disk before the ISO).
`xtask e2e` overwrites nothing of yours, but `xtask run --selftest` builds are destructive to every disk
they see and are meant for QEMU scratch disks only.

`super-small` (default profile) builds the kernel and the loaders with `build-std` and immediate-abort panics: smaller, but panic
messages are lost. The payload is always stored deflate-compressed; the ISO is about 0.77 MiB. Both firmware
paths boot in QEMU, and the BIOS path also when the ISO is written to a disk or USB stick (hybrid MBR).

## Status

Verified in QEMU (BIOS and UEFI; virtio, AHCI and NVMe disks; virtio, e1000, e1000e, igb and rtl8139 NICs):
install, first boot and a second boot to a login prompt, for the `i3` and `hyprland` presets and a minimal
test config, plus the Ventoy boot described above. The `minimal`, `server` and `plasma` presets resolve
(`check-presets`) but have not been through a full install in the test harness. Verified on real hardware: installs on an old Gigabyte GA-F2A88XM-D3H (RTL8168evl, no RDRAND) through
first boot; the second boot failed there once with a missing journal, which is fixed by writing a real internal
journal (checked with `e2fsck`/`debugfs` and in QEMU, not yet re-run on that machine). Not verified: the igc and
other Realtek drivers (other RTL8168/8169 revisions, RTL8125/8126), real hardware in general, logging in with the default
credentials, starting the Hyprland session with the downloaded config, and the UEFI boot entry created by
`efibootmgr` (GRUB is installed to the fallback path, so booting works through `EFI/BOOT/BOOTX64.EFI`). The first-boot log is only in
the journal and is not persisted.

USB tethering: the iPhone path worked on real hardware (an ASUS ExpertBook with an unlocked iPhone); the Android path (RNDIS, CDC-ECM) worked on one real phone and in QEMU's emulated adapters; the AX88179 dongle worked on real hardware. `xtask e2e` does not exercise tethering.

Not supported: Wi-Fi, USB devices other than tethering phones and Ethernet dongles, ARM and Raspberry Pi, and installing onto anything but NVMe, SATA/AHCI and virtio
disks. The ISO is about 0.7 MB (`super-small`) to 0.8 MB (`large`); tethering adds about 105 KiB; the design notes in `PLAN.md` describe what was
aimed at and what remains.
