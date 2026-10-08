# Omarchy support

**Status:** the Omarchy preset exists (`configs/omarchy.lua`, derived from Omarchy v4.0.4) and resolves with
`cargo xtask check-presets` (896 packages, about 2.4 GiB). It is the official-repository subset of Omarchy's package
list, with `ly` instead of `sddm`, and without Omarchy's own scripts, themes and dotfiles, because those come from
Omarchy's repository or the AUR, which the installer does not use for presets. `cargo xtask e2e --config configs/omarchy.lua --disk-gib 24`
passes in QEMU (BIOS, virtio): install, first boot (`pacman -U` of all 906 packages) and a second boot to the
`omarchy login:` prompt. It does not start Hyprland or log in, and the default 16 GiB test disk is too small for it.
Use a disk of 24 GiB or more.

This document describes how to support Arch-based systems that ship only a different package selection
and configuration, such as Omarchy. Distributions with their own repositories or keys are out of scope. The installer kernel remains Archstaler's own, while package selection and
system configuration vary by preset.

## Decide what "same kernel" means

Keep the **Archstaler installer kernel** the same for every preset. It is the `no_std` kernel built for
`x86_64-unknown-none`; it does not become an Arch or Omarchy kernel. The installed system
still needs its own bootable Linux kernel and matching initramfs.

Omarchy is an Arch-based system and should start as a normal Archstaler preset: same installer kernel,
Arch package pipeline, different package selection and system configuration.

## Current boundary

The existing preset mechanism is the files marked `-- archstaler: kind=preset` in `configs/`, with shared defaults in `configs/common.lua`. The
presets supply package names and system configuration; they do not select a different installer kernel.
The GUI and CLI already load these configs through `hostcfg`.

Several current assumptions must be addressed when adding Arch-derived distribution support:

- `kernel/src/install.rs` fetches only the `core` and `extra` databases and resolves Arch package names.
- Package downloads are verified against the embedded Arch keyring, unpacked into the target root, and
  cached for first boot.
- `firstboot/firstboot.sh` runs `pacman-key --populate archlinux` and installs the cache with `pacman -U`.
- System setup writes Arch-specific mirror and package state. First boot builds an initramfs with
  `mkinitcpio` and writes a GRUB entry for `/vmlinuz-linux` and `/initramfs-linux.img`.
- The package resolver and package format are not a general distribution abstraction. Do not make
  non-Arch support appear to work by accepting arbitrary mirror URLs alone.

## Omarchy preset

Implement Omarchy first, reusing the Arch backend and adding no new kernel code.

1. Add `configs/omarchy.lua` (first line `-- archstaler: kind=preset`). Reuse `configs/common.lua` for shared Arch packages, users, disk setup,
   and standard configuration. Keep the Omarchy package list and any supported configuration files in
   the Omarchy preset or small shared Lua modules; do not fork all common defaults.
2. Derive the package list and configuration from a documented, pinned Omarchy release. Prefer normal
   Arch packages and Arch repositories where possible. Record any packages that require another
   repository and do not silently run an upstream installer script as root.
3. Use the existing `user_files`, `user_archives`, and built-in `scripts` mechanisms for user-level
   configuration only where they are appropriate. Pin remote content by hash when the mechanism
   supports it; treat unpinned desktop configuration as executable code and do not fetch it implicitly.
4. Ensure the selected installed kernel, boot package, initramfs generator, login manager, and enabled
   services agree. Do not assume that an Omarchy desktop package set also provides Archstaler's current
   `linux`, `mkinitcpio`, or `grub` package choices.
5. Add the preset to the preset build/check path and GUI preset menu through the existing preset
   discovery mechanism. Update examples only if they document preset selection.
6. Validate resolution against current Arch sync databases, build the ISO, and run a QEMU end-to-end
   install. Inspect first-boot logs and verify that the installed machine reaches the intended login
   session with the expected user configuration.

Omarchy support is a preset, not a claim that Archstaler reproduces every step of the upstream Omarchy
installer. Document omissions and the upstream release used to derive the package/configuration set.

## Shared completion criteria

- `OVERVIEW.md` and the user-facing preset documentation state which backend each target uses and what
  is not supported.
- Arch's current presets still resolve and pass their existing end-to-end test unchanged.
- Each advertised target has a pinned package/system source, a defined signing/trust policy, a supported
  installed kernel and initramfs path, and a repeatable boot test.
- Run `cargo xtask check-presets` for package-based presets. Run `cargo xtask size` if the ISO payload,
  kernel, or bundled data changes.
- Never imply that Omarchy preset coverage is complete until the corresponding install-and-boot test passes.