# Contributing to archstaller

Thanks for helping. archstaller is an unattended Arch Linux installer that runs on its own `no_std` kernel, so
a few rules differ from a typical Rust project. The short version of them is in [`AGENTS.md`](../AGENTS.md);
the architecture is in [`OVERVIEW.md`](../OVERVIEW.md) and [`docs/architecture.md`](../docs/architecture.md).

## Before you start

- **Small and focused wins.** Open an issue first for anything that adds a driver, a preset, a config field
  or a dependency, so the size budget and the scope can be discussed.
- **Out of scope** unless a spec in [`plans-implement/`](../plans-implement/) asks for it: Wi-Fi, SMP, Secure Boot,
  an interactive UI, file systems other than ext4, architectures other than x86_64, USB beyond tethering.
- Test machines matter more than code volume. A [hardware report](https://github.com/Neekdo12/archstaller/issues/new?template=hardware_report.yml)
  helps as much as a patch.

## Set up

You need a Linux host. On Arch:

```sh
sudo pacman -S --needed base-devel git curl gnupg tar zstd xorriso mtools rustup lua \
    e2fsprogs dosfstools util-linux bubblewrap qemu-system-x86 edk2-ovmf \
    gtk4 gtksourceview5 pkgconf
rustup show        # installs the pinned nightly from rust-toolchain.toml
cargo xtask build  # builds target/archstaller.iso
```

[`docs/building-and-running.md`](../docs/building-and-running.md) has every command: QEMU runs, e2e installs, size
checks, and the preset build.

## Checks to run

```sh
cargo xtask keyring                # once: builds target/keyring.bin, which the pgp-lite tests read
cargo test --release -p pgp-lite --features std -p pkg -p ext4w -p disk -p initrd \
    -p hostcfg -p archstaller-gui -p loadcore -p config -p net -p aurbuild -p xtask
cargo xtask gen-luals --check          # configs/archstaller.lua matches the Rust schema
cargo xtask build && cargo xtask size  # the ISO stays within the budget in sizes.md
cargo xtask check-presets              # every preset still resolves (needs `pacman -Sy`)
```

For anything that changes how installation works, also run `cargo xtask e2e` (BIOS and `--uefi`). It downloads a
few hundred MiB and installs onto a blank QEMU disk. CI runs the first three on every pull request; the preset
check and e2e run on a schedule because Arch changes under us.

## Rules that are easy to miss

- **`OVERVIEW.md` is documentation-as-code.** Edit it in place in the same change as the code (no changelog style). CI fails a pull request that changes source without touching it; put `[skip-overview]` in a commit message for refactors, comments and tests.
- **Shared formats change on every side at once.** `crates/bootinfo` payload and BIOS patch formats are shared with
  `xtask` (`iso.rs`, `bios.rs`) and `kernel/src/install.rs`. `config/src/lib.rs` is shared between `xtask` and the
  kernel, and the Lua schema, `configs/archstaller.lua` and its test must follow.
- **Host-side config rules live in `crates/hostcfg`** (Lua loading, profiles, catalogues, validation) and are used by
  `xtask` and `gui/`. Do not duplicate validation. GTK dependencies stay in `gui/` and `hostcfg`.
- **Drivers poll.** No device interrupts; the only interrupt is the 1 kHz PIT tick. Follow
  `crates/drivers/src/e1000.rs` and `nvme.rs`.
- **`crates/*` are `no_std` + `alloc`** unless they only run on the build host (`xtask`).
- **No Linux kernel in the installer path.** `kernel/` is a `no_std` `x86_64-unknown-none` binary.
- **ISO size matters.** Run `cargo xtask size` after touching the kernel or ISO contents; keep `sizes.md` honest.
- **Be honest about testing** in the PR: "passes in QEMU (UEFI, virtio)", "ported from Linux, never run on the chip".

## Pull requests

- Branch from `master`; keep commits understandable (what and why in the subject).
- Fill in the PR template. CI must pass; the *Lint (advisory)* job may report formatting differences that predate
  your change.
- Documentation and plans that you complete: update the status line at the top of the matching file in
  `plans-implement/`, and delete the matching section of `TO-TEST.md` once it passes.

## Licence

By contributing you agree that your work is released under the [MIT licence](../LICENSE).
