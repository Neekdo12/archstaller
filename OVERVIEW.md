# OVERVIEW.md

Current architecture of archstaler. Kept in sync with the repo by rule (see `AGENTS.md`).

## What it is

An Arch Linux installer that boots without any Linux kernel. Limine (BIOS + UEFI) loads a custom
`no_std` Rust mini-kernel that installs Arch onto disk: partitions, downloads and verifies packages over
HTTPS, writes the filesystem, and hands off to a first-boot service that runs real pacman/systemd steps
that need a running Linux.

## Boot/install flow

1. **Limine** loads `kernel`, plus modules: `config.bin`, `keyring.bin`, `tiny-init`, Limine's own
   BIOS/UEFI files (see `xtask/src/iso.rs`'s `LIMINE_CONF`).
2. **kernel** (`kernel/src/main.rs`, `install.rs`) runs, single core, polling only:
   - enumerates disks/NICs via `crates/drivers::probe_all()` (PCI scan). With the `usb-tethering` kernel
     feature it also tries `imobiledevice::tether()` and, on success, adds the iPhone as one more NIC.
   - selects the target disk per `Config.disk` (serial match, or `auto_largest`); writes nothing until
     this succeeds.
   - DHCP, then downloads `core.db`/`extra.db` over HTTPS (`crates/net`).
   - resolves packages (`crates/pkg`): pacman-compatible `vercmp`, versioned deps, soname provides,
     groups, repo priority, conflicts.
   - partitions (`crates/disk`): GPT + protective MBR, BIOS boot partition, ESP (FAT32, `/boot`), root.
   - writes root as ext4 (`crates/ext4w`), write-once, no journal.
   - streams each package into `/var/cache/pacman/pkg` on the target, verifying SHA-256 and PGP signature
     (`crates/pgp-lite` + `keyring.bin`) before extracting.
   - writes `/etc` (fstab, hostname, locale, mirrorlist, ...), builds an initramfs (`crates/initrd` +
     `tiny-init`), installs Limine onto the target ESP/BIOS boot partition, reboots.
3. **First boot** (`firstboot/`): `archstaler-firstboot.target`/`.service` runs `pacman-key --init/
   --populate`, `pacman -U --overwrite '*'` on the cached packages (runs real scriptlets/hooks, writes
   the local pacman DB), creates users, enables services, adds the ext4 journal, builds the real
   initramfs, rewrites the Limine entry, reboots into the installed system.

## Repository layout

| Path | Contents |
|---|---|
| `kernel/` | the installer kernel: console (`fb.rs`, `serial.rs`, `console.rs`), heap (`heap.rs`), exceptions (`idt.rs`), paging (`paging.rs`), TSC clock (`time.rs`), install flow (`install.rs`), entry point (`main.rs`) |
| `xtask/` | host tooling: `iso.rs` (build kernel + assemble ISO), `lua.rs` (config.lua -> config.bin), `keyring.rs` (keyring blob + pin), `limine.rs` (fetch pinned Limine), `qemu.rs` (run in QEMU), `e2e.rs` (install + boot test), `presets.rs`, `linux_test.rs`, `main.rs` (CLI) |
| `config/` | `Config`/`Disk`/`User`/`UserFile`/`UserArchive` types shared by `xtask` and `kernel`, plus validation |
| `crates/hal` | traits: `BlockDevice`, `NetDevice`, `Clock`, `Rng` |
| `crates/drivers` | PCI enumeration (`pci.rs`), virtio blk/net, AHCI, NVMe, Intel e1000/e1000e/igb(+igc), Realtek r8169/r8125/rtl8139 — all polled, no IRQs |
| `crates/usb` | xHCI host controller driver (polled): command/event/transfer rings, enumeration of enabled ports, descriptors, control + bulk transfers. One controller, no hubs |
| `crates/imobiledevice` | iPhone USB tethering: plist (binary + XML) codec, usbmuxd framing over bulk endpoints (`mux.rs`), pairing with a generated RSA-2048 host identity (`cert.rs`, `pair.rs`), TLS-wrapped lockdownd session + `StartService` (`lockdown.rs`), `hal::NetDevice` adapter (`netdev.rs`). Unit-tested on the host only; needs a real phone end-to-end |
| `crates/net` | `smoltcp` stack glue (`stack.rs`), HTTP/1.1 client (`http.rs`, `client.rs`), TLS via `rustls` + `rustls-rustcrypto` + `webpki-roots` (`tls.rs`) |
| `crates/pgp-lite` | OpenPGP v4 signature verification (RSA, EdDSA) against an embedded keyring blob |
| `crates/pkg` | sync DB parser (`desc.rs`, `db.rs`), `vercmp` port, dependency resolver (`resolve.rs`), tar/zstd/gzip readers (`tar.rs`, `compress.rs`, `io.rs`) |
| `crates/ext4w` | write-once ext4 writer (`writer.rs`): extents, xattrs, symlinks, hardlinks; no journal/metadata_csum/dir_index at install time |
| `crates/disk` | GPT + protective MBR (`gpt.rs`), FAT32 writer (`fat32.rs`), CRC32 (`crc32.rs`), Limine BIOS installer port (`limine.rs`), region helpers (`region.rs`) |
| `crates/initrd` | cpio newc writer (`cpio.rs`), kernel module dependency resolution from ELF `.modinfo` (`modules.rs`) |
| `tiny-init/` | the initramfs `/init`: raw syscalls only, no libc, `no_std`; loads modules, mounts root, `switch_root` |
| `firstboot/` | systemd unit files + `firstboot.sh`, embedded into the image at install time, run on first boot |
| `presets/`, `examples/` | Lua configs; `presets/common.lua` holds shared defaults, `examples/config.lua` is the documented example, `examples/e2e.lua` is used by `xtask e2e` |
| `docs/` | `wifi.md` (spec for a not-implemented feature), `iphone-tethering.md` (spec the tethering crates were written from) |
| `sizes.md` | measured ISO size breakdown and size-reduction options |
| `PLAN.md` | original design plan/decision log |

## Config (`config.lua` -> `config.bin`)

Evaluated on the build host by `xtask` (`mlua`), serialized with `postcard`, embedded as a Limine module.
Schema in `config/src/lib.rs`: `hostname`, `timezone`, `locale`, `keymap`, `disk` (selector + `esp_mib`),
`mirrors`, `packages`, `providers` (dependency -> chosen package), `root_password_hash`, `users`
(`password_hash` is SHA-512 crypt, never plaintext), `services`, `kernel_params`, `user_files`,
`user_archives`. `xtask` validates values strictly (they end up in shell-read files, unit files, boot
loader config).

## Trust model

- Keyring blob (`boot/keyring.bin`) is derived at build time from a pinned `archlinux-keyring` package
  (`xtask/keyring.pin`, sha256-checked), reduced to fingerprint + pubkey + expiry per packager key that
  has web-of-trust certification from the main Arch signing keys (`xtask/src/keyring.rs`).
- Package signatures come from the sync DB's `%PGPSIG%` field, verified against that blob
  (`crates/pgp-lite`) before extraction; SHA-256 from `%SHA256SUM%` is checked too.
- Sync databases themselves are not signed; their integrity relies on HTTPS.
- `cargo xtask update-keyring` moves the pin forward and rebuilds the blob.
- First boot's `pacman -U` re-verifies everything with real GnuPG as a backstop.

## ISO layout (assembled by `xtask/src/iso.rs`)

ISO9660 (via `xorriso`, hybrid El Torito BIOS + UEFI), containing `boot/kernel`, `boot/config.bin`,
`boot/keyring.bin`, `boot/tiny-init`, `boot/limine/*` (Limine's BIOS files + `limine.conf`),
`boot/limine-bios-hdd.bin` (MBR stage 1 for the *target* disk's BIOS install), `EFI/BOOT/BOOTX64.EFI`.
Current total ~2.3 MiB; see `sizes.md` for the full byte-by-byte breakdown and reduction ideas.

## Testing

- Host unit tests per crate (`cargo test -p <crate> --features std`) check against real tools/data:
  `vercmp` vs `/usr/bin/vercmp`, resolution vs `pacman -Sp`, `ext4w` images vs `e2fsck`/`debugfs`, GPT vs
  `sfdisk`/`fdisk`, FAT32 vs `fsck.fat`, BIOS installer vs `limine bios-install`.
- `cargo xtask linux-test`: boots the host Linux kernel with our initramfs + an ext4w-built root.
- `cargo xtask e2e [--uefi] [--disk ...] [--nic ...] [--config FILE]`: full install in QEMU onto a blank
  disk, then boots the result twice.
- `cargo xtask run --usb`: adds `qemu-xhci` + `usb-storage` and builds the `usb-selftest` kernel, which enumerates the USB device and runs a SCSI INQUIRY over bulk endpoints.
- `cargo xtask size [--small] [--limit BYTES]`: ISO content breakdown + size budget check.
- `cargo xtask check-presets`: resolves every preset against local pacman sync DBs.

## iPhone USB tethering (optional)

Off by default; build with `--features usb-tethering` on the kernel (`cargo xtask build --tethering`).
`kernel/src/main.rs` calls `imobiledevice::tether()` after `probe_all()`: xHCI init, find an Apple device,
usbmux `Connect` to lockdownd (port 62078), `Pair` (the user taps "Trust"; retried for 120 s; the pairing
record is not persisted), TLS `StartSession`, `StartService` for the hotspot relay, then a second mux
channel that becomes `IphoneNet`. `crates/net` is untouched: the relay carries raw IP, so `IphoneNet`
adds/strips a 14-byte Ethernet header (`RELAY_RAW_IP` in `crates/imobiledevice/src/lib.rs`). `install::run`
uses the first NIC with link, so a wired NIC wins if present. The relay service name, mux device id and
raw-IP-vs-Ethernet choice are marked UNVERIFIED in the code: they were never run against a real iPhone.
The xHCI driver itself is smoke-tested in QEMU with `cargo xtask run --usb` (`qemu-xhci` + `usb-storage`,
bulk-only INQUIRY). Adds about 120 KiB to the kernel.

## Out of scope

Wi-Fi, other USB devices, SMP, Secure Boot, an interactive UI, filesystems other than ext4, architectures
other than x86_64. A spec exists for Wi-Fi in `docs/wifi.md`; it is not implemented.
