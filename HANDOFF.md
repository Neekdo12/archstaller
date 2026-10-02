# HANDOFF.md

State of the work for a new session. Read `AGENTS.md` first (rules: no commits or pushes, keep
`OVERVIEW.md` in sync, polling-only drivers), then `OVERVIEW.md` for the architecture. This file
is a snapshot of what has been verified and what has not. Delete or rewrite it when it stops being useful.

## Branches

`michal`, `master` and `kubik` point at the same commit and are pushed. Work on `michal`. The user
commits and pushes; leave changes uncommitted. (From the agent's shell, `git push` over SSH fails; the user pushes.)

## GitHub issues

| # | Title | Status |
|---|---|---|
| 1 | CPU 100% when idle | Fixed: 1 kHz PIT tick (`kernel/src/idle.rs`), polling loops `hlt` through `hal::idle()`. QEMU only (about 5% host CPU). Device waits and USB transfers still spin on purpose. |
| 2 | No reboot after the test | Fixed: `reboot()` in `kernel/src/main.rs` (8042 pulse, port 0xCF9, triple fault). QEMU only. |
| 3 | iPhone tethering "Unsupported" | Fixed and confirmed on a real iPhone. |

## Verified on real hardware

- iPhone tethering (ASUS ExpertBook P5405CSA).
- Android tethering on one phone (RNDIS or ECM not recorded).
- AX88179 dongle (Axagon ADE-SG): link, DHCP, downloads. Speed test showed 2.7 MiB/s on eduroam before the TCP
  window and TLS read-size changes; not re-measured since.
- Old PC Gigabyte GA-F2A88XM-D3H (RTL8168evl xid 0x2c9, no RDRAND) on the school network: tester
  `14 ok, 1 warning (no RDRAND, jitter RNG), 0 failed`; a full install and first boot worked.

## Not verified on real hardware

- The ext4 internal-journal fix (second boot used to fail with "failed to locate journal superblock" /
  "failed to mount /sysroot"). Tested with `dumpe2fs`, `e2fsck`, `debugfs logdump` (`internal_journal_is_valid`)
  and e2e installs in QEMU only. Re-install on the old PC and boot twice to confirm.
- `kms` hook removal from the real initramfs and the AMD firmware in the default list (GPU init failure was
  suspected on a machine without a GPU; a plain reboot made it work once).
- CDC-NCM (host unit tests only), igc, other RTL8168/8169 revisions, RTL8125/8126, other dongle chips
  (ASIX AX88772, Realtek RTL8152/8153 are not implemented).
- The 256 KiB TCP window on gigabit; asynchronous USB transmit and several outstanding USB receive transfers
  are not implemented (only matter near line rate).

## Things learned the hard way (all fixed, kept for context)

- Heap must sit below 4 GiB: the bootloader's direct map is only guaranteed there (page fault on a big machine).
- No RDRAND on old CPUs: `kernel/src/rng.rs` falls back to timing jitter.
- Unhandled PIC spurious IRQ7 gave `#GP` error code 0x13b; `idt.rs` has gates for 0x20..0x2f and 0xff.
- RTL8168evl needs its own setup (`hw_start_8168e_2`, TX config, link-change patch); the rx ring was too small.
- School DHCP offered an address owned by a static host: `Stack::dhcp` ARP-probes and sends DHCPDECLINE.
- DNS from DHCP may not answer: 1.1.1.1 and 8.8.8.8 are appended.
- xHCI: a multi-TRB transfer misreports the length on a short packet; one TRB up to 64 KiB, aligned buffers.
- iPhone: device-side usbmux protocol, `ipheth` data path, a Pair reply may carry only an `EscrowBag`.

## Boot loader

Our own loader replaced Limine (`boot/`, `crates/loadcore`, `crates/bootinfo`; see `OVERVIEW.md`). The ISO is ~0.77 MiB.
`--super-small` is only an alias of `--small` now. Verified in QEMU: UEFI and BIOS (CD, hybrid disk/USB) boot the
installer. If the kernel image outgrows 4 MiB, raise `bootinfo::KERNEL_MAX`.

## Debug flag

Verbose tracing (`hal::log!`) is compiled out unless the kernel feature `debug` is on (`cargo xtask build --debug`;
the tester preset and any `dry_run = true` config get it automatically). `hal::info!` always prints. Use
`hal::log!` for traces, `hal::info!` only for what a normal user should see (install progress, failures).

## How to build and test

```sh
cargo xtask build --tethering --config presets/tester.lua --out NAME.iso   # real-hardware test ISO
cargo xtask presets                       # all six preset ISOs into target/isos/ (tethering on, tester has full debug)
cargo xtask run --usb --headless          # xHCI smoke test in QEMU
cargo xtask run --selftest --headless --nic usb-rndis   # Android RNDIS path in QEMU
cargo xtask e2e [--uefi] [--disk ahci --nic e1000]      # full install and two boots in QEMU
cargo test -p usb -p imobiledevice -p usbnet --features usb/std,imobiledevice/std
cargo test -p net                         # includes the DHCP probe/decline tests (no `std` feature)
QEMU_EXTRA='-trace usb_* -D /tmp/qtrace.log' cargo xtask run ...   # raw QEMU args
cargo xtask size [--small|--super-small]   # ISO ~0.77 MiB; tethering adds ~105 KiB
```

`presets/tester.lua` is a read-only dry run: probes hardware, brings up the network (wired, then tethering if no
wired link), downloads the package databases, pings, runs a speed test, prints PASS/FAIL, reprints driver logs, and
reboots after 60 s (300 s after a failure). It never writes a disk. ISO files in the repo root are build outputs and
git-ignored.

QEMU cannot emulate an iPhone, an AX88179 or a Realtek NIC; those need real hardware. QEMU's xHCI is more
forgiving than real controllers.

## Code map for the tethering work

| File | Role |
|---|---|
| `crates/usb/src/xhci.rs` | xHCI: init + firmware handoff, ports, rings, event stash, async IN transfers |
| `crates/usb/src/device.rs` | enumeration, control/vendor/bulk transfers, `set_interface` |
| `crates/usb/src/desc.rs` | descriptors; each alternate setting is its own `InterfaceDesc` |
| `crates/usb/src/lib.rs` | `Scan`: enumerate once, consumers `take` a device |
| `crates/imobiledevice/src/{mux,lockdown,pair,cert,netdev,lib}.rs` | usbmux, lockdownd pairing, `ipheth` NIC, `tether()` |
| `crates/usbnet/src/{lib,rndis,ncm,ax88179}.rs` | classify and bring up RNDIS/ECM/NCM/AX88179 as `hal::NetDevice` |
| `kernel/src/main.rs` | `usb_tether()`: scan, iPhone, Android/dongle, retry 6 times 3 s apart |

Every USB step logs a `usb:` line (debug builds); the last one names the failing step.

## Gotchas

- Do not run `pkill -f qemu-system-x86_64` from the same shell command: it kills the shell. Use
  `kill $(pgrep -x qemu-system-x86)`.
- A stale QEMU holding `target/test-disk.img` makes the next `xtask run` fail with "Failed to get write lock".
- The sandbox's QEMU network is slow or flaky, so speed numbers there are not reliable and mirror timeouts
  in `xtask run` are expected.
