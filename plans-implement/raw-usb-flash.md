# Raw USB installer flashing

**Status: implemented, not tested.** No Rust toolchain was available when it was written, so it has not been
compiled, its unit tests have not run, and no real stick has been flashed. Before calling it supported: `cargo test
-p archstaler-gui`, then the manual tests below on a disposable stick. Where the code is:

- `gui/src/flash.rs`: devices from `lsblk -J -b` (util-linux 2.37+, for `MOUNTPOINTS`) plus sysfs (`/sys/dev/block/M:m`
  for the USB ancestry, `holders/`); `problems()` holds the eligibility rules; `RawBlockFlash` is the `MediaTarget`;
  `Backend` is the injectable device interface, `UDisks` the real one (gio's D-Bus client, already linked by GTK, no new
  D-Bus crate; `libc` for `posix_fadvise`). Tests use an lsblk fixture and a regular file as the device.
- `gui/src/pages/iso.rs`: the "Flash ISO to USB (erases device)" button (always offered) and the
  dialog (`flash_dialog`, `flash_scan`, `flash_show`).

Decisions the text below left open:

- **Mounted partitions.** A stick whose file systems are mounted under `/run/media`, `/media` or `/mnt` (the desktop's
  automount places) is offered and unmounted through UDisks2 before writing; a mount anywhere else, active swap or any
  holder (dm-crypt, LVM, RAID) makes it ineligible.
- **USB ancestry** needs both udev's transport (`TRAN=usb`) and a `/usb` component in the resolved sysfs path; one
  without the other is treated as conflicting metadata. The device must also be removable or hot-pluggable.
- **Identity** is the kernel name, major:minor, resolved sysfs path, size, vendor, model and serial. It is compared
  before unmounting, after unmounting, and after the device is opened; the opened handle's `st_rdev` must equal the
  selected major:minor.
- **Confirmation** is typing the kernel name (`sdb`) after selecting a drive; nothing is preselected.
- **Opening**: `org.freedesktop.UDisks2.Block.OpenDevice("rw", {flags: O_EXCL|O_CLOEXEC})` with interactive
  authorization, which is why UDisks2 2.7.3 or newer is required (checked through the Manager's `Version` property; when
  it is missing or older, the dialog says so and only the folder copy remains).
- **Read-back** uses the same handle: after `fsync`, `posix_fadvise(DONTNEED)` drops the cached pages so the bytes come
  from the stick. A drive that keeps a volatile cache of its own and lies about flushing could still pass.
- **Eject** is `Drive.PowerOff`, falling back to `Drive.Eject`; when both fail the result still reports a verified
  flash and tells the user to use the desktop's safe removal.

This document specifies a future GUI feature that writes the Archstaler hybrid ISO directly to a USB
block device. It is offered when no Ventoy data volume is detected. The operation erases the selected
USB device; it must never start automatically because Ventoy is missing.

## User flow

1. Keep the existing Ventoy path unchanged: when a mounted volume is identified as Ventoy, offer the
   current operation to copy the ISO as a regular file.
2. When Ventoy is not detected, show two distinct choices: choose another mounted folder for a normal
   file copy, or **Flash ISO to USB (erases device)**. Do not relabel ordinary mounted volumes as
   flashable USB targets.
3. On the flash path, enumerate eligible removable block devices and show manufacturer/model, device
   path, capacity, connection/bus information when available, and currently mounted partitions. Never
   preselect a target.
4. Require the user to select a whole device, not a partition, then show a destructive confirmation
   naming the device, size, and that every partition and existing file will be destroyed. Require an
   additional deliberate confirmation, such as typing the displayed device model or `/dev` basename.
5. Re-scan and revalidate the selected physical device immediately before unmounting and writing. If its
   identity or size changed, or it is no longer eligible, abort and require the user to select again.
6. Show separate unmounting, writing, flushing, and verification states. On success, report the target,
   ISO byte count, verification result, and offer safe eject/power-off through the desktop device
   service.

A Ventoy detection miss is not proof that a mounted drive is a USB stick. The user must explicitly
choose raw flashing and a separately enumerated eligible device.

## Target selection and data-loss safeguards

Raw flashing must have stricter device checks than the existing mounted-volume copy:

- Support Linux only initially, through UDisks2 or another audited desktop block-device API that obtains
  authorization through the normal desktop mechanism. Do not invoke `sudo`, interpolate device paths
  into a shell command, or ask the user to run a privileged command from the GUI.
- Enumerate block devices, not `sysinfo::Disks` mounted filesystems. Use system device metadata (for
  example udev/sysfs and UDisks2) to gather stable identity, size, removable status, USB ancestry,
  partition relationships, mounts, swap use, and device holders.
- Initially allow only whole removable USB mass-storage devices with a supported writable block device.
  Treat unknown or conflicting metadata as ineligible. Do not offer the system disk, a device containing
  `/`, `/boot`, the running system, the selected ISO, active swap, or a device with mounted/held partitions.
- Device names such as `/dev/sdb` are not stable identities. Bind the confirmation to the selected
  device's stable UDisks object and kernel identity (major/minor plus sysfs path and available hardware
  identifiers); re-check it immediately before writing to prevent a hotplug reorder from redirecting the
  operation to another disk.
