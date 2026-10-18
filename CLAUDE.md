# CLAUDE.md

Guidelines and instructions for Claude Code and AI agents working on archstaller.

## Project Overview

`archstaller` is an unattended Arch Linux installer that boots without any Linux kernel.
- **Custom boot loader** (`boot/`, BIOS + UEFI) enters the kernel with `bootinfo::BootInfo`.
- **Bare-metal kernel** (`kernel/`): `no_std` `x86_64-unknown-none` binary running on a single core with polling-only device drivers. It partitions the disk, downloads and verifies packages via HTTPS/PGP, formats the root filesystem, builds an initramfs, and sets up first boot.
- **Root filesystems supported**: Ext4 (`crates/ext4w`), Btrfs (`crates/btrfsw`), XFS V5 (`crates/xfsw`), and F2FS (`crates/f2fsw`). Only the selected filesystem engine is compiled into the ISO kernel per preset/build.
- **Host tooling** (`xtask/`): `cargo xtask <cmd>` handles all building, running, QEMU testing, and ISO assembly.
- **Desktop GUI** (`gui/`): GTK4 application for configuration editing, preset loading, and direct media flashing.

## Core Rules for AI Agents

### 1. Rule: Keep `OVERVIEW.md` in Sync
Whenever you change anything in the repo (add/remove/rename a crate, change a workflow step, change config schema, change the boot/install flow, add a driver, change ISO layout, etc.), update `OVERVIEW.md` in the same change so it always reflects the current state of the repo. Treat `OVERVIEW.md` as documentation-as-code, not a changelog — edit it in place, don't append history.

### 2. Rule: No Commits
Do not run `git commit` (or `git push`). Leave changes staged or unstaged in the working tree for the user to review and commit.

### 3. Rule: Test-Driven Development (TDD)
Follow Test-Driven Development (TDD) strictly:
- Always write tests first (covering standard behavior and comprehensive edge cases, boundary conditions, and error paths) before writing or altering the implementation.
- Run tests to confirm failure (Red) before writing the code to satisfy them (Green).
- Every public and internal function must have corresponding unit tests.

### 4. Rule: Clippy-Compliant Code
All code across the entire workspace must be strictly Clippy-compliant:
- `cargo clippy --workspace --all-targets` must pass with zero warnings.
- Do not silence lints using blanket `#[allow]` attributes. If a lint suppression is required and genuinely justified, use `#[expect(lint_name, reason = "...")]` with a clear explanation, in accordance with the workspace lint policy in `Cargo.toml`.

## Common Commands & Workflows

Nightly Rust is required (`rust-toolchain.toml`). `cargo xtask` is the primary entry point:

```sh
# Building
cargo xtask build                       # Build default target/archstaller.iso
cargo xtask keyring                     # Build target/keyring.bin from pinned archlinux-keyring

# Testing & Linting
cargo test --workspace                  # Run all workspace unit and integration tests
cargo clippy --workspace --all-targets  # Clippy lint check across all crates and targets
cargo xtask gen-schema --check          # Check JSON schema matches Rust definitions
cargo xtask check-presets               # Validate all presets resolve with official sync DBs
cargo xtask linux-test                  # Automated QEMU test of filesystem formats against Linux kernel

# Size & Budget
cargo xtask size                        # Verify kernel and ISO sizes stay within budget (see sizes.md)

# Running & Emulation
cargo xtask run                         # Boot archstaller ISO in QEMU
cargo xtask run --uefi                  # Boot under OVMF UEFI
cargo xtask e2e                         # End-to-end installation test in QEMU
```

## Architectural Conventions

- **Kernel is `no_std`**: `kernel/` is a `no_std` `x86_64-unknown-none` binary. No Linux kernel exists in the installer path.
- **Library crates are `no_std` + `alloc`**: All crates under `crates/*` remain `no_std` + `alloc` unless they are explicitly build-host tooling (`xtask`, `crates/aurbuild`, `crates/hostcfg`).
- **Polling drivers only**: No hardware device interrupts; the only interrupt is the 1 kHz PIT timer tick (`kernel/src/idle.rs`) that allows idle loops to `hlt` via `hal::idle()`. Follow `crates/drivers/src/e1000.rs` and `nvme.rs`.
- **Shared contracts**:
  - `crates/bootinfo`: Payload bundle layout and BIOS stage patch formats shared between bootloader, `xtask`, and kernel.
  - `config/src/lib.rs`: Serialized with `postcard` into `config.bin`; shared between `xtask` and `kernel`.
  - Host-side validation lives in `crates/hostcfg`; do not duplicate validation in `xtask` or `gui/`.
- **GUI isolation**: GTK4 / GtkSourceView dependencies remain strictly in `gui/` and `crates/hostcfg`; never in `kernel/`, `config/`, or boot path crates.
- **ISO Size Budget**: ISO size is strictly constrained (< 3 MiB budget; typical build < 800 KiB). Always run `cargo xtask size` when modifying kernel or ISO bundle components.

## Out of Scope

Do not implement unless an explicit specification in `plans-implement/` directs it:
- Wi-Fi (see `plans-implement/wifi.md`)
- SMP (Symmetric Multiprocessing)
- Secure Boot
- Interactive installer-kernel UI
- Filesystems beyond the supported big four (`ext4`, `btrfs`, `xfs`, `f2fs`)
- CPU architectures other than x86_64 (Raspberry Pi ARM/AArch64 progress tracked in `plans-implement/raspberry-pi.md`)
- USB devices other than supported tethering paths (`crates/usb`, `crates/imobiledevice`, `crates/usbnet`)
