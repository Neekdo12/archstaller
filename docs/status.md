# Status

Verified in QEMU (BIOS and UEFI; virtio, AHCI, IDE-mode and NVMe disks; virtio, e1000, e1000e, igb and rtl8139 NICs):
install, first boot and a second boot to a login prompt, for the `i3` and `hyprland` presets and a minimal
test config, plus the Ventoy boot described above. The `minimal`, `server` and `plasma` presets resolve
(`check-presets`) but have not been through a full install in the test harness. Verified on real hardware: installs on an old Gigabyte GA-F2A88XM-D3H (RTL8168evl, no RDRAND) through
first boot; the second boot failed there once with a missing journal, which is fixed by writing a real internal
journal (checked with `e2fsck`/`debugfs` and in QEMU, not yet re-run on that machine). Not verified: the igc, Atheros and
new Realtek paths (other RTL8168/8169/8101 revisions, RTL8125/8126), real hardware in general, logging in with the default
credentials, starting the Hyprland session with the downloaded config, and the UEFI boot entry created by
`efibootmgr` (GRUB is installed to the fallback path, so booting works through `EFI/BOOT/BOOTX64.EFI`). The first-boot log is only in
the journal and is not persisted.

USB tethering: the iPhone path worked on real hardware (an ASUS ExpertBook with an unlocked iPhone); the Android path (RNDIS, CDC-ECM) worked on one real phone and in QEMU's emulated adapters; the AX88179 dongle worked on real hardware. `xtask e2e` does not exercise tethering.

Not supported: Wi-Fi, USB devices other than tethering phones and Ethernet dongles, ARM and Raspberry Pi, and installing onto anything but NVMe, SATA/AHCI, IDE-mode ATA and virtio
disks. The ISO is about 0.7 MB (`super-small`) to 0.8 MB (`large`); tethering adds about 105 KiB; the design notes in `PLAN.md` describe what was
aimed at and what remains.

GUI package search (2026-10-08, real core and extra from the mirror, 15,325 packages, X11 under i3): typing
`fierfox` listed `firefox` first; Add appended it and the live total (321 packages, 702.3 MiB for the default list
plus Firefox) matched the resolve preview (321 packages, 702 MiB). Importing on an Arch desktop added 151 of
158 explicitly installed packages (6 were listed, `steam` from multilib was reported as skipped) and listed the
19 foreign ones without adding them. An empty mirror shows the error with Retry. Not run: a machine without pacman.
