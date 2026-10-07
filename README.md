# archstaler

## Overview

An Arch Linux installer written in Rust that does not run on top of Linux.

### How it works

- The ISO boots through a small boot loader of its own (`boot/`, BIOS and UEFI, a few tens of KiB) into a small `no_std` kernel of its own.
- It runs on a single CPU core, uses polling drivers, and has no device interrupts. The only interrupt is a 1 kHz timer tick that lets idle loops halt the CPU instead of spinning.
- That kernel reads a configuration baked into the ISO, downloads packages from an Arch mirror over HTTPS, verifies them, lays down GPT + FAT32 + ext4, and writes the system to disk.
- Anything that needs a running Linux, such as `pacman` scriptlets, ALPM hooks, `mkinitcpio`, and user creation, is deferred to the first boot of the installed system.

### Design goals

- The installer is unattended by design.
- There is no interactive UI: the config decides everything.
- The disk to erase is chosen either by serial number or, if you opt in, simply as the largest one.

> **Warning.** With `disk.auto_largest = true` (the setting in the presets and in `configs/config.lua`),
> booting the ISO **erases the largest disk in the machine without asking.** Only boot these ISOs on
> hardware or VMs where that is what you want.

This is a proof of concept for people who reinstall Arch often and want it automated. It has no prompts,
countdowns, or abort keys on purpose.

## Quick start

### Build host prerequisites

On the build host, you need:

- a nightly Rust toolchain with `rust-src`
- the `x86_64-unknown-none` target
- `cc`
- `xorriso`
- `mtools`
- `gpg`
- `curl`
- `tar` with zstd support
- `qemu-system-x86_64` with OVMF (`/usr/share/edk2/x64`) for testing

`rust-toolchain.toml` selects the toolchain.

### Useful commands

```sh
# Build a debug ISO with verbose driver/network tracing
cargo xtask build --debug

# Review an AUR package and print the pinned aur_packages entry
cargo xtask aur-pin NAME [--commit C]

# Build one ISO per preset
cargo xtask presets

# Build a single ISO from configs/config.lua
cargo xtask build

# Regular release build; panic messages kept
cargo xtask build --profile large

# Start the GTK app
cargo run -p archstaler-gui

# Build an ISO from a custom Lua config
cargo xtask build --config path/to/my.lua --out my.iso
```

### The desktop app

`archstaler-gui` is a GTK 4 app for Linux (Windows and macOS are not supported). It needs the `gtk4` and
`gtksourceview5` libraries at runtime (Arch: `pacman -S gtk4 gtksourceview5`) and their development files plus
`pkg-config` to build it. Run it from inside the checkout, or set `ARCHSTALER_ROOT`; it finds the presets and
builds the ISO through `cargo xtask`.

With a prebuilt binary:

```sh
cargo build --profile gui -p archstaler-gui
target/gui/archstaler-gui
```

Rebuild after every code change.

#### Features

- **Pages:** System, Disk, Mirrors & packages (with a resolve preview and the AUR group), Users, Services & kernel,
  Build & drivers, Scripts, Lua source, Build ISO. A red dot in the sidebar marks a page with a validation
  problem; the bar at the bottom shows the first one with a button that goes there. The checks are the command
  line's.
- **Menus and keys:** File menu, From preset, Tools. `Ctrl+N` new, `Ctrl+O` open, `Ctrl+S` save,
  `Ctrl+Shift+S` save as, `Ctrl+K` (or `/` outside a text field) opens the command launcher, which lists the
  same actions plus every page and preset.
- **Presets:** open as an unsaved config and say so when they erase the largest disk.
- **Lua source:** shows the generated Lua with syntax highlighting. "Edit source" switches to source mode:
  the form pages are unavailable, you edit the text (undo, search with `Ctrl+F`, `Ctrl+Space` suggestions for
  field names, driver IDs, built-in scripts, profile names, package and service names), and the diagnostics line
  shows the loader's answer, with "Go to line" when the message names one. "Return to the forms" works when
  the text is well formed and passes the loader's shape and type checks. An incomplete config, such as one with
  no disk chosen, comes back and the problem bar says what is missing. A file the GUI did not write opens in
  source mode and is replaced by form output only after you confirm. Suggestions are hints: the loader decides
  what a config means.
