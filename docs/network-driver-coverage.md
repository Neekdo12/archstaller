# Network driver coverage roadmap

This document defines a practical path to broaden wired networking support in Archstaler's installer.
Targets are **at least 95% coverage of a documented x86_64 PCI Ethernet hardware cohort** and **at least
70% coverage of a documented USB Ethernet/tethering cohort**. These percentages are goals to measure,
not claims about every computer or network adapter made from 2000 to the present.

## Scope and honest coverage claims

The installer targets `x86_64-unknown-none` only. This is not multi-architecture support. Most PCs
manufactured around 2000 use 32-bit processors; they are outside the target regardless of whether
their Ethernet NIC is supported. AMD64 processors appeared in 2003 and mainstream x86_64 desktop
systems followed later. Define the intended hardware period as **x86_64 systems from the earliest
supported platform generation through the current test date**, not all machines manufactured since
2000.

The percentage denominators must be explicit:

- **PCI target:** Ethernet PCI/PCIe adapters in a versioned hardware corpus of x86_64 desktops,
  laptops, workstations, and servers. Include onboard Ethernet and add-in cards. Exclude Wi-Fi,
  non-x86_64 systems, and devices with no x86_64-capable host.
- **USB target:** USB Ethernet adapters and USB tethering network functions in a separate versioned
  corpus. This is not a percentage of all USB peripherals.
- Report both **device-ID coverage** and **tested-device coverage**. A driver recognizing an ID is not
  proof that initialization, link negotiation, DHCP, HTTPS downloads, and sustained transfers work.
- Do not infer market coverage from the number of driver modules, Linux driver's ID tables, or the
  number of entries in `pci.ids`. Device popularity and verified behavior matter; publish the corpus,
  method, and date alongside any percentage.

Until a representative dataset exists, report exact tested hardware and supported ID ranges rather
than saying the project has achieved 95% or 70% market coverage. Use a weighted sample when reliable
deployment counts exist; otherwise report unweighted corpus coverage and label it as such.

## Current baseline

PCI Ethernet support is in `crates/drivers`: `virtio-net`, Intel `e1000` and `igb` (including selected
igc-family devices), Realtek `r8169`, and `rtl8139`. These are optional Cargo features and are selected
for installer builds through the `hostcfg` installer-driver catalogue. PCI discovery is in
`crates/drivers/src/pci.rs`; devices are initialized from `drivers::probe_all()` and implement
`hal::NetDevice`.

USB networking is in `crates/usbnet`: RNDIS, CDC-ECM, CDC-NCM, and ASIX AX88179. iPhone tethering is
implemented in `crates/imobiledevice`. USB host support in `crates/usb` currently means xHCI, direct
devices only, and no hubs. Transfers and device drivers are polled; do not introduce device interrupts.

The existing shared `crates/net` stack should continue to consume `hal::NetDevice` implementations.
Adding another Ethernet controller must not require duplicating DHCP, DNS, HTTP, or TLS logic.

## Build the coverage corpus first

1. Collect PCI vendor/device IDs from public hardware inventories, supported-device tables, user
   hardware reports, and tested machines. Keep provenance and license notes for every source. Linux
   driver tables may help identify hardware, but do not copy GPL implementation code into this repo.
2. Normalize IDs to `(vendor_id, device_id, revision/subsystem where relevant)`, map them to hardware
   family and approximate release period, and record whether the source describes a wired NIC or a
   multifunction device.
3. Collect USB VID/PID and interface-class/protocol tuples for USB Ethernet adapters and tethering
   functions. Include standards-based CDC devices and vendor-specific products/rebrands.
4. Remove duplicate product rebrands only for the family-level summary; retain every observed ID in
   the machine-readable corpus so actual ID-table coverage can be computed.
5. Store the corpus and a small coverage tool in the repository. Each record should include source,
   confidence, hardware family, era, and test status. Missing provenance is not evidence of coverage.
6. Produce a report that lists total corpus entries, recognized IDs, initialized devices, link-tested
   devices, and end-to-end-tested devices separately, with percentages for each stage.

Keep the corpus privacy-safe: do not commit serial numbers, MAC addresses, hostnames, or personally
identifying inventory data.

## PCI Ethernet implementation roadmap

### Prioritize by corpus gap

Rank missing families by estimated prevalence, age, architectural diversity, implementation effort,
and availability of test hardware. Likely investigation candidates include older Intel PRO/100, 3Com
3c59x/3c90x, DEC/Intel Tulip-compatible devices, VIA Rhine, SiS, Broadcom b44/tg3, Marvell Yukon,
Qualcomm/Atheros atl1c/atl1e/atl1, and newer Realtek or Intel revisions not handled by current drivers.
This is a candidate list, not a promise that each family will be implemented; use the collected
corpus to choose the next driver.

Prefer support that covers a family of IDs through a shared controller generation. Avoid adding
duplicate implementations for vendor rebrands of one controller. Maintain per-family ID tables and
test subsystem/revision exceptions explicitly when register behavior differs.

### Driver requirements

1. Add a feature-gated module in `crates/drivers` and register it in `probe_all()`. Add its ID and
   description to the `hostcfg` driver catalogue so config validation, GUI selection, and kernel build
   feature selection remain consistent.