- The required capacity is at least the ISO file length. Tell the user that existing partitions, data,
  and partition layout will be overwritten, and that unplugging during writing can leave the USB
  unusable until rewritten.
- Never infer consent from an earlier checkbox, preset, or missing-Ventoy state. A confirmation is per
  device and per flash operation.
- Keep the existing file-copy operation available. If a device cannot be proven safe to write, explain
  why and do not provide a force/override button in the first implementation.

## Writing and verification

Archstaler's ISO is a hybrid BIOS/UEFI boot image. Write its bytes starting at byte offset zero of the
whole target device; do not create a filesystem, partition the device separately, or write to a mounted
partition. The ISO's own partition/boot layout is the output.

1. Open and validate the ISO before requesting destructive confirmation. Record its file size and
   calculate its SHA-256 while streaming it, or use a previously verified build hash tied to the exact
   same file identity and size.
2. Ask UDisks2 to unmount every mounted filesystem on the target and deactivate swap if any exists;
   normally swap use should make the device ineligible instead of being forcibly stopped. Abort if any
   partition cannot be safely unmounted or the device remains busy.
3. Obtain exclusive access to the whole device using the desktop block-device service. Recheck device
   identity, capacity, read-only state, holders, mount state, and source/destination identity after
   exclusive access is obtained.
4. Stream the ISO to offset zero with bounded buffers, updating progress and checking cancellation
   between writes. Handle short writes and errors explicitly. Do not buffer the entire ISO in memory.
5. Flush userspace buffers and request the device/kernel cache to flush. Do not report success merely
   because the last write call returned successfully.
6. Read back exactly the ISO-length prefix from the target and compute SHA-256. It must match the ISO
   hash. The remaining device capacity is expected to contain old or undefined data and is not part of
   verification.
7. On any failure, report that the device may be partially overwritten and is not verified. Do not
   claim rollback is possible. Release exclusive access cleanly. Cancellation stops further writes at a
   buffer boundary, flushes what was written where possible, and reports a partial flash; it does not
   restore the previous contents.
8. After successful verification, release the device and use the supported desktop API to safely eject
   or power it off. If ejection is unavailable, still report verified completion and instruct the user
   to use the desktop's safe-removal action before unplugging.

## Code architecture

Keep media operations behind the existing `gui/src/media.rs` abstractions. Add a separate raw-device
model and a Linux-only `RawBlockFlash` target implementing the media operation contract; do not make
`FolderCopy` handle device nodes or weaken its path, free-space, overwrite, and read-back checks. Extend
progress phases to communicate unmounting, writing, flushing, verifying, and ejecting. Results should
identify the target and say explicitly whether verification passed.

Keep GTK/GUI code responsible for selection, warnings, progress, and confirmations. Keep the privileged
block-device interaction in a small, audited backend that receives a selected stable device identity,
not a raw string supplied by a widget. Feature-detect the required UDisks2 methods at runtime; if the
service is missing or authorization is denied, disable only the raw-flash option and explain why.

The implementation should be cancellable and asynchronous from the UI's perspective. All block I/O and
D-Bus calls run off the GTK main thread; typed progress and completion messages return to the main
context. Ensure that cancellation, window close, device removal, and authorization failure release
handles and locks without touching another device.

## Test plan

- Unit-test target eligibility using fixtures for removable USB devices, internal disks, root/boot
  ancestry, mounted partitions, swap, read-only devices, ambiguous identity, insufficient capacity,
  changed major/minor, and source/destination being the same device.
- Test the writer against a regular temporary file used as a block-device substitute: exact offset-zero
  contents, short writes, I/O failures, cancellation boundaries, flush/report behavior, and read-back
  hash mismatch. Keep destructive-device tests impossible to run against real devices by default.
- Add a fake UDisks2/D-Bus backend or an injectable block-device interface so authorization, unmount,
  write, flush, verification, cancellation, and device-disconnect paths are deterministic in tests.
- Manually test with disposable USB media on supported Linux desktops, recording device model, size,
  bus, desktop service version, and firmware mode. Boot the written media in both BIOS and UEFI QEMU
  configurations where practical; then test at least one real UEFI machine before advertising support.
- Verify Ventoy detection still chooses file-copy behavior and that a non-Ventoy mounted volume never
  starts raw flashing without the explicit path and destructive confirmation.

## Completion criteria

- Raw flashing is Linux-only, opt-in per operation, and never automatic on Ventoy detection failure.
- Internal/system/source disks are excluded; the selected USB device is revalidated immediately before
  destructive I/O.
- The full ISO-length read-back hash matches before the UI reports success.
- Errors, cancellation, device removal, and authorization denial leave a clear status and never claim
  that previously stored data can be recovered.
- `README.md`, the GUI documentation, and `OVERVIEW.md` clearly distinguish Ventoy file-copy from
  destructive raw flashing and state which platforms support each path.

## Authentication agent

Opening the drive makes UDisks2 ask polkit for the user's password, which needs an authentication agent in the session.
On a full desktop one runs already. Otherwise `gui/src/polkit.rs` starts an installed agent at launch and stops it on
exit (an agent that was already running is left alone). With none installed the flash fails with a message that says so.
