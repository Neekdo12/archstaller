# HANDOFF.md

State of the work for a new session. Read `AGENTS.md` first (rules: no commits or pushes, keep
`OVERVIEW.md` in sync, polling-only drivers), then `OVERVIEW.md` for the architecture. This file
covers what is in flight, what has been verified, and how to continue. It is a snapshot, not
documentation of the design; delete or rewrite it when it stops being useful.

## Branches

- `master`: base. Contains the iPhone tethering code as first written plus the merged
  `the_supperior_supperior_branch` (igb/rtl8139 drivers, dry-run hardware test mode, `sizes.md`).
- `michal`: fixes for GitHub issue #1 (CPU at 100% when idle) and #2 (no reboot after the test).
- `kubik`: all the USB tethering work (GitHub issue #3 and the Android support). Current branch. The
  user opens the PR from here.

`kubik` now has the `reboot()` fix from `michal` (copied, same code) but not the idle tick (`hal::idle()`).

## Issues

| # | Title | Status |
|---|---|---|
| 1 | CPU 100% | Fixed on `michal`: 1 kHz PIT tick (`kernel/src/idle.rs`) so idle polling loops can `hlt` through `hal::idle()`. Verified in QEMU only (about 5% host CPU during the countdown). Device waits in drivers and USB transfers still spin on purpose. |
| 2 | No reboot | Fixed on `michal`: `reboot()` in `kernel/src/main.rs` tries the 8042 pulse, then port 0xCF9, then a triple fault. Verified in QEMU only. |
| 3 | iPhone tethering "Unsupported" on an ASUS ExpertBook P5405CSA | Fixed on `kubik`: the iPhone now tethers on that machine (user confirmed). |

## USB tethering: where it stands

**iPhone: works on real hardware** (user confirmed on an ASUS ExpertBook P5405CSA, Intel Core Ultra 200V,
xHCI 8086:a831 / 8086:a87d). It took these fixes, all found from the user's boot logs: xHCI Configure
Endpoint contexts, SuperSpeed EP0 packet size, firmware handoff, waiting for ports to debounce after the
controller reset, the real device-side usbmux protocol (the first version used the host-daemon plist
protocol, which a phone never answers), tethering through the phone's `ipheth` interface instead of a
lockdown service, short-read panics, and accepting an `EscrowBag`-only Pair reply (newer iOS sends no
`Result` key).

**Android: works on one real phone** (user confirmed; which function it used, RNDIS or ECM, was not
recorded) and in QEMU. QEMU's `usb-net` emulates RNDIS (configuration 2) and ECM (configuration 1) and the
full selftest (DHCP, HTTPS download, 1 MB HTTP) passes over each.

**USB Ethernet dongles: written, never run on hardware.** `crates/usbnet` also has CDC-NCM (`ncm.rs`) and the
ASIX AX88179 vendor protocol (`ax88179.rs`), added because the user's Axagon ADE-SG (AX88179, `0b95:1790`)
did not work with the ECM-only build. Both have host unit tests of the framing (`crates/usbnet/tests/ncm_ax.rs`),
but QEMU cannot emulate them. The AX88179 register sequence and packet formats come from the Linux
`ax88179_178a` driver (fetched as documentation, summarised, then written clean-room), so a wrong register
value or off-by-N frame length is plausible. If the dongle fails, the `usb:` log shows the step
(`AX88179 PHY reset failed`, `waiting for the Ethernet link`, `link up, PHY status ...`).
Throughput: the dongle path reported 2.7 MiB/s in the tester's speed test. The cause was not the dongle but the
stack: TLS read the socket in 4 KiB chunks (about 10x slower than 16 KiB chunks in QEMU: 2.3 vs 25 MiB/s against
the same mirror) and the TCP receive buffer was 64 KiB (caps a 24 ms path at 64 KiB/24 ms = 2.7 MiB/s). Now
`TCP_RX` is 1 MiB (`crates/net/src/stack.rs`) and `fill` reads 16 KiB (`crates/net/src/tls.rs`). Not yet measured on
the real dongle. The speed report also prints how long TLS record processing took versus waiting for bytes.

First real-hardware result for the AX88179 (Axagon ADE-SG): bring-up and link worked, DHCP timed out. A likely
cause was fixed: a transfer split into several TRBs (anything over 16 KiB, such as the AX88179's 20-26 KiB receive
buffer) has its length mis-measured on a short packet, because the xHCI event reports only the residual of the TRB
that ended early. `MAX_TRB_LEN` is now 64 KiB and `Dma::for_transfer` aligns transfer buffers so one TRB never
crosses a 64 KiB boundary. The first 4 transmitted frames and 6 received transfers are logged
(`usb: tx frame ...`, `usb: rx transfer N bytes -> M frames`) to show which direction fails if DHCP still does.
Not implemented: other vendor-specific chips (ASIX AX88772, Realtek RTL8152/8153), RNDIS keepalives,
interrupt-endpoint notifications.

**Speed:** the user measured 2.7 MiB/s with the AX88179 dongle on eduroam. That equals the old 64 KiB TCP receive
window divided by a ~24 ms round trip, and applies to every NIC, so the window is now 1 MiB (window scale 5 is
negotiated; verified in a QEMU packet capture). The sandbox's own uplink (~20 Mbit/s) hides any gain in QEMU, so the
real effect is unmeasured. `hwtest`'s speed test also prints how much of the time was TLS decryption versus waiting
for bytes (`net::tls::CRYPTO_TSC` / `IO_TSC`; decryption was ~6% in QEMU). Not done: asynchronous USB transmit (each ACK
is a blocking bulk write) and more than one outstanding USB receive transfer; both only matter near line rate.

The kernel tries tethering only when no wired NIC has link (`usb_tether` in `kernel/src/main.rs`). With
devices attached but none tethering (Android in file-transfer mode) it re-scans 6 times, 3 s apart, so the
user can switch USB tethering on after plugging in.

### Next steps

1. Run the dongle: `cargo xtask build --tethering --config presets/tester.lua --out dongle.iso`, plug the dongle
   (cable connected to a router) with no other wired link, read the `usb:` lines. Check the AX88179 path first
   (`AX88179 link up ...` then `USB Ethernet tethering up`), then DHCP and the speed test the tester prints.
2. Try more phones; an unrecognised one shows its interface triples in the log.
3. Realtek RTL8152/8153 and ASIX AX88772 vendor drivers are the next likely dongle chips; each needs its own
   register-level driver, so do them only against a real device.
4. The tester now runs a speed test after a fully passing run and reboots with `reboot()`. The reboot and
   idle fixes were ported to this branch from `michal` (same code, so the later merge should be clean).
5. Delete this file when it stops being useful.

## How to build and test

```sh
cargo xtask build --tethering --config presets/tester.lua --out NAME.iso   # real-hardware test ISO
cargo xtask run --usb --headless          # xHCI smoke test in QEMU: qemu-xhci + usb-storage, SCSI INQUIRY
cargo xtask run --selftest --headless --nic usb-rndis   # Android RNDIS path in QEMU: DHCP + HTTPS + 1 MB HTTP
cargo xtask run --usb --tethering --nic none --headless # the retry loop (device attached, none tethers)
cargo test -p usb -p imobiledevice -p usbnet --features usb/std,imobiledevice/std
QEMU_EXTRA='-trace usb_* -D /tmp/qtrace.log' cargo xtask run ...   # raw QEMU args, for USB tracing
cargo xtask size --small --tethering      # tethering costs about 105 KiB; ISO limit is 3 MiB
```

`presets/tester.lua` is a read-only dry run: it probes hardware, brings up the network, downloads the
package databases, prints PASS/FAIL, and reboots after 60 seconds. It never writes a disk. The ISO files
in the repo root (`iphone_hwtest*.iso`, `test_iso.iso`, `tethering_test.iso`) are build outputs, not
tracked.

QEMU cannot emulate an iPhone, so the Apple-specific parts (`crates/imobiledevice`) can only be
checked on a real phone. The xHCI driver and Android path are checked in QEMU, which is more forgiving
than real controllers (it hid several of the iPhone bugs above).

## Logging

Driver crates without a console log through `hal::log!` (hook installed in `kernel/src/main.rs`).
Everything USB-related prints a `usb:` line: controllers, per-port `portsc`, enumeration steps, each
interface triple, the mux/lockdown/pairing steps, and the Pair reply. When a real-hardware attempt
fails, the last `usb:` line names the failing step; ask the user for all of them.

## Code map for the tethering work

| File | Role |
|---|---|
| `crates/usb/src/xhci.rs` | xHCI: init + firmware handoff, ports, command/event/transfer rings, event stash, async IN transfers |
| `crates/usb/src/device.rs` | enumeration (all configurations, retries), control/vendor/bulk transfers, `set_interface` |
| `crates/usb/src/desc.rs` | descriptors; every alternate setting is its own `InterfaceDesc` |
| `crates/usb/src/lib.rs` | `Scan`: enumerate once, then consumers `take` a device (the iPhone and Android paths each pick theirs) |
| `crates/imobiledevice/src/mux.rs` | device-side usbmux: framing, handshake, TCP-like channel |
| `crates/imobiledevice/src/lockdown.rs`, `pair.rs`, `cert.rs` | lockdownd QueryType/GetValue/Pair, pair record and certificate generation |
| `crates/imobiledevice/src/netdev.rs` | `ipheth` interface as `hal::NetDevice` |
| `crates/usbnet/src/lib.rs` | classify RNDIS/ECM/NCM/AX88179 functions, bring-up, `hal::NetDevice` |
| `crates/usbnet/src/{rndis,ncm,ax88179}.rs` | pure framing/register definitions for each protocol (host-tested) |
| `crates/imobiledevice/src/lib.rs` | `tether()`: the whole flow with progress notes |
| `kernel/src/main.rs` | `usb_tether()` under `#[cfg(feature = "usb-tethering")]`: scan, iPhone, Android, retry |

## Gotchas

- Do not run `pkill -f qemu-system-x86_64` from the same shell command line: it matches its own
  command and kills the shell. Use `kill $(pgrep -x qemu-system-x86)`.
- A stale QEMU holding `target/test-disk.img` makes the next `xtask run` fail with "Failed to get
  write lock".
- `xtask run` in this sandbox cannot reach the Arch mirror, so the dry-run's `core.db`/`extra.db`
  downloads time out there; that is expected and unrelated to the USB work.
- The user commits and pushes themselves (AGENTS.md); leave changes uncommitted.
