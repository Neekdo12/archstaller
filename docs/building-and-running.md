# Building, running and testing

How to build the installer ISOs on a host, try them in QEMU and run the test suites. The short version is in
the [README](../README.md).

## Build host prerequisites

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

## Useful commands

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
cargo run -p archstaller-gui

# Build an ISO from a custom Lua config
cargo xtask build --config path/to/my.lua --out my.iso
```

## Trying it in QEMU

```sh
cargo xtask run --bios --headless          # or --uefi; adds a scratch disk with serial TESTDISK0
cargo xtask run --uefi --disk nvme --nic e1000e
```

`xtask run` builds the ISO from `configs/config.lua` and attaches `target/test-disk.img`. Options:
`--disk virtio|ahci|nvme`, `--nic virtio|e1000|e1000e|igb|rtl8139|usb-rndis|none`, `--config FILE`, `--headless`.

## Testing

```sh
cargo test --release -p pgp-lite --features std -p pkg -p ext4w -p disk -p initrd -p hostcfg -p archstaller-gui -p loadcore
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
cargo xtask gen-luals --check                # configs/archstaller.lua matches the Rust schema
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

