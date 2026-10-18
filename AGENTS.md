# AGENTS.md

Instructions for AI agents working in this repo.

## Rule: keep OVERVIEW.md in sync

Whenever you change anything in the repo (add/remove/rename a crate, change a workflow step, change
config schema, change the boot/install flow, add a driver, change ISO layout, etc.), update
`OVERVIEW.md` in the same change so it always reflects the current state of the repo. Treat
`OVERVIEW.md` as documentation-as-code, not a changelog — edit it in place, don't append history.

## Rule: test-driven development (TDD)

Follow Test-Driven Development (TDD) strictly:
- Always write tests first, including comprehensive edge cases, boundary conditions, and error states, before implementing or modifying the target function or logic.
- Verify tests fail (Red) before writing the implementation to make them pass (Green).
- Every public and internal function must have corresponding unit tests.

## Rule: clippy-compliant code

All code in the workspace must be strictly Clippy-compliant:
- `cargo clippy --workspace --all-targets` must pass with zero warnings.
- Do not silence lints with blanket `#[allow]` attributes. If a lint suppression is necessary and justified, use an explicit `#[expect(lint_name, reason = "...")]` with a clear explanation, adhering to the workspace lint policy in `Cargo.toml`.

## Other conventions

- No Linux kernel anywhere in the installer path. `kernel/` is a `no_std` `x86_64-unknown-none` binary.
- The boot loader is ours (`boot/`, `crates/loadcore`, `crates/bootinfo`); `bootinfo` is the contract with the kernel, and `crates/bootinfo`'s
  payload and BIOS patch formats are shared with `xtask` (`iso.rs`, `bios.rs`) and the installer (`kernel/src/install.rs`): change all sides together.
- Host-side config rules live in `crates/hostcfg` (TOML loading, profiles, driver/script catalogues, validation) and are used by `xtask` and `gui/`; do not duplicate validation in either. GUI dependencies (gtk4, sourceview5, ...) stay in `gui/` and `hostcfg`, never in `kernel/`, `config/` or the boot path.
- Drivers are polling only, no device interrupts. The only other interrupt is the 1 kHz PIT tick (`kernel/src/idle.rs`) that lets
  idle polling loops `hlt` via `hal::idle()`; interrupts are off everywhere else. Follow the style in
  `crates/drivers/src/e1000.rs` / `nvme.rs` for new drivers.
- `crates/*` are `no_std` + `alloc` unless they only run on the build host (`xtask`).
- Nightly Rust is required (`rust-toolchain.toml`). `cargo xtask <cmd>` is the entry point for
  everything (build, run, presets, e2e, size, keyring). See `README.md` and `docs/building-and-running.md` for the full command list.
- Config (`config/src/lib.rs`) is shared between `xtask` (serializes from TOML) and `kernel`
  (deserializes). Keep both sides and `config/src/lib.rs`'s validation in sync when changing the schema.
- ISO size matters. Run `cargo xtask size` after changes that touch the kernel or ISO contents; see
  `sizes.md` for the current breakdown and budget.
- Out of scope, do not implement unless a spec doc explicitly asks for it: Wi-Fi, SMP, Secure Boot,
  an interactive UI, filesystems other than the supported big four (`ext4`, `btrfs`, `xfs`, `f2fs`), architectures other than x86_64, and USB beyond the
  tethering paths (`crates/usb`, `crates/imobiledevice` for iPhone, `crates/usbnet` for Android). The spec
  for future Wi-Fi work lives in `plans-implement/wifi.md`, and Raspberry Pi (ARM/AArch64) work lives in
  `plans-implement/raspberry-pi.md`.
