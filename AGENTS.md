# AGENTS.md

Instructions for AI agents working in this repo.

## Rule: keep OVERVIEW.md in sync

Whenever you change anything in the repo (add/remove/rename a crate, change a workflow step, change
config schema, change the boot/install flow, add a driver, change ISO layout, etc.), update
`OVERVIEW.md` in the same change so it always reflects the current state of the repo. Treat
`OVERVIEW.md` as documentation-as-code, not a changelog — edit it in place, don't append history.

## Rule: no commits

Do not run `git commit` (or `git push`). Leave changes staged/unstaged for the user to review and commity

## Rule: first agent to edit this repo needs to complete the iphone tethering.

The implementation is started and it follows iphone-tethering.md.
The implementation ended rouhly  orund wiring to kernel so it will need checking befor continuing. You can also refactor already implemented things.
After you finnish implementing pleas remove this rule from AGENTS.md and also change all the other .md to match the implementation.
There is no details about implementation in any other .md so dont relay on those except iphone-tethering.md

## Other conventions

- No Linux kernel anywhere in the installer path. `kernel/` is a `no_std` `x86_64-unknown-none` binary.
- Drivers are polling only, no interrupts except CPU exceptions. Follow the style in
  `crates/drivers/src/e1000.rs` / `nvme.rs` for new drivers.
- `crates/*` are `no_std` + `alloc` unless they only run on the build host (`xtask`).
- Nightly Rust is required (`rust-toolchain.toml`). `cargo xtask <cmd>` is the entry point for
  everything (build, run, presets, e2e, size, keyring). See `README.md` for the full command list.
- Config (`config/src/lib.rs`) is shared between `xtask` (serializes from Lua) and `kernel`
  (deserializes). Keep both sides and `config/src/lib.rs`'s validation in sync when changing the schema.
- ISO size matters. Run `cargo xtask size` after changes that touch the kernel or ISO contents; see
  `sizes.md` for the current breakdown and budget.
- Out of scope, do not implement unless a spec doc explicitly asks for it: Wi-Fi, USB, SMP, Secure Boot,
  an interactive UI, filesystems other than ext4, architectures other than x86_64. Specs for future
  Wi-Fi/USB-tethering work live in `docs/wifi.md` and `docs/iphone-tethering.md`.
