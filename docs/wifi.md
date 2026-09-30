# Implementation spec: Wi-Fi as a network source

Goal: let the installer associate with a Wi-Fi access point (WPA2-PSK at minimum) and use it exactly like
a wired NIC. This is a large effort — expect it to dwarf everything else in the installer kernel, mostly
because of vendor firmware blobs (see `sizes.md`'s comparison of Wi-Fi vs. USB tethering ISO size impact).
Read that file's Wi-Fi section before starting, to keep size expectations calibrated.

Read `crates/hal/src/lib.rs` (`NetDevice` trait), `crates/drivers/src/e1000.rs` (style reference: polling
PCI NIC driver) and `crates/net/src/stack.rs` (how a `NetDevice` plugs into `smoltcp`) first. The end goal
is another type implementing `hal::NetDevice`; nothing in `crates/net` should need to change.

## Scope decision to make before writing code

Pick **one** chipset family for the first implementation; do not try to cover Intel + Realtek + Broadcom at
once. Recommendation: **Intel (iwlwifi-compatible)**, because:
- Firmware file format and command protocol are the best documented (Linux `iwlwifi` driver source and
  `linux-firmware` iwlwifi READMEs).
- QEMU can pass through a real Intel card to a VM (`vfio-pci`) for testing without needing bare metal for
  every iteration, if the development machine has one.

Whichever family is picked, scope to WPA2-PSK (CCMP/AES) only. Skip WPA3/SAE, skip enterprise (802.1X),
skip WEP/open (open networks can come later, they are strictly simpler — no handshake at all, just
association). Skip 5-address mesh, skip AP mode, skip monitor mode.

## New crates

- `crates/wifi80211` — 802.11 management frame construction/parsing (auth, assoc, beacon/probe parsing),
  the WPA2 4-way handshake state machine, and CCMP encrypt/decrypt. `no_std` + `alloc`. Depends on `sha2`,
  `aes` (already in the dependency tree via TLS — check `Cargo.lock` before adding a second AES
  implementation) and a new dependency for AES-CCM mode if `aes-gcm`'s sibling `ccm` crate isn't already
  pulled in.
- `crates/drivers/src/iwlwifi/` (a module in the existing `drivers` crate, not a new crate, since it plugs
  into `probe_all` exactly like `e1000.rs` does) — the PCI device driver: firmware loading, TX/RX ring
  management, register access.

## Step 1 — firmware handling

Unlike every existing driver in `crates/drivers`, this one needs a large binary blob that cannot be
`include_bytes!`'d into the kernel without an unacceptable ISO size hit (hundreds of KiB to over a MiB,
see `sizes.md`). Decide firmware sourcing **before** writing the PCI driver:

- **Recommended:** download the firmware from the network at install time, the same way package data is
  downloaded — except this is needed *before* networking exists, which is a chicken-and-egg problem specific
  to Wi-Fi. Two ways out:
  - Require a wired fallback for the *very first* HTTP request (fetch firmware over Ethernet/virtio, then
    switch to Wi-Fi for the rest) — defeats the purpose for a Wi-Fi-only machine.
  - Ship the firmware on a separate, optional ISO/USB volume the user attaches only if they need Wi-Fi
    (e.g. a second Limine module list entry, only loaded if present) — keeps the base ISO small, makes
    Wi-Fi opt-in at boot time by whether the extra volume is present. This is the only approach that keeps
    the base ISO's size promise; implement this one.
- Store the firmware module as its own Limine module (`xtask/src/iso.rs`'s `LIMINE_CONF` already lists
  modules by path; add an optional one, and have `kernel/src/main.rs` check whether it was actually loaded
  before deciding Wi-Fi is available — Limine allows a module path to be missing, check its module-loading
  API for how a missing module is reported).
- Whichever approach is picked, get the actual firmware bytes from the `linux-firmware` Arch package (same
  trust model as everything else: pin a version, check its sha256, like `xtask/src/keyring.rs` does for
  `archlinux-keyring` — read that file for the pattern to copy).

