# Omarchy and CachyOS support

This document describes how to extend Archstaler to other Arch-derived systems. Only Arch-based
distributions are in scope. The installer kernel remains Archstaler's own, while package selection and
system configuration vary by preset.

## Decide what "same kernel" means

Keep the **Archstaler installer kernel** the same for every preset. It is the `no_std` kernel built for
`x86_64-unknown-none`; it does not become an Arch, CachyOS, or Omarchy kernel. The installed system
still needs its own bootable Linux kernel and matching initramfs.

Omarchy is an Arch-based system and should start as a normal Archstaler preset: same installer kernel,
Arch package pipeline, different package selection and system configuration. CachyOS is also
Arch-compatible in package format, but has its own repositories, signing keys, and kernel choices; it
needs explicit repository and trust support before it can be called supported.

## Current boundary

The existing preset mechanism is `presets/*.lua`, with shared defaults in `presets/common.lua`. The
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

1. Add `presets/omarchy.lua`. Reuse `presets/common.lua` for shared Arch packages, users, disk setup,
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

## CachyOS backend support

Treat CachyOS as a distinct Arch-compatible package source, not as an Arch preset with an arbitrary
mirror override. First establish that the repository database and package archives use formats the
existing parser and extractor correctly handle.

1. Add explicit host-side source configuration for the selected CachyOS repositories and mirrors. Keep
   source selection typed and validated; do not infer trust from a URL or allow a preset to replace the
   Arch keyring.
2. Extend the shared config contract and host validation only for the source information the installer
   needs. Keep host-only build settings in `crates/hostcfg`; keep values needed by the kernel in
   `config/src/lib.rs`, and update serialization, validation, and deserialization together.
3. Add a pinned CachyOS signing-key source and verification path. Verify sync database authenticity if
   provided by the repository, and verify package signatures before extraction. Define how Arch and
   CachyOS keys are distinguished and rotated; do not trust every key in an unreviewed keyring.
4. Generalize repository database fetching/resolution in `kernel/src/install.rs` without changing the
   default Arch behavior. Preserve repository priority and provider selection, and ensure package URLs
   are derived only from validated configured sources.
5. Keep the installed package manager state consistent with the chosen repositories and signing keys so
   later `pacman` operations do not silently switch to an unrelated Arch configuration.
6. Add a CachyOS preset with a coherent kernel, firmware, initramfs, and bootloader package set. Make the
   chosen CachyOS kernel path explicit; the current GRUB generation assumes `/vmlinuz-linux` and
   `/initramfs-linux.img` and must not be reused unchanged if CachyOS uses different filenames.
7. Add host tests for source validation and repository resolution, signature tests using pinned public
   fixtures, plus a QEMU end-to-end install that confirms the target boots with the selected kernel.

Do not mix Arch and CachyOS repositories by default. If mixed repositories are ever supported, define
repository priority, package replacement/conflict rules, and signing policy explicitly and test them.

## Shared completion criteria

- `OVERVIEW.md` and the user-facing preset documentation state which backend each target uses and what
  is not supported.
- Arch's current presets still resolve and pass their existing end-to-end test unchanged.
- Each advertised target has a pinned package/system source, a defined signing/trust policy, a supported
  installed kernel and initramfs path, and a repeatable boot test.
- Run `cargo xtask check-presets` for package-based presets. Run `cargo xtask size` if the ISO payload,
  kernel, or bundled data changes.
- Never imply that Omarchy preset coverage or CachyOS repository support is complete until the
   corresponding install-and-boot test passes.