# archstaler

An Arch Linux installer written in Rust that does **not** run on top of Linux.

The ISO boots through [Limine](https://github.com/Limine-Bootloader/Limine) (BIOS and UEFI) into a small
`no_std` kernel of its own: one CPU core, polling drivers, no interrupts other than CPU exceptions. That
kernel reads a configuration that was baked into the ISO, downloads packages from an Arch mirror over
HTTPS, verifies them, lays down GPT + FAT32 + ext4 and writes the system to disk. Anything that needs a
running Linux (pacman scriptlets, alpm hooks, `mkinitcpio`, user creation) is deferred to the first boot of
the installed system.

The installer is unattended by design. There is no interactive UI: the config decides everything, and the
disk to erase is chosen either by serial number or, if you opt in, simply as the largest one.

> **Warning.** With `disk.auto_largest = true` (the setting in `presets/`), booting the ISO **erases the
> largest disk in the machine without asking.** Only boot these ISOs on hardware or VMs where that is what
> you want.

## Quick start

Prerequisites on the build host: a nightly Rust toolchain with `rust-src`, the `x86_64-unknown-none` target,
`cc`, `xorriso`, `mtools`, `gpg`, `curl`, `tar` with zstd support, and for testing `qemu-system-x86_64` with
OVMF (`/usr/share/edk2/x64`). `rust-toolchain.toml` selects the toolchain.

```sh
cargo xtask presets          # one ISO per preset -> target/isos/archstaler-<preset>.iso
cargo xtask build            # a single ISO from examples/config.lua -> target/archstaler.iso
cargo xtask build --config path/to/my.lua --out my.iso
```

### Presets

| ISO | What you get | Download | Disk |
|---|---|---|---|
| `archstaler-minimal.iso` | console system, systemd-networkd | ~0.4 GiB | 4 GiB+ |
| `archstaler-server.iso` | minimal + OpenSSH, htop, tmux, rsync, vim | ~0.4 GiB | 4 GiB+ |
| `archstaler-i3.iso` | Xorg + i3, NetworkManager, PipeWire, Firefox | ~1.0 GiB | 10 GiB+ |
| `archstaler-hyprland.iso` | Hyprland (Wayland), same extras | ~1.1 GiB | 10 GiB+ |
| `archstaler-plasma.iso` | KDE Plasma (Wayland session), same extras | ~1.3 GiB | 20 GiB+ |

Every preset installs the `ly` login manager (`ly@tty2.service`) and creates the user `passwd_is_passwd`
with the password `passwd` in group `wheel` (sudo works; root is locked). **Change that password** before
the machine is reachable from a network, especially with the server preset, which enables SSH. On the
console-only presets (minimal, server) `ly` lists both `shell` and `xinitrc` sessions; pick `shell`,
because `xinitrc` needs X and `xauth`. The shared defaults live in `presets/common.lua`; `cargo xtask
check-presets` resolves every preset against your local pacman sync databases and reports package counts,
download sizes and provider choices. All presets install the wired-NIC firmware
(`linux-firmware-intel`, `linux-firmware-realtek`) or, on the desktop presets, the full `linux-firmware`.

The presets use `disk.auto_largest = true`. For a machine with several disks, use `disk.confirm_serial`
in your own config instead (see below).

### Trying it in QEMU

```sh
cargo xtask run --bios --headless          # or --uefi; adds a scratch disk with serial TESTDISK0
cargo xtask run --uefi --disk nvme --nic e1000e
```

`xtask run` builds the ISO from `examples/config.lua` and attaches `target/test-disk.img`. Options:
`--disk virtio|ahci|nvme`, `--nic virtio|e1000|e1000e|rtl8139`, `--config FILE`, `--headless`.

### Virtual machines and USB sticks

- Firmware: BIOS or UEFI both work; **Secure Boot must be off** (neither the ISO nor the installed system
  supports it). Only x86_64 is supported; there is no ARM or Raspberry Pi support.
- The installer has **no USB drivers**. It sees NVMe, AHCI/SATA and virtio disks only, so an installer USB
  stick is never a candidate for `auto_largest`. It also means it cannot install onto a USB disk.
- Everything the installer needs is loaded into RAM by the boot loader before the kernel starts, and
  packages come from the network, so the stick can be pulled once the installer's first log lines have
  appeared. **Pull it before the final reboot**: if the firmware still prefers the stick, the machine boots
  the installer again and, with `auto_largest`, erases the system that was just installed.
- Networking: wired only, no Wi-Fi. The installer recognizes 30 PCI device IDs in three driver families:
  virtio-net (2 IDs), Intel e1000 (14) and e1000e (10), and Realtek RTL8168/8169 (4). Only virtio and the
  Intel drivers have been run, and only against QEMU's emulated NICs. The Realtek driver has never run on
  real hardware, and newer Intel chips that share an ID (for example the PCH-integrated I217/I219) may
  need setup the driver does not do.

### Ventoy

The ISOs boot from [Ventoy](https://www.ventoy.net): copy the `archstaler-*.iso` files to the Ventoy data
partition, boot the stick, pick an ISO, and choose **Boot in normal mode** (the first entry of the boot
mode menu that Ventoy shows; grub2 and memdisk mode are not needed).

This was tested with Ventoy 1.1.17 in QEMU, in both BIOS and UEFI mode, using an image laid out like a
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
| `services` | units enabled on first boot |
| `kernel_params` | appended to the kernel command line |

Config values end up in shell-read data files, unit files and boot loader configuration, so `xtask`
validates them strictly at build time and rejects anything with unexpected characters.

## How it works

1. **Boot.** Limine loads the kernel plus modules (`config.bin`, the signing keyring, `tiny-init`, Limine's
   own files for the target).
2. **Disk.** The target disk is selected. Nothing is written before this succeeds.
3. **Network.** DHCP, then `core.db` and `extra.db` over HTTPS (TLS 1.2/1.3 via `rustls` with a pure-Rust
   crypto provider and the bundled Mozilla roots).
4. **Resolve.** A pacman-compatible resolver (libalpm `vercmp`, versioned deps, soname provides, groups,
   repo priority, conflicts). Where the config leaves a provider choice open the first candidate by repo
   priority and name is used, like pacman's default answer, and the choice is logged.
5. **Partition and format.** GPT with a protective MBR: 1 MiB BIOS boot partition, ESP, root. The root is a
   write-once ext4 image (extents, xattrs, `metadata_csum`, no journal) built by `ext4w`.
6. **Packages.** Each package is streamed into `/var/cache/pacman/pkg` on the target while its SHA-256 and
   PGP signature are checked (the signature comes from the database's `%PGPSIG%`, verified against a keyring
   blob derived from the pinned `archlinux-keyring`). Only after verification is it extracted.
7. **Boot setup.** `/etc` files, a first-boot service and a small initramfs (`tiny-init` plus the kernel
   modules it needs) are written; Limine goes onto the ESP and, for BIOS, into the BIOS boot partition.
8. **First boot.** The system boots into `archstaler-firstboot.target`: it initializes the pacman keyring,
   runs `pacman -U` on the cached packages (which runs every scriptlet and hook), re-applies the
   configuration, creates users, enables services, adds an ext4 journal, builds the real initramfs, writes
   the final Limine entry and reboots into the installed system.

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
- Out of scope: Wi-Fi, USB, SMP, Secure Boot, an interactive UI, filesystems other than ext4, architectures
  other than x86_64.

## Repository layout

| Path | Contents |
|---|---|
| `kernel/` | the `no_std` installer kernel (`x86_64-unknown-none`): console, heap, exceptions, TSC clock, paging for MMIO, the install flow |
| `xtask/` | host tooling: Lua evaluation, keyring blob and its pin (`keyring.pin`), Limine fetch (pinned + sha256), ISO assembly, QEMU runs, tests |
| `config/` | config types shared by `xtask` and the kernel, plus validation |
| `crates/hal` | `BlockDevice`, `NetDevice`, `Clock`, `Rng` traits |
| `crates/drivers` | PCI, virtio-blk/net, AHCI, NVMe, e1000/e1000e, r8169 (all polled) |
| `crates/net` | smoltcp stack, HTTP/1.1 client, TLS |
| `crates/pgp-lite` | OpenPGP v4 signature verification (RSA, EdDSA) |
| `crates/pkg` | sync database parser, `vercmp`, resolver, tar/zstd/gzip readers |
| `crates/ext4w` | write-once ext4 writer |
| `crates/disk` | GPT, FAT32 writer, Limine BIOS boot code installer |
| `crates/initrd` | cpio writer, kernel module dependency resolution |
| `tiny-init/` | the initramfs `init` (raw syscalls, no libc) |
| `firstboot/` | systemd units and script for the first boot |
| `presets/`, `examples/` | Lua configs |

## Testing

```sh
cargo test --release -p pgp-lite --features std -p pkg -p ext4w -p disk -p initrd
cargo xtask linux-test                       # boots the host kernel with our initramfs and an ext4w root
cargo xtask e2e [--uefi] [--disk ahci --nic e1000]   # install onto a blank disk, then boot twice
cargo xtask e2e --config presets/i3.lua      # the same for a preset
cargo xtask size [--small] [--limit BYTES]   # ISO contents and size limit check
cargo xtask check-presets                    # resolve every preset against the local pacman databases
cargo xtask update-keyring                   # move the keyring pin to the newest release
```

The host-side tests check the crates against real tools and data: `vercmp` against `/usr/bin/vercmp`,
resolution against `pacman -Sp`, `ext4w` images with `e2fsck`/`debugfs`, GPT with `sfdisk`/`fdisk`, FAT32
with `fsck.fat`, and the BIOS installer byte-for-byte against `limine bios-install`. `xtask e2e` downloads
several hundred MiB from the Arch mirror. `--selftest` builds are destructive to every disk they see and are
meant for QEMU scratch disks only.

`--small` builds the kernel with `build-std` and immediate-abort panics: smaller, but panic messages are
lost.

## Status

Verified in QEMU (BIOS and UEFI; virtio, AHCI and NVMe disks; virtio, e1000 and e1000e NICs): install, first
boot and a second boot to a login prompt, for the `i3` preset and a minimal test config, plus the Ventoy
boot described above. The other presets resolve (`check-presets`) but have not been through a full install
in the test harness. Not verified: the Realtek driver, real hardware in general, logging in with the default
credentials, and the UEFI boot entry created by `efibootmgr` (booting works through the fallback path
`EFI/BOOT/BOOTX64.EFI`). The first-boot log is only in the journal and is not persisted.

Not supported: Wi-Fi, USB, ARM and Raspberry Pi, and installing onto anything but NVMe, SATA/AHCI and virtio
disks. The ISO is about 2.4 MB (2.26 MB with `--small`); the design notes in `PLAN.md` describe what was
aimed at and what remains.