## Step 2 — PCI driver skeleton (`crates/drivers/src/iwlwifi/`)

Follow `crates/drivers/src/e1000.rs`'s structure: a `probe(dev: &PciDevice, out: &mut Devices)` entry point
called from `probe_all`, matching on vendor 0x8086 and a table of known device IDs (see how `e1000.rs` and
`r8169.rs` keep an ID table today — copy that pattern, do not invent a new one).

1. Map BAR0 (MMIO) via `drivers::mmio::Mmio`, same as every other driver here.
2. Reset sequence and firmware upload: this is the most iwlwifi-specific part. The firmware is a TLV
   container (`struct fw_img` sections in Linux's `iwlwifi/fw/img.h` — documentation reference only, do
   not copy GPL code, re-implement from the public format description and from packet captures/register
   traces) loaded into device SRAM through a DMA'd "transport" descriptor ring, then the device is told to
   jump to it.
3. Command ring ("TX cmd queue 0" in Linux terms) for firmware commands (`MVM_` opcodes: `ADD_STA`,
   `PHY_CONTEXT_CMD`, `MAC_CONTEXT_CMD`, `SCAN_REQ_UMAC`, ...) and an RX ring for firmware notifications —
   structurally similar to the NVMe submission/completion queue pattern already in
   `crates/drivers/src/nvme.rs`; reuse that polling style (poll a status/doorbell register, no interrupts).
4. This step alone (firmware transport + command protocol) is comparable in effort to the entire rest of
   the installer combined. Budget for it accordingly and get a minimal "load firmware, get a firmware
   version response back" milestone working and tested before attempting association.

## Step 3 — scanning and association (`crates/wifi80211`)

1. Build a `SCAN_REQ_UMAC` firmware command (or passive-listen to beacons if the chosen chipset supports a
   simpler path) to find the configured SSID's BSSID and channel. The config schema (`config/src/lib.rs`)
   needs a new `wifi` section: `{ ssid, psk, band_hint }` — add it following the existing config struct
   conventions and update `config/src/lib.rs`'s validation the same way other string fields are validated
   (see how `hostname`/`mirrors` are checked, to reuse the same "reject unexpected characters" approach
   mentioned in `README.md`'s configuration section).
2. Send `MAC_CONTEXT_CMD`/`PHY_CONTEXT_CMD` to configure the channel, then 802.11 authentication (open
   system) and association request/response frames through the firmware's frame-injection command.
3. Parse the association response for confirmation; extract the AP's supported cipher suite from the
   beacon/probe response's RSN information element to confirm CCMP is offered (fail with a clear error if
   the AP only offers TKIP/WEP or WPA3-only SAE — those are out of scope).

## Step 4 — WPA2 4-way handshake (`crates/wifi80211::wpa2`)

1. Derive the PMK from the PSK and SSID: `PBKDF2-HMAC-SHA1(psk, ssid, 4096, 256 bits)` — a small addition,
   `pbkdf2` + `sha1` crates (check if `sha1` is already pulled in transitively; if not, this is a new, small
   dependency — SHA-1 only, WPA2 does not use SHA-256 for the PMK derivation).
2. Implement the 4-way handshake as a small state machine reacting to EAPOL-Key frames from the firmware's
   RX path:
   - Message 1 (from AP): extract ANonce, generate SNonce, derive PTK
     (`PRF-384/512(PMK, "Pairwise key expansion", MAC_AP || MAC_STA || ANonce || SNonce)` — reuse whichever
     PRF construction matches the negotiated AKM, HMAC-SHA1-based for WPA2).
   - Message 2 (to AP): SNonce + MIC computed with the derived KCK.
   - Message 3 (from AP): verify MIC, install the GTK (delivered encrypted with the KEK, unwrap with
     AES key-wrap — a new small primitive, check if the `aes` crate's ecosystem already has a `key-wrap`
     crate before writing one).
   - Message 4 (to AP): final ack.
3. Install the derived PTK/GTK into the firmware via the appropriate `ADD_STA`/key-config command so the
   firmware/hardware does CCMP encrypt/decrypt for data frames transparently. (Whether the chosen chipset
   does crypto in firmware or expects the host to do it in software depends on the exact generation — check
   the specific chipset's capability flags before assuming; if the host must do CCMP itself, that is
   additional code in `crates/wifi80211::ccmp` wrapping the `aes`/`ccm` crates already mentioned above.)

## Step 5 — `hal::NetDevice` adapter

Same shape as every other driver: `transmit`/`receive` push/pop 802.11 data frames (802.3 frames unwrapped
from/wrapped into 802.11 QoS data frames with the negotiated encryption applied), `link_up` reflects
whether association + 4-way handshake completed. Register it from `probe_all` like every PCI NIC driver.

## Testing strategy

1. Unit tests (host, `std`): PMK/PTK derivation against known WPA2 test vectors (IEEE 802.11i test vectors
   are public), CCMP encrypt/decrypt against test vectors, 802.11 frame parsing against captured beacon/
   association fixtures (capture with a real card in monitor mode, or use existing public pcap samples —
   check their license before committing them as fixtures; regenerate from a real network if unclear).
2. Firmware command protocol: near-impossible to unit test meaningfully without real hardware or a very
   detailed device model. Budget time for iterating against real hardware with a serial-console log of
   every firmware command and response, similar to how `xtask e2e` already validates behavior via serial
   log assertions (see `xtask/src/e2e.rs` for the pattern of "boot, wait for a specific serial line, assert
   success").
3. `cargo xtask e2e --wifi` (new flag), manual only — QEMU has no realistic Wi-Fi/iwlwifi device emulation,
   so this cannot be automated in CI. A real machine with the target chipset and a real AP (or the iPhone
   hotspot in Wi-Fi mode, which is a convenient always-available WPA2-PSK AP for testing) is required.
4. Track ISO size impact with `cargo xtask size` after wiring in the optional firmware module; confirm the
   base ISO (without the firmware module attached) stays at its current size, and only grows when the
   firmware module is actually present at build time, per the "opt-in extra volume" design in Step 1.

## Non-goals (explicitly do not implement)

- WPA3/SAE, WPA-Enterprise/802.1X, WEP, open networks (open can be a small follow-up, not part of this
  effort), mesh, AP/monitor mode.
- Any chipset family beyond the one chosen in the scope decision. Adding a second family later is a
  repeat of steps 1-2 for that family's firmware/command format; steps 3-4 (802.11 logic, WPA2 handshake)
  are already shared and chipset-independent once written.
- Roaming between multiple APs with the same SSID, background rescanning, power management.
- iPhone tethering — see `iphone-tethering.md`, a separate and much smaller effort that this file's scope
  decision explicitly should not be conflated with (both are optional network sources but solve different
  problems and share no code).

## Suggested implementation order (for an agent tackling this in one pass)

1. Firmware sourcing/packaging decision implemented (opt-in Limine module, `xtask` download+pin, like
   `xtask/src/keyring.rs`).
2. `crates/drivers/src/iwlwifi/`: reset + firmware upload + "get firmware version" milestone.
3. Command ring + a minimal `SCAN_REQ_UMAC` round-trip, logged over serial for manual inspection.
4. `crates/wifi80211`: frame parsing/construction, tested against fixtures.
5. Association (open system) against a real AP, confirmed over serial log.
6. `crates/wifi80211::wpa2`: handshake state machine, tested against IEEE test vectors.
7. Wire the handshake into the association flow against a real AP with a real PSK.
8. `hal::NetDevice` adapter, wire into `probe_all`, run a full `cargo xtask e2e --wifi` by hand.
9. Update `README.md` (networking section) and `PLAN.md` to describe Wi-Fi as an optional, separately
   downloaded add-on rather than a default part of the ISO.
