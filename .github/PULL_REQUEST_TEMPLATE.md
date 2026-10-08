## What and why

<!-- One or two sentences. Link the issue: "Closes #123". -->

## Kind of change

- [ ] Bug fix
- [ ] New feature (driver, preset, config option, GUI)
- [ ] Refactor or cleanup, no behaviour change
- [ ] Documentation or CI only

## Checklist

- [ ] `cargo test --release -p pgp-lite --features std -p pkg -p ext4w -p disk -p initrd -p hostcfg -p archstaller-gui -p loadcore` passes
- [ ] `cargo xtask build` works, and `cargo xtask size` stays within the budget (see `sizes.md`) if the kernel or the ISO contents changed
- [ ] `OVERVIEW.md` is updated in this change if a crate, workflow step, config field, boot or install step, driver, or ISO layout changed (edit it in place; no history)
- [ ] Shared formats changed on **all** sides: `crates/bootinfo` payload and BIOS patch formats with `xtask` (`iso.rs`, `bios.rs`) and `kernel/src/install.rs`; `config/src/lib.rs` with the Lua side and `cargo xtask gen-luals`
- [ ] Config validation lives in `crates/hostcfg` and `config`, not duplicated in `xtask` or `gui/`
- [ ] New drivers are polling only (no device interrupts) and follow `crates/drivers/src/e1000.rs` / `nvme.rs`
- [ ] No Linux kernel in the installer path; `crates/*` stay `no_std` + `alloc` unless they only run on the build host
- [ ] GTK dependencies stay in `gui/` and `hostcfg`

## How it was tested

<!-- QEMU (BIOS/UEFI, disk and NIC models), `cargo xtask e2e`, real hardware (which), or "not run". Be specific; untested driver code is welcome if it says so. -->

## Notes for the reviewer

<!-- Risks, follow-ups, things deliberately left out. -->