- **Build ISO:** builds from the saved file (an unsaved config is saved first, or built from a temporary copy
  when the target is a Ventoy drive), shows the stages and the log, can cancel, and copies the finished ISO
  onto a mounted Ventoy volume as a file with a SHA-256 read-back check. Without a Ventoy drive it can also copy
  into a folder of any mounted volume, or, only when you press **Flash ISO to USB (erases device)**, write the
  ISO over a whole USB stick: the dialog lists only removable USB disks that hold neither the running system nor
  the ISO, preselects none, and starts after you type the disk's name (`sdb`). The stick is unmounted and opened
  through UDisks2 (your desktop asks for your password), written from its first byte, read back and compared by
  SHA-256, then powered off. This erases everything on the stick and is Linux only; it needs UDisks2 2.7.3 or
  newer and util-linux 2.37 or newer. Not yet tested on a real stick.
- **Theme and size:** the window is dark by default, whatever the system theme says
  (it sets `GTK_THEME=Adwaita:dark` at start unless you set `GTK_THEME` yourself; `ARCHSTALER_THEME=system` follows the system, `ARCHSTALER_THEME=light` forces light). Its default size is
  90% of the screen at most. A window narrower than 760 px hides the sidebar (the header's toggle brings it
  back), and long labels wrap, so it works in a tiling-window-manager tile or a small laptop screen; the
  narrowest tested width was 600 px. A second copy can be started next to the first.
- **AUR packages:** one compact card: search, a result row with Review, then the recipe with its automatic
  checks folded behind a count, risky lines highlighted, and one acknowledgement before "Pin and add". NetworkManager is added automatically when no network
  service is enabled, because the build needs one.
- `ARCHSTALER_PAGE=<id>` (`system disk packages users services build scripts lua iso`) opens the window on a page.

#### Manual smoke test

Manual check (no display server in the tests): start the app; open a preset from the menu and see the warning;
change the hostname and save, reopen the file; open Lua source, choose "Edit source", break the text and see
the error and its line, fix it and return to the forms; resolve dependencies; build an ISO from a preset and
see the stages finish; press `Ctrl+K` and run a command. The text widgets, the launcher and the builds were
checked this way on GTK 4.22 with GtkSourceView 5.20 in a dark theme (a light theme was looked at on one
page); keyboard-only use with a screen reader and other desktops was not.

### Presets

| ISO | What you get | Download | Disk |
|---|---|---|---|
| `archstaler-minimal.iso` | console system, systemd-networkd | ~0.4 GiB | 4 GiB+ |
| `archstaler-server.iso` | minimal + OpenSSH, htop, tmux, rsync, vim | ~0.4 GiB | 4 GiB+ |
| `archstaler-i3.iso` | Xorg + i3, NetworkManager, PipeWire, Firefox | ~1.0 GiB | 10 GiB+ |
| `archstaler-sway.iso` | Sway (Wayland) with waybar, wofi, foot, mako, swaylock/swayidle, portals, NetworkManager, PipeWire, Firefox; a first-boot script writes `~/.config/sway/config` (`configs/sway-config.sh`). Not yet resolved or install-tested | ~1.1 GiB (estimate) | 10 GiB+ |
| `archstaler-hyprland.iso` | Hyprland (Wayland) with kitty, rofi, waybar, quickshell, hyprlock/hypridle, Neovim (LazyVim) and Nerd fonts (FantasqueSansM, JetBrains Mono, Iosevka), plus the downloaded config zip | ~1.4 GiB | 15 GiB+ |
| `archstaler-plasma.iso` | KDE Plasma (Wayland session), same extras | ~1.3 GiB | 20 GiB+ |
| `archstaler-omarchy.iso` | Hyprland with the application set of Omarchy v4.0.4, official-repository packages only (not Omarchy itself: no Omarchy scripts, themes or dotfiles; the omitted packages are listed in `configs/omarchy.lua`). Installs and boots to the login prompt in QEMU | ~2.4 GiB | 24 GiB+ |

Every preset installs the `ly` login manager (`ly@tty2.service`) and creates the user `passwd_is_passwd`
with the password `passwd` in group `wheel` (sudo works; root is locked). **Change that password** before
the machine is reachable from a network, especially with the server preset, which enables SSH. On the
console-only presets (minimal, server) `ly` lists both `shell` and `xinitrc` sessions; pick `shell`,
because `xinitrc` needs X and `xauth`. The shared defaults live in `configs/common.lua`; `cargo xtask
check-presets` resolves every preset against your local pacman sync databases and reports package counts,
download sizes and provider choices. All presets install the wired-NIC firmware
(`linux-firmware-intel`, `linux-firmware-realtek`) and the AMD GPU firmware (`linux-firmware-amdgpu`,
`linux-firmware-radeon`, about 30 MiB) or, on the desktop presets, the full `linux-firmware`. The first boot
builds the initramfs without the `kms` hook, so a GPU that cannot initialise (missing firmware, an unsupported
chip) does not stop the boot before the root file system is mounted.

The Hyprland preset also pulls a Hyprland config from a server during installation: `FRIEND_CONFIG` at the
top of `configs/hyprland.lua` is the `https://` URL of a zip whose contents are laid out relative to the
home directory (`.config/hypr/hyprland.lua`, `.config/hypr/modules/...`). It is extracted into every user's
home on first boot, as that user. Set it to an empty string to skip it. If the server is down, or does not
answer with a zip, the archive is skipped with a warning and Hyprland keeps its defaults. **Only point it at a
server you trust**: a Hyprland config can run arbitrary commands when the session starts, and the archive is
checked by nothing but HTTPS. The preset installs the programs the current config launches (kitty, rofi,
waybar, quickshell, awww, swaync, swayosd, hyprlock, hypridle, cliphist, Neovim with the tools LazyVim needs,
and the three Nerd fonts). Things the config refers to that are **not** installed or not in the zip: a
quickshell config, `zen-browser` and the `macOS` cursor theme (AUR only; Firefox is installed instead),
`code`, `kitty-themes`, the wallpaper `~/Images/Wallpapers/special.jpg` and `~/.local/bin/satty-screenshot`.

`configs/tester.lua` is not an installer: it is a read-only hardware test (`archstaler-tester.iso`). It probes the
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

`xtask run` builds the ISO from `configs/config.lua` and attaches `target/test-disk.img`. Options:
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
- Networking: wired PCI NICs, plus optional iPhone and Android USB tethering and USB Ethernet dongles (below). No Wi-Fi. The installer drives these families (the exact PCI ids are in each driver):
  virtio-net; Intel e1000 (every id of Linux's `e1000`: 8254x) and e1000e (82571-82574, 82583, 80003ES2LAN, ICH8-ICH10,
  PCH 82577-82579, I217, I218, I219) and igb/igc (82576, 82580, I350, I210/I211, I225/I226); Realtek RTL8101/8102/8103/
  8105/8106 (the 100 Mbit/s 8136), RTL8168/8111 steppings b, c, cp, d, dp, e, evl, f, g, h, ep, RTL8411, RTL8169
  s/sb/sc, RTL8125/8126 and RTL8139; Qualcomm Atheros AR8131/8132/8151/8152 (atl1c) and AR8161/8162/8171/8172 and
  Killer E2200/E2400/E2500 (alx); VMware vmxnet3. Realtek chips are identified by their TxConfig revision and get
  Linux's per-revision start sequence (the 8168/8111 revision is printed in the log). Tested in QEMU's emulated NICs:
  virtio, e1000, e1000e, igb, rtl8139 and vmxnet3, each with an ARP exchange, DHCP, a TLS download from the Arch
  mirror and a 1 MB HTTP transfer, and for igb and rtl8139 also the start of a real installation. **Real hardware**:
  the RTL8168evl/8111evl (xid 0x2c9, Gigabyte GA-F2A88XM-D3H) works, including DHCP, the package downloads and a full
  install, on a network that needed the DHCP address probe (below); the AX88179 USB dongle works too. **Never run**:
  igc, every other Realtek revision (the new start sequences were ported from the Linux driver without the chips),
  the Atheros drivers and the PCH-integrated Intel parts from the I217 on, because QEMU has no models for them;
  they follow the Linux drivers' setup and may not work on a given chip revision (the RTL8125/8126 in particular
  need chip-specific tuning in Linux). Some PCI IDs, especially igc's, were written from memory and are unchecked.
  When a NIC is not recognized the installer prints every storage and network PCI device it found (`pci 02:00.0
  10ec:8136 class 020000`). Not supported: Broadcom `tg3`/`bnx2`, Marvell Yukon/`sky2`, JMicron, nVidia nForce,
  VIA, SiS, Aquantia and 10 Gbit NICs.
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

Configs are Lua files evaluated on the **build host**; the result is serialized into the ISO. A config
returns one table with a single namespace, `as` (`return { as = as }`). `configs/config.lua` is a documented
example, `configs/e2e.lua` is the config of `xtask e2e`, and `configs/*.lua` marked
`-- archstaler: kind=preset` on their first line are the presets (`configs/common.lua` holds their shared
code and is not one). Modules beside a config load with `require("common")`.

