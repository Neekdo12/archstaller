# Hardware, networking and boot media

What the installer runs on, which network adapters it drives, how USB tethering works and how to boot it from a
stick. The list of network ids is in [`network-driver-coverage.md`](../plans-implement/network-driver-coverage.md).

## Virtual machines and USB sticks

- Firmware: BIOS or UEFI both work; **Secure Boot must be off** (neither the ISO nor the installed system
  supports it). Only x86_64 is supported; there is no ARM or Raspberry Pi support.
- The installer has **no USB storage drivers**. It sees NVMe, AHCI/SATA and virtio disks only, so an installer USB
  stick is never a candidate for `auto_largest`. It also means it cannot install onto a USB disk. The only
  USB code is the optional phone tethering described under Networking.
- Everything the installer needs is loaded into RAM by the boot loader before the kernel starts, and
  packages come from the network, so the stick can be pulled once the installer's first log lines have
  appeared. **Pull it before the final reboot**: if the firmware still prefers the stick, the machine boots
  the installer again and, with `auto_largest`, erases the system that was just installed.
- Networking: wired PCI NICs, plus optional iPhone and Android USB tethering and USB Ethernet dongles (below). No Wi-Fi. The installer drives these families (the exact PCI ids are in each driver):
  virtio-net; Intel e1000 (every id of Linux's `e1000`: 8254x) and e1000e (82571-82574, 82583, 80003ES2LAN, ICH8-ICH10,
  PCH 82577-82579, I217, I218, I219) and igb/igc (82576, 82580, I350, I210/I211, I225/I226); Realtek RTL8101/8102/8103/
  8105/8106 (the 100 Mbit/s 8136), RTL8168/8111 steppings b, c, cp, d, dp, e, evl, f, g, h, ep, RTL8411, RTL8169
  s/sb/sc, RTL8125/8126 and RTL8139; Qualcomm Atheros AR8131/8132/8151/8152 (atl1c) and AR8161/8162/8171/8172 and
  Killer E2200/E2400/E2500 (alx); VMware vmxnet3. Realtek chips are identified by their TxConfig revision and get
  Linux's per-revision start sequence (the 8168/8111 revision is printed in the log). Tested in QEMU's emulated NICs:
  virtio, e1000, e1000e, igb, rtl8139 and vmxnet3, each with an ARP exchange, DHCP, a TLS download from the Arch
  mirror and a 1 MB HTTP transfer, and for igb and rtl8139 also the start of a real installation. **Real hardware**:
  the RTL8168evl/8111evl (xid 0x2c9, Gigabyte GA-F2A88XM-D3H) works, including DHCP, the package downloads and a full
  install, on a network that needed the DHCP address probe (below); the AX88179 USB dongle works too. **Never run**:
  igc, every other Realtek revision (the new start sequences were ported from the Linux driver without the chips),
  the Atheros drivers and the PCH-integrated Intel parts from the I217 on, because QEMU has no models for them;
  they follow the Linux drivers' setup and may not work on a given chip revision (the RTL8125/8126 in particular
  need chip-specific tuning in Linux). Some PCI IDs, especially igc's, were written from memory and are unchecked.
  When a NIC is not recognized the installer prints every storage and network PCI device it found (`pci 02:00.0
  10ec:8136 class 020000`). Not supported: Broadcom `tg3`/`bnx2`, Marvell Yukon/`sky2`, JMicron, nVidia nForce,
  VIA, SiS, Aquantia and 10 Gbit NICs.
- DHCP behaviour: the offered address is ARP-probed before use, like Linux clients do. If another host already uses it
  (a static device inside a DHCP range), the installer sends a DHCPDECLINE and asks again, up to 4 times. DNS falls
  back to 1.1.1.1 and 8.8.8.8 if the DHCP-provided servers do not answer. Old CPUs without `RDRAND` get a weaker
  timing-jitter random generator (the tester reports a warning).
- USB tethering (optional; off in a plain `cargo xtask build`, on in `cargo xtask presets` unless `--no-tethering`): build with `cargo xtask build --tethering`. It only runs when no
  wired NIC has link, and the phone then appears as one more NIC. It uses an xHCI controller (`crates/usb`).
  - **iPhone** (worked on one real iPhone): plug in an unlocked iPhone with Personal Hotspot on and tap "Trust"
    (and enter the passcode) when it asks; the installer waits up to 120 s. It pairs through lockdownd over
    the phone's usbmux interface (`crates/imobiledevice`) and then uses the phone's standard tethering
    interface, the one Linux's `ipheth` driver uses. Pairing is redone on every run.
  - **Android** (worked on one real phone, RNDIS or ECM not recorded; also tested against QEMU's emulated
    adapters): switch "USB tethering" on in the phone's settings (the phone must be unlocked), then plug it in. The
    installer scans again for about 20 s, so enabling tethering after plugging in also works. No pairing is needed.
  - **USB Ethernet dongles** (`crates/usbnet`): class-compliant adapters that offer CDC-ECM or CDC-NCM, and
    ASIX AX88179 gigabit adapters (for example the Axagon ADE-SG; vendor-specific protocol, matched by USB id).
    Plug a cable into a router first: the AX88179 driver waits up to 15 s for link. The AX88179 works on real
    hardware (link, DHCP, downloads). RNDIS and ECM have run against QEMU's emulated adapter; the NCM framing
    has host unit tests only. Other vendor-specific chips (ASIX AX88772, Realtek RTL8152/8153) are not
    supported unless the dongle also offers an ECM or NCM configuration.
  Every step prints a `usb:` log line, so a failure shows how far it got.

## Ventoy

The ISOs boot from [Ventoy](https://www.ventoy.net): copy the `archstaler-*.iso` files to the Ventoy data
partition, boot the stick, pick an ISO, and choose **Boot in normal mode** (the first entry of the boot
mode menu that Ventoy shows; grub2 and memdisk mode are not needed).

This was tested with Ventoy 1.1.17 in QEMU (while the ISO still booted through Limine; the custom boot loader that replaced it has not been tried under Ventoy yet), in both BIOS and UEFI mode, using an image laid out like a
Ventoy stick (MBR, Ventoy's boot code and EFI partition, FAT32 data partition) attached as a USB drive:
the menu lists the ISOs, the installer starts, sees only the blank target disk, and keeps installing after
the virtual stick is removed. Not tested: real hardware, an exFAT data partition (Ventoy's default; Ventoy
reads FAT32 and exFAT alike), other Ventoy modes and Ventoy's Secure Boot mode.