2. Follow the existing polling style in `e1000.rs`/`nvme.rs`: bounded waits with timeouts, no device
   interrupts, clear diagnostics, and safe failure that allows other probed NICs to remain usable.
3. Implement PCI matching, BAR mapping, bus mastering, DMA allocation/alignment, descriptor rings,
   reset/initialization, MAC discovery, link state, transmit, and receive. Audit DMA addresses against
   the kernel's identity/direct-map and address-width constraints; do not assume all devices support
   64-bit DMA.
4. Return a `hal::NetDevice`; keep Ethernet frame handling compatible with `crates/net`. Do not place
   DHCP/IP/TLS policy in the device driver.
5. Add host unit tests for descriptor layouts, register fields, reset state machines, and packet-ring
   transitions where they can be tested without MMIO. Document which behavior still requires real
   hardware.
6. Keep unsupported revisions from probing as if initialized. Unknown devices should be logged with
   vendor/device IDs and skipped without blocking boot or hiding another working adapter.

For very old PCI NICs, assess whether the device can function in polling mode and whether it needs
firmware. A family that fundamentally requires interrupts or unavailable firmware is not a suitable
fit for this installer just to raise an ID-count percentage.

## USB Ethernet implementation roadmap

Achieving a useful 70% target requires expanding both the host-controller and network-device layers;
adding more USB VID/PID entries alone is insufficient.

1. Build a USB device corpus using VID/PID plus interface descriptors. Separate devices that expose
   standard CDC-ECM/NCM or RNDIS from truly vendor-specific data paths. Count common rebrands correctly.
2. Extend USB host support beyond the current xHCI-only path where the corpus justifies it. Evaluate
   EHCI for USB 2.0-era machines and OHCI/UHCI for USB 1.x-era ports. Controllers must retain the
   existing bounded polling behavior and safe ownership handoff from firmware.
3. Add hub support before claiming broad adapter compatibility. Many internal laptop radios/docks and
   external hubs place the network function behind a hub; current direct-attached-only enumeration
   misses these devices. Hub support needs bounded depth/device counts, port-reset timeouts, disconnect
   handling appropriate to a boot-time scan, and tests for malformed descriptors.
4. Add high-value USB Ethernet families according to the corpus, likely including ASIX AX88772, Realtek
   RTL8152/RTL8153/RTL8156, and SMSC LAN95xx, in addition to current AX88179 and CDC classes. Keep
   class drivers preferred over vendor-specific paths when they correctly expose the same interface.
5. For each vendor protocol, implement only the control requests, register setup, framing, and link
   negotiation needed for `hal::NetDevice`. Apply strict bounds to descriptor parsing, transfer sizes,
   NTB/RNDIS packet counts, and all device-provided offsets and lengths.
6. Add USB ID/interface tables from sources whose licensing permits their use, record source and
   supported revisions, and test rebrands that use different descriptors despite sharing a chip.
7. Keep iPhone and Android tethering as explicitly tested network functions; do not count every phone
   as supported merely because its vendor class is recognized. Users may need to enable tethering on
   the phone before the function appears.

If Wi-Fi over USB is considered later, it is a separate scope decision: a USB Wi-Fi chipset still needs
radio firmware, 802.11 management, authentication, encryption, and association support. Do not count it
toward USB Ethernet coverage or silently expand this work into Wi-Fi.

## Validation ladder

For every new driver family, use the following evidence levels and report the highest level actually
passed:

1. **Decode tests:** host tests for descriptors, register values, and packet framing using public
   specifications and project-authored test vectors.
2. **Probe tests:** confirm matching IDs initialize and unknown IDs are safely rejected. Use QEMU
   device emulation where available; do not claim emulation for hardware QEMU does not model.
3. **Link tests:** confirm negotiated link state and MAC address on real hardware or a faithful device
   model.
4. **Network tests:** DHCP, DNS, HTTPS package fetch, and sustained frame transfer through the shared
   network stack. Check packet loss and ring wraparound under load.
5. **Installer e2e:** install with the driver selected, verify package downloads and installation
   completion, then retain serial logs and hardware identification for the report.

Add test fixtures only when their licenses permit redistribution. Keep a hardware test report with
controller model/revision, link partner/speed, firmware mode, test date, and pass/fail evidence; redact
MAC addresses and serial numbers.

After changes, run the narrow crate tests and the relevant `cargo xtask` checks. Driver-feature changes
must also update `OVERVIEW.md` and the `hostcfg` driver catalogue. Kernel/ISO changes require
`cargo xtask size`; hardware claims require the corresponding evidence above.

## Reporting the targets

Call PCI coverage **95% achieved** only when the published, versioned x86_64 wired-NIC corpus shows at
least 95% recognized and initialized coverage and a separately disclosed end-to-end-tested percentage.
Call USB coverage **70% achieved** only when the analogous USB Ethernet/tethering corpus reaches that
threshold. Publish the denominator, weighting, exclusions, unknown devices, and last test date.

Neither percentage implies support for 95%/70% of all computers, all network interfaces, Wi-Fi,
non-x86_64 systems, or every machine from calendar year 2000 onward. Keep estimates and verified results
clearly separate.