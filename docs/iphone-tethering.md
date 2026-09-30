# Implementation spec: iPhone USB tethering as a network source

Goal: let the installer get network access from an iPhone/iPad's Personal Hotspot over a USB cable,
in addition to the existing PCI NICs in `crates/drivers`. No Wi-Fi, no firmware blobs. This is a from-scratch
addition; nothing in the tree implements USB today.

Read `crates/hal/src/lib.rs` (the `NetDevice` trait) and `crates/drivers/src/pci.rs` (the style: polling,
raw register access, no interrupts) before starting. Everything here follows the same style: poll loops,
no IRQs, `no_std` + `alloc`, errors as small enums, no dynamic firmware loading.

## Scope

- One xHCI host controller, one device attached to it (the iPhone). No hubs, no hot-plug beyond
  "the device was there at boot or gets plugged in before the network stage starts", no other USB
  device classes.
- Only the USB transport and Apple's pairing/lockdown protocol. Do not implement anything Wi-Fi related,
  MFi authentication co-processor talk, backup/AFC/springboard services, or anything beyond what is needed
  to enable the personal hotspot relay and get an IP-over-USB pipe.
- The end result plugs into `hal::NetDevice`, exactly like `crates/drivers/src/e1000.rs` does, so the rest
  of `crates/net` (DHCP, HTTP, TLS) needs no changes at all. Do not touch `crates/net`.

## New crates

Add to the workspace `Cargo.toml` members list:

- `crates/usb` — xHCI driver + USB core (device/config descriptors, control transfers, bulk transfers).
  `no_std`, depends only on `hal` (for `Clock`, if a delay is needed) and `alloc`.
- `crates/imobiledevice` — Apple's usbmuxd/lockdown/pairing protocol on top of `crates/usb`. `no_std` +
  `alloc`. Depends on `crates/usb`, `pgp-lite` (for RSA keypair generation/signing if not already exposed
  there — check first, it may only have verification) and a new minimal plist parser (see below).

Wire the new driver into `crates/drivers::probe_all` behind a feature flag `usb-tethering` (off by default
until this is trusted), similar to how `fault-test` / `*-selftest` features gate optional code today
(`kernel/Cargo.toml`).

## Step 1 — xHCI driver (`crates/usb`)

Reference: xHCI spec (Intel, publicly available, "eXtensible Host Controller Interface for Universal Serial
Bus"). Implement only what is needed for one full-speed/high-speed device with control + 2 bulk endpoints
(in/out):

1. **Discovery.** xHCI controllers are PCI class 0x0C/0x03/0x30. Reuse `drivers::pci::enumerate()` to find
   it, map its MMIO BAR (reuse `drivers::mmio::Mmio`).
2. **Controller init.** Reset (`USBCMD.HCRST`), wait for `USBSTS.CNR` to clear, program `DCBAAP`,
   `CRCR` (command ring), allocate and program one event ring + ERST, set `MaxSlotsEn` in `CONFIG`.
   All of this is register poking with polling for completion bits — no interrupts, matching the rest of
   the driver set (see how `crates/drivers/src/nvme.rs` waits on completion queues; do the same pattern
   for the xHCI event ring and command/transfer ring TRBs).
3. **Port + device detection.** Poll `PORTSC` for `CCS`/`PED`. On a connected port, issue `Enable Slot`,
   `Address Device` command TRBs, read the resulting device slot context.
4. **Descriptors.** Standard control transfers (`GET_DESCRIPTOR`) to read device/config/interface/endpoint
   descriptors. iPhones expose an Apple vendor-specific class interface (not CDC-NCM) — you do not need to
   parse it beyond finding the bulk IN/OUT endpoint addresses and max packet size; the mux protocol runs
   directly over those two bulk endpoints.
5. **Bulk transfer queueing.** Normal/Bulk transfer TRBs on the endpoint's transfer ring, polling the event
   ring for completion. Expose a small API: `usb::Device::bulk_write(ep, &[u8])`,
   `usb::Device::bulk_read(ep, &mut [u8]) -> usize`.

Keep the API surface tiny and synchronous (blocking with a timeout, like `BlockDevice::read`/`write` in
`hal`), since the rest of the codebase has no async runtime.

### Testing for this step

Add `cargo xtask run --usb` (new flag) that passes a QEMU emulated iPhone-like device is not possible
(QEMU has no Apple device emulation) — instead:
- Unit-test the xHCI TRB ring bookkeeping (producing/consuming, cycle bit handling) against a fake MMIO
  backed by a `Vec<u8>`, the same way `crates/disk/tests/disk.rs` tests GPT logic against an in-memory
  buffer instead of real hardware.
- Smoke-test against QEMU's generic USB support: attach a `usb-storage` or `usb-net` device to QEMU's xHCI
  controller (`-device qemu-xhci -device usb-storage,...`) to validate slot enable/address/descriptor
  read/bulk transfer against *some* real-ish device, before ever touching a real iPhone. This validates the
  xHCI driver independent of the Apple-specific protocol in step 2.
- Manual test: real iPhone, real machine, serial console log of enumeration succeeding.

## Step 2 — plist parser (small new dependency-free parser)

Apple's protocols exchange Apple binary or XML property lists ("plist"). Implement a minimal parser/writer
for a `no_std` subset in `crates/imobiledevice::plist` (or a separate tiny `crates/plist` if it turns out
useful elsewhere):

- Only the binary plist format (bplist00) is needed for lockdownd/usbmuxd traffic in modern iOS; the XML
  format can be skipped unless testing shows otherwise.
- Value types needed: dict, array, string, data (byte blob), integer, bool, real. No dates needed.
- Read (`Value::parse(&[u8]) -> Value`) and write (`Value::encode() -> Vec<u8>`) — pairing and lockdown
  both send and receive plists.

Test against fixtures: capture real usbmuxd/lockdownd traffic once (see step 4 test notes) and store a few
`.bplist` fixtures under `crates/imobiledevice/tests/fixtures/`, parsed and round-tripped in a host-side
`#[test]` (feature `std`, like `crates/pgp-lite/tests/verify.rs` does with its fixtures).

## Step 3 — usbmuxd framing

Reference: [libimobiledevice](https://github.com/libimobiledevice/libimobiledevice) and
[libusbmuxd](https://github.com/libimobiledevice/libusbmuxd) source (documentation only, do not copy code —
license is LGPL and this project should stay independently written; use it to understand the wire format,
then write clean-room Rust).

1. On the raw bulk pipe, frames are: a 4-byte big-endian length prefix (including the header), a
   usbmuxd header (protocol version, message type, tag), and a binary-plist body for control messages, or
   raw bytes for the "network relay" version once a connection is established.
2. Message types needed: `Listen` is not needed (we are not enumerating devices, we know we have exactly
   one). Needed: version exchange, `Connect` (to a TCP-like port on the device, used indirectly through
   lockdownd's `StartService` response port), plain data frames after a connection is established.
3. Implement as `imobiledevice::mux::Muxer` wrapping a `usb::Device`, with:
   - `send_plist(&Value)`
   - `recv_plist(timeout_ms) -> Value`
   - `connect(port: u16) -> MuxChannel` (a raw byte stream after that)

## Step 4 — pairing

This is the part with actual protocol logic, not just framing.

1. On first-ever connection to a given iPhone, the host must present a **pairing record**: an RSA keypair
   (host generates it) and a device certificate that the phone signs during a `Pair` lockdown request.
   Modern iOS requires the user to tap "Trust" on the phone the first time.
2. Flow (see libimobiledevice's `idevice_pair` / `lockdownd_pair` for the exact plist field names —
   `PublicKey`, `DeviceCertificate`, `HostCertificate`, `RootCertificate`, `SystemBUID`, `HostID`):
   - Generate an RSA-2048 host keypair (reuse `pgp-lite`'s RSA if it exposes key generation + signing;
     otherwise add a minimal generation path using the `rsa` crate, already a dependency transitively via
     `rustls-rustcrypto` — check `Cargo.lock` before adding a new RSA implementation).
   - Send a `Pair` request with the host's public key and a generated self-signed root/host certificate.
   - The phone returns `DeviceCertificate`, `EscrowBag`, etc. Store these.
   - Persist the resulting pairing record (device UDID, host keys/certs, device certs) — see "Persistence"
     below.
3. On every subsequent boot with the same phone, skip generation: replay the stored pairing record.
4. If the phone has never trusted this host (no tap on "Trust" happened, or the record is stale/rejected),
   `Pair` fails or `StartService` calls fail with a permission error. Surface this as a normal installer
   error ("connect the iPhone, unlock it, and tap Trust"), not a panic. The install flow should retry a few
   times with a delay, since the user needs time to tap the dialog.

### Persistence

The installer today has no writable persistent storage before the target disk is formatted (see
`kernel/src/install.rs` flow: disk is selected and confirmed before anything else happens). Two options,
pick based on how the disk-selection stage in `kernel/src/install.rs` is structured when this is built:

- **Simplest:** do not persist across boots at all. Every install run re-pairs from scratch and the
  installer prints a message asking the user to tap "Trust" every time. Acceptable since installs are a
  one-shot flow, not a recurring boot.
- **Nicer:** once the target disk's GPT exists (after partitioning, before packages are written), stash the
  pairing record as a small file in the ESP so a re-run of the installer (e.g. after a failed package
  download) does not need re-pairing. Only do this if step "Simplest" proves annoying in practice.

Start with "do not persist"; it is much less code and the failure mode (tap Trust again) is harmless.

## Step 5 — lockdownd and starting the hotspot relay

1. Open a mux channel to lockdownd's well-known port (port 62078, historically fixed).
2. Send `QueryType`, then `StartSession` (using the pairing record's host ID and session keys — modern
   lockdownd sessions are wrapped in TLS using the host/device certs from pairing; reuse `rustls` already in
   the tree for this, with the device cert as the trust anchor for that single connection, and the
   host cert/key as the client identity — this is the one place a client certificate is needed, unlike the
   plain server-auth TLS used for mirrors in `crates/net/src/tls.rs`).
3. Once in a session, send `StartService` for `com.apple.mobile.insecure_lockdown` or the personal-hotspot
   relay service name (verify current exact name against a real device with USB captures — this has
   changed across iOS versions; do not hardcode without checking, and comment the source of truth once
   confirmed).
4. `StartService` returns a port; open a new mux channel to that port. That channel is now raw IP traffic
   from the phone's hotspot relay — i.e. the phone does DHCP-server-like behavior for the host, or the
   channel behaves like a point-to-point link depending on iOS version. Confirm which by capturing real
   traffic (see Testing).

## Step 6 — `hal::NetDevice` adapter

Wrap the mux channel from step 5 in a type implementing `hal::NetDevice`
(`crates/imobiledevice/src/netdev.rs`):

- `transmit`: write an Ethernet-framed (or raw IP, depending on what step 5 produces — adjust `smoltcp`
  medium accordingly, see `Medium::Ethernet` vs `Medium::Ip` in `crates/net/src/stack.rs`) packet into the
  mux channel.
- `receive`: non-blocking read of the next available frame, buffered like `virtio::net` does today.
- `link_up`: true once the mux channel + relay negotiation from step 5 succeeded.

Register it in `drivers::probe_all` (or a new `drivers::imobiledevice::probe` called from `kernel/src/main.rs`
directly, since it is not a PCI device and doesn't fit the `pci::enumerate()` loop — follow how
`kernel/src/main.rs` already calls `drivers::probe_all()` and add a second, explicit call site for USB
devices).

## Testing strategy (overall)

1. Unit tests (host, `std` feature) for: plist parsing/round-trip, xHCI ring bookkeeping against fake MMIO,
   usbmux frame parsing against captured fixtures.
2. Capture real traffic once, using `usbmux`/`pymobiledevice3` on a Linux or Mac host with a real iPhone
   plugged in and `tcpdump`/`usbmon` (`cat /sys/kernel/debug/usb/usbmon/0u`), to get ground-truth plist
   contents and confirm current service names and port numbers. Store the anonymized captures as test
   fixtures (strip any real device serials/UDIDs first).
2. `cargo xtask e2e --usb` (new flag): full install using this path instead of a PCI NIC, run manually with
   a real iPhone attached to the test machine — cannot be automated in QEMU since QEMU cannot emulate an
   iPhone. Document this limitation in the test's `--help` text and in `README.md`'s testing section once
   merged.
3. Add a size check: `cargo xtask size` should show the new crates' contribution; expect on the order of
   100-150 KiB added to the kernel (see `sizes.md` for the estimate this spec is based on). If the real
   number is much higher, something pulled in more than needed (e.g. a full XML plist writer, or an RSA
   implementation duplicate of one already in the dependency tree) — check `cargo bloat`/`llvm-nm` per
   `sizes.md`'s method before merging.

## Non-goals (explicitly do not implement)

- Wi-Fi (see `wifi.md` for that, separate effort).
- Any other USB device class (mass storage, HID, other phones/Android — Android tethering uses a
  completely different, standard USB class and does not need any of this crate).
- Multiple simultaneously attached USB devices, hubs, hot-unplug handling beyond "link_up() becomes false
  and the installer reports a network error like it would for a cable pull".
- Device-initiated services beyond the hotspot relay (no backup, no app install, no springboard, no
  crash log pulling, no diagnostics).

## Suggested implementation order (for an agent tackling this in one pass)

1. `crates/usb` — xHCI + control/bulk transfers, tested against QEMU's generic `usb-storage` device.
2. `crates/imobiledevice::plist` — parser/writer, tested against captured fixtures.
3. `crates/imobiledevice::mux` — framing over `crates/usb`, tested against captured fixtures.
4. `crates/imobiledevice::pair` — pairing flow, requires a real device to test end-to-end (a tap on
   "Trust" cannot be simulated).
5. `crates/imobiledevice::lockdown` — session + `StartService`, real device required.
6. `crates/imobiledevice::netdev` — `hal::NetDevice` adapter, wire into `kernel/src/main.rs`.
7. Update `README.md` (networking section) and `PLAN.md` to describe this as an additional, optional
   network source, not a replacement for the existing wired NICs.
