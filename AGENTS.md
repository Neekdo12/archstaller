# AGENTS.md

Instructions for AI agents working in this repo.

## Rule: keep OVERVIEW.md in sync

Whenever you change anything in the repo (add/remove/rename a crate, change a workflow step, change
config schema, change the boot/install flow, add a driver, change ISO layout, etc.), update
`OVERVIEW.md` in the same change so it always reflects the current state of the repo. Treat
`OVERVIEW.md` as documentation-as-code, not a changelog — edit it in place, don't append history.

## Rule: no commits

Do not run `git commit` (or `git push`). Leave changes staged/unstaged for the user to review and commity

## Other conventions

- No Linux kernel anywhere in the installer path. `kernel/` is a `no_std` `x86_64-unknown-none` binary.
- Drivers are polling only, no device interrupts. The only other interrupt is the 1 kHz PIT tick (`kernel/src/idle.rs`) that lets
  idle polling loops `hlt` via `hal::idle()`; interrupts are off everywhere else. Follow the style in
  `crates/drivers/src/e1000.rs` / `nvme.rs` for new drivers.
- `crates/*` are `no_std` + `alloc` unless they only run on the build host (`xtask`).
- Nightly Rust is required (`rust-toolchain.toml`). `cargo xtask <cmd>` is the entry point for
  everything (build, run, presets, e2e, size, keyring). See `README.md` for the full command list.
- Config (`config/src/lib.rs`) is shared between `xtask` (serializes from Lua) and `kernel`
  (deserializes). Keep both sides and `config/src/lib.rs`'s validation in sync when changing the schema.
- ISO size matters. Run `cargo xtask size` after changes that touch the kernel or ISO contents; see
  `sizes.md` for the current breakdown and budget.
- Out of scope, do not implement unless a spec doc explicitly asks for it: Wi-Fi, SMP, Secure Boot,
  an interactive UI, filesystems other than ext4, architectures other than x86_64, and USB beyond the
  iPhone tethering path (`crates/usb`, `crates/imobiledevice`). The spec for future Wi-Fi work lives in
  `docs/wifi.md`; `docs/iphone-tethering.md` is the spec the tethering code was written from.
