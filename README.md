# archstaller

### Wanted to install Arch without babysitting an installer?

**Fear no more.**

**archstaller** is a tiny, unattended Arch Linux installer written in Rust.

And by tiny, I mean **the installer ISO can be smaller than a single photo you took today.**

It doesn't boot Linux to run `archinstall`.
It doesn't need a shell.
It doesn't need an interactive UI.

It boots its own tiny bootloader, runs its own `no_std` kernel, connects to an Arch mirror, verifies the packages, partitions the disk, installs Arch, and hands everything over to the real system on first boot.

Basically:

> **Boot → install → reboot → Arch.**

No clicking through 47 questions. No "are you sure?" No babysitting.

---

## ⚠️ This thing is destructive

The default presets use:

```lua
disk.auto_largest = true
```

That means **the largest disk is erased automatically.**

There is no confirmation dialog.

There is no emergency "NOOO STOP" button.

This is intentional.

**Only boot the installer on a machine or VM where you actually want the selected disk erased.**

If you want a safer setup, select a disk by its serial number with `disk.confirm_serial`.

---

## What can it install?

There are several presets:

| Preset     | What you get                               | Approx. download size |
| ---------- | ------------------------------------------ | ----------: |
| `minimal`  | Console + systemd-networkd                 |    ~0.4 GiB |
| `server`   | Minimal + SSH, tmux, vim, rsync, htop      |    ~0.4 GiB |
| `i3`       | Xorg + i3 + Firefox + PipeWire             |    ~1.0 GiB |
| `sway`     | Sway + Waybar + Wofi + Firefox             |    ~1.1 GiB |
| `hyprland` | Hyprland + Kitty + Waybar + Neovim/LazyVim |    ~1.4 GiB |
| `plasma`   | KDE Plasma + desktop extras                |    ~1.3 GiB |
| `omarchy`  | Omarchy-inspired package set on Hyprland   |    ~2.4 GiB |

Every preset comes with `ly` and a default user.

**Change the default password before putting the installed machine on a network.**

---

## The fun part

archstaller doesn't rely on Linux being already there.

The installer has its own:

* x86_64 bootloader
* `no_std` kernel
* storage drivers
* network drivers
* DHCP
* DNS
* HTTPS/TLS
* Arch package database parser
* dependency resolver
* PGP package verification
* GPT + FAT32 + ext4 writers
* first-boot system setup

The actual installed Linux system takes over on the first boot.

The installer is basically a tiny temporary operating system whose only job is:

> **put another operating system on this disk.**

---

## Quick start

You'll need a nightly Rust toolchain with `rust-src`, the `x86_64-unknown-none` target, and the usual ISO/QEMU tooling.

Build an ISO:

```sh
cargo xtask build
```

Build every preset:

```sh
cargo xtask presets
```

Run it in QEMU:

```sh
cargo xtask run --uefi --headless
```

Build from your own config:

```sh
cargo xtask build --config path/to/my.lua --out my.iso
```

The GUI is available too:

```sh
cargo run -p archstaller-gui
```

---

## Configuration

Installations are defined using Lua:

```lua
return {
    as = as
}
```

You can configure the hostname, disk, packages, users, services, kernel parameters, first-boot scripts, mirrors, drivers, USB tethering and more.

For the full configuration reference, see [`docs/lua-config.md`](docs/lua-config.md).

The build system validates the configuration before putting it into the ISO, so typos don't silently become mystery installer behavior.

---

## Networking

No Wi-Fi.

But wired Ethernet is supported across a surprisingly stupid number of NICs.

USB networking can also be enabled for:

* Android tethering
* iPhone tethering
* USB Ethernet adapters

So yes, in the right setup, you can install Arch through your **phone's USB connection**.

---

## Hardware support

archstaller currently targets:

* x86_64
* NVMe
* SATA/AHCI
* virtio disks
* BIOS and UEFI

It does **not** support:

* Wi-Fi
* ARM / Raspberry Pi
* Secure Boot
* USB storage as an installation target

See the documentation for the current list of tested NICs and hardware.

---

## Testing

Most development happens in QEMU.

Useful commands:

```sh
cargo test --release
cargo xtask linux-test
cargo xtask e2e
cargo xtask check-presets
```

There is also a hardware tester ISO which probes storage/network hardware without touching the disk:

```text
archstaller-tester.iso
```

---

## Why?

Because apparently the reasonable response to wanting a simpler Arch installer was:

> **write an operating system.**

So that's what happened.

---

## Status

archstaller is a **proof of concept / very much a work in progress**.

It already installs Arch in QEMU and has been tested on real hardware, including USB tethering and USB Ethernet.

Some drivers, presets and real-hardware combinations are still unverified.

If you want to know whether your particular machine is going to work, **read the hardware support documentation before nuking your disk.**

---

## Documentation

More detailed stuff lives here:

* [`docs/lua-config.md`](docs/lua-config.md): complete Lua configuration reference
* [`docs/editor-setup.md`](docs/editor-setup.md): config files, Lua Language Server setup for VS Code and Neovim
* [`docs/presets.md`](docs/presets.md): what each preset installs, default user, the Hyprland config download, the hardware tester
* [`docs/building-and-running.md`](docs/building-and-running.md): build prerequisites, commands, QEMU, test suites
* [`docs/gui-app.md`](docs/gui-app.md): the GTK desktop app (building, features, ISO build, Ventoy copy, USB flashing)
* [`docs/hardware-and-networking.md`](docs/hardware-and-networking.md): supported NICs, DHCP, USB tethering, USB sticks, Ventoy
* [`docs/architecture.md`](docs/architecture.md): how the installer works, trust model, repository layout
* [`docs/status.md`](docs/status.md): what was verified in QEMU and on real hardware
* [`plans-implement/aur.md`](plans-implement/aur.md): AUR support
* [`plans-implement/`](plans-implement/): implementation plans and specs (AUR, GTK GUI, raw USB flashing, Sway preset, Wi-Fi, network driver coverage, Omarchy, typed Lua config)
* [`PLAN.md`](PLAN.md): original design notes and future plans

The README is intentionally short.

**If you need the 400-line version, that's what `docs/` is for.**

---

## License

[MIT](LICENSE).