`docs/lua-config.md` is the full reference for every setting, with checked examples; it is written to be pasted whole into a chatbot. All Lua files live flat in `configs/`. The Lua Language Server types (`configs/archstaler.lua`, generated from
the Rust schema by `cargo xtask gen-luals`; `--check` fails when it is stale) and the root `.luarc.json` give
completion and diagnostics for the `as` table in VS Code and Neovim/LazyVim (point `lua_ls` at the repository
root; it reads `.luarc.json`). The editor only helps while typing: the build rejects unknown keys and wrong
value types with the full path, for example `as.system.hostname: invalid type: integer 5, expected a string`
(list positions count from 1).

Editor setup (the Lua Language Server, `lua-language-server`, 3.x; `.luarc.json` at the repository root already
holds the settings, so open the repository root as the workspace):

- **VS Code:** install the "Lua" extension (sumneko.lua). Nothing else to configure.
- **Neovim / LazyVim:** install `lua-language-server` (Arch: `pacman -S lua-language-server`) and enable the
  standard `lua_ls` server (LazyVim: add `lua_ls` through the `lang.lua` extra, or `opts.servers.lua_ls = {}`
  in an `nvim-lspconfig` spec). It must start with the repository root as `root_dir`: lspconfig picks the
  directory holding `.luarc.json` on its own. If the server does not pick the file up, set
  `settings = { Lua = { workspace = { library = { "configs" } }, runtime = { version = "Lua 5.4" } } }`.
  No custom plugin is needed.
- **Without an editor:** `lua-language-server --check . --configpath .luarc.json` prints the same diagnostics
  for every file (wrong value types, missing required fields, values outside `"super-small"|"large"` or the
  driver ids). A misspelled key shows up as the missing required field it should have been.

Diagnostics and completion come from the generated `configs/archstaler.lua`; they are hints, and the build is
what accepts or rejects a config. Package names cannot be a closed list, so they are not completed.

The old flat layout (`return { hostname = ..., disk = ... }`) is still read, with a warning, for one migration
window; files written by the GUI always use the `as` layout. A table that mixes `as` with flat keys is refused.

| Field | Meaning |
|---|---|
| `as.schema` | must be `1` |
| `as.system.hostname`, `timezone`, `locale`, `keymap` | system identity and localization |
| `as.system.root_password_hash` | optional; root is locked if absent |
| `as.install.disk.auto_largest` | opt in: erase and install onto the largest disk; a tie is refused |
| `as.install.disk.confirm_serial`, `model` | the safe mode: the selector must match exactly one disk and its serial must equal `confirm_serial`, otherwise nothing is written |
| `as.install.disk.esp_mib` | size of the FAT32 EFI system partition, mounted at `/boot` |
| `as.install.mirrors` | base URLs; `$repo` and `$arch` are substituted |
| `as.install.dry_run` | hardware test mode (no disk is written) |
| `as.packages.explicit` | package or group names installed explicitly |
| `as.packages.providers` | `{ dep = "package", ... }`: which package provides an ambiguous dependency (written sorted by name) |
| `as.packages.aur` | pinned AUR recipes built on the installed system's first boot, see `docs/aur.md` |
| `as.users` | `{ { name, password_hash, groups, shell }, ... }`; hashes are SHA-512 crypt (`openssl passwd -6`), never plaintext |
| `as.first_boot.user_files` | `{ { url, dest }, ... }`: `https://` files (max 1 MiB) downloaded by the installer and copied on first boot to `~/<dest>` of every configured user, owned by that user. A server that is down, a non-200 answer or an oversized file only prints a warning; the installation continues without that file |
| `as.first_boot.user_archives` | `{ { url }, ... }`: `https://` zip archives (max 16 MiB) downloaded by the installer and extracted on first boot into the home directory of every configured user (paths inside the zip are relative to the home, e.g. `.config/hypr/hyprland.lua`), as that user. Same failure handling as `user_files`; an answer that is not a zip is skipped too |
| `as.first_boot.services` | units enabled on first boot |
| `as.first_boot.kernel_params` | appended to the kernel command line |
| `as.first_boot.scripts` | `{ { id, args?, file? \| url?+sha256? }, ... }`: run as root, in order, at the end of the first boot. An `id` alone is a built-in script (`enable-sshd`, `enable-fstrim`); `file` is a local script embedded at build time (max 64 KiB); `url` must be `https://` and pinned by `sha256` (the installer fails on a mismatch). A failing script only logs a warning |
| `as.build.profile`, `as.build.tethering` | host-only: `super-small` (default) or `large` installer build; include USB tethering. `extra-large` is reserved |
| `as.build.installer_drivers` | host-only: installer drivers to include (`virtio-blk ahci ata nvme virtio-net vmxnet3 e1000 igb r8169 rtl8139 alx`); all if absent, at least one storage and one network driver (or `build.tethering`) otherwise |

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
| `crates/aurbuild` | host-only AUR support: search, review, pin and plan (the packages are built on the installed system, see `docs/aur.md`) |
| `crates/hostcfg` | host-only config model shared by `xtask` and the GUI: Lua loading, build profiles, driver and script catalogues, Lua writer, package resolution preview, build progress events |
| `gui/` | `archstaler-gui`, the GTK4 desktop app (Linux): config editor, Lua source editor, ISO build, Ventoy copy, raw USB flash |
| `configs/` | Lua configs: example, e2e, presets, shared modules, generated LuaLS types |

