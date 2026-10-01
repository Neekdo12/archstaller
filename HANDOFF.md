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

Old CPU without RDRAND (reported by the user on another machine: `[FAILED] RDRAND available`): the installer used to
need RDRAND for TLS and key generation. `kernel/src/rng.rs` now falls back to timing jitter (TSC deltas around PIT
counter reads, SHA-256 pool and counter-mode output) and the tester shows a warning instead of a failure. Verified in
QEMU with `-cpu host,-rdrand` (`QEMU_EXTRA`): HTTPS downloads and the speed test work. Weaker than a hardware RNG.
That machine is a Gigabyte GA-F2A88XM-D3H (integrated Realtek RTL8111F = RTL8168 family, PCI 10ec:8168), so the
`r8169` driver ran on real hardware for the first time and got `DHCP: Timeout`. Changes made without being able to
test (QEMU has no Realtek model): ring addresses are now written as two 32-bit writes, high half first, as Linux does
(the chips mishandle 64-bit register accesses); the PHY is power-up/auto-negotiation restarted through PHYAR; the
driver logs the chip xid, PHY status and the first tx/rx descriptors (`r8169: ...`). Diagnostics: the tester reprints all `hal::log!` lines (`kernel/src/digest.rs`) after `RESULT` and waits 300 s after a
failure, because the `r8169: pci ..., chip xid` line had scrolled off the screen. The r8169 driver also logs PHY
registers (bmcr/bmsr/anar/anlpar/gbcr/gbsr) after the link wait and, while nothing has been received, the chip's
latched interrupt status, rx-missed counter and the ring address read back (every 2 s, 5 times).
Network stage logging: `net::client` logs each stage of every download (`net: host is IP (ms)`, `TCP connected in N ms, TLS
handshake M ms`, `HTTP status, bytes, ms`, or the stage and error that failed), reprinted in the tester's driver log
block. Added because core.db/extra.db timed out on the old machine (GA-F2A88XM-D3H) with DHCP already working and
the stage that stalls was unknown.
Result of the stage logging: DNS lookup timed out for both databases on the old machine (DHCP fine). `net::Stack`
now counts frames by kind and direction and logs `rx: N arp, ... (M unicast); tx: ...` plus the DHCP lease line
(`network: addr/prefix gateway dns`) when a lookup goes unanswered. Hypothesis: unicast frames (ARP replies from the
gateway) are not received while broadcasts (DHCP) are, so the DNS server's MAC is never resolved.
Counters from the old machine: lease 10.21.14.2/24, gateway 10.21.14.1, DNS 10.10.10.2 (another subnet); the gateway was
resolved by ARP (2 ARP requests, replies received), 4 more DNS queries went out and no unicast frame came back, while
broadcast traffic kept arriving. So the card receives unicast and the DHCP-provided resolver does not answer.
Now 1.1.1.1 and 8.8.8.8 are appended to the DNS server list (`FALLBACK_DNS` in `crates/net/src/stack.rs`); the list is
logged (`net: DNS servers ...`). If those are unreachable too, outbound traffic beyond the gateway is filtered.
Result with the fallbacks (DNS servers 10.10.10.2, 10.10.10.3, 1.1.1.1, 8.8.8.8): still unanswered; counters before/after
(rx arp/udp/tcp/other/unicast, tx arp/udp/tcp/other) 2 7 0 9 7 | 2 6 0 0 then 2 7 0 15 7 | 2 10 0 0: four more UDP queries
sent, no unicast frame received. The card receives unicast (the gateway's 2 ARP replies came in), so this looks like the
network not forwarding or answering anything from this machine (a MAC registration / login network?). The tester now
also does a direct TCP connect to 1.1.1.1:443 (`internet reachability (no DNS)`) to say so explicitly.
School network, cause found: the DHCP server offered 10.21.14.2, which belongs to a static device (the user's 3D-printer
notebook, MAC 5c:f9:dd:55:07:0c) outside DHCP's knowledge, so replies to our PC went to that device. Linux clients ARP-probe
the offered address and DHCPDECLINE it; `Stack::dhcp` now does the same (`probe_address`, `dhcp_decline`, up to 4 declines,
3 s pause before asking again; unit-tested with a mock NIC in `crates/net/src/stack.rs`). Verified on the real school network
on the old machine (GA-F2A88XM-D3H, RTL8111 evl): tester `14 ok, 1 warning (no RDRAND, jitter RNG), 0 failed`.
Earlier: school network (wired port forwards nothing from this machine, but the user says Linux works on that port): the stack
now sends a gratuitous ARP after DHCP like Linux's clients, logs the DHCP server, logs every ARP frame in both directions
(up to 12 each: who the gateway MAC is) and a histogram of the other ethertypes received (0x888e would be 802.1X EAPOL,
0x88cc LLDP, 0x86dd IPv6), and the tester pings the gateway, the DHCP DNS server and 1.1.1.1. Compare the logged gateway
MAC with `ip neigh` on Linux on the same port. Not yet seen on the school network.
Fifth report: `*** EXCEPTION #13 general protection fault, error code 0x13b` = IDT error code for vector 0x27, the
legacy PIC's spurious IRQ7 (masked lines can still raise it), because the IDT only had the timer vector 0x20 above the
CPU exceptions. `idt.rs` now has gates for every PIC vector 0x21..0x2f (spurious IRQ7/15 are only acknowledged if the
in-service register says they are real) and for the APIC spurious vector 0xff; the IDT is 256 entries.
Fourth report: with the evl setup DHCP works, but the core/extra database downloads time out. Hypothesis (not
confirmed on hardware): the 32-entry receive ring overflowed during blocking TLS decryption on the old CPU while the
1 MiB TCP window let the server burst more than the ring held. Now the r8169 ring is 256 descriptors (512 KiB) and
`TCP_RX` is 256 KiB so a full window fits; the driver logs `rxmissed` (up to 8 times, 2 s apart) if the chip still
drops frames. The other drivers' rings (e1000, igb, virtio, rtl8139) are unchanged and may need the same.
Third report: the board's chip is xid 0x2c9 = RTL8168evl/8111evl (Linux MAC version 34), not the 8168F, so the 8168F
path never ran, and rxmissed was high (frames reached the MAC, no usable RX descriptors). Added from Linux for this
revision: `hw_start_8168e_2` (ephy table, ERI 0xd0 = 0x07ff0060, FIFO sizes), TxConfig with TXCFG_AUTO_FIFO
(0x03000780), RxMaxSize 0x3fff, and `link_chg_patch_8168evl` applied on every link-up edge (`poll_link`).
Second report: 3 DHCP discovers transmitted ("tx frame 304 bytes") but nothing received. Added, following Linux's
RTL8168F path (xid 0x480 only): `hw_start_8168f` (ERI FIFO sizes and packet-filter reset, EPHY tuning, MCU/DLLPR/MISC
/Config5 bits, PCIe clock-request off), RxConfig 0xc70e as Linux's `rtl_init_rxcfg` + `rtl_set_rx_mode` (the old value
had one extra bit), and the multicast filter MAR0/4 opened. Other revisions get only the generic path. The same machine's earlier report was
`DHCP: Timeout`; its NIC (PCI id from the tester's device list, `net0:` line) is not
known yet. Drivers never run on hardware: igc, RTL8168/8169, RTL8125/8126, rtl8139 (see README).

Boot crash on real hardware (`*** EXCEPTION #14 page fault`, error 0x2, cr2 0xFFFF8001000005F8, right after the
first banner): the heap was the largest usable memory region wherever it was, and on a machine with a big region
above 4 GiB the write of the allocator's metadata faulted (the bootloader's direct map only covers the first 4 GiB
for certain). `heap::init` now prefers the largest usable region below 4 GiB and prints `heap region ...` before
touching it. Verified in QEMU with 12 GiB (the largest region is above 4 GiB there). The exact cause on the
failing machine was inferred from the address, not confirmed: if it recurs, the printed `heap region` line and the
`rip` resolved with an unstripped build (`CARGO_PROFILE_RELEASE_STRIP=none`, `nm`/`addr2line`) show where.

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