## Testing

```sh
cargo test --release -p pgp-lite --features std -p pkg -p ext4w -p disk -p initrd -p hostcfg -p archstaler-gui -p loadcore
cargo xtask linux-test                       # boots the host kernel with our initramfs and an ext4w root
cargo xtask e2e [--uefi] [--disk ahci|nvme|ide --nic e1000]   # install onto a blank disk, then boot twice
cargo xtask e2e --config configs/i3.lua      # the same for a preset
cargo xtask e2e --config configs/omarchy.lua --disk-gib 24   # big presets need a bigger test disk (default 16 GiB)
cargo xtask e2e --config configs/e2e-aur.lua   # plus one AUR package, built and installed on the second boot
cargo xtask run --usb --headless            # xHCI smoke test: qemu-xhci + usb-storage, SCSI INQUIRY
cargo xtask run --selftest --headless --nic usb-rndis   # DHCP + HTTPS + 1 MB over an emulated Android RNDIS adapter
cargo xtask size [--profile large] [--limit BYTES]   # ISO contents and size limit check
cargo xtask check-presets                    # resolve every preset against the local pacman databases
cargo xtask coverage [--era 2010-2019]     # wired-network id coverage of the installer drivers, by entry, popularity weight and estimated hardware era (coverage/*.tsv)
cargo xtask gen-luals --check                # configs/archstaler.lua matches the Rust schema
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

Verified in QEMU (BIOS and UEFI; virtio, AHCI, IDE-mode and NVMe disks; virtio, e1000, e1000e, igb and rtl8139 NICs):
install, first boot and a second boot to a login prompt, for the `i3` and `hyprland` presets and a minimal
test config, plus the Ventoy boot described above. The `minimal`, `server` and `plasma` presets resolve
(`check-presets`) but have not been through a full install in the test harness. Verified on real hardware: installs on an old Gigabyte GA-F2A88XM-D3H (RTL8168evl, no RDRAND) through
first boot; the second boot failed there once with a missing journal, which is fixed by writing a real internal
journal (checked with `e2fsck`/`debugfs` and in QEMU, not yet re-run on that machine). Not verified: the igc, Atheros and
new Realtek paths (other RTL8168/8169/8101 revisions, RTL8125/8126), real hardware in general, logging in with the default
credentials, starting the Hyprland session with the downloaded config, and the UEFI boot entry created by
`efibootmgr` (GRUB is installed to the fallback path, so booting works through `EFI/BOOT/BOOTX64.EFI`). The first-boot log is only in
the journal and is not persisted.

USB tethering: the iPhone path worked on real hardware (an ASUS ExpertBook with an unlocked iPhone); the Android path (RNDIS, CDC-ECM) worked on one real phone and in QEMU's emulated adapters; the AX88179 dongle worked on real hardware. `xtask e2e` does not exercise tethering.

Not supported: Wi-Fi, USB devices other than tethering phones and Ethernet dongles, ARM and Raspberry Pi, and installing onto anything but NVMe, SATA/AHCI and virtio
disks. The ISO is about 0.7 MB (`super-small`) to 0.8 MB (`large`); tethering adds about 105 KiB; the design notes in `PLAN.md` describe what was
aimed at and what remains.
