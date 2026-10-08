# archstaler – an Arch installer in Rust without the Linux kernel

A hybrid ISO (BIOS + UEFI) with Limine that starts our own `no_std` mini-kernel in Rust (one core, polling drivers).
It installs Arch from a mirror onto ext4 according to a pre-built Lua config. Everything that needs a running Linux
(pacman scriptlets, alpm hooks, mkinitcpio) is deferred to the first boot of the installed system.

## Decisions

- Packages: online from a mirror (the official Arch ISO is not used)
- Platform: x86_64, BIOS + UEFI through Limine, own drivers
- Config: `config.lua` is evaluated when the ISO is built (mlua on the host) → `config.bin` (postcard) as a Limine module
- Root FS: ext4
- Hooks/scriptlets: at the first boot (`pacman -U` from the cache)
- Secure Boot: not supported (neither the ISO nor the target); it must be off
- Out of scope: Wi-Fi, USB other than phone tethering (see below), SMP, Secure Boot, an interactive TUI, file systems other than ext4, other architectures

## Workspace layout

| Path | Description |
|---|---|
| `xtask/` | host: Lua evaluation, kernel + tiny-init build, keyring + CA blob, Limine (pinned + sha256), xorriso, QEMU run/test, size report |
| `kernel/` | `x86_64-unknown-none`, the `limine` crate, heap (`talc`), IDT for exceptions only, TSC timer, framebuffer + serial console |
| `config/` | shared serde config types (xtask + kernel) |
| `crates/hal` | the traits `BlockDevice`, `NetDevice`, `Clock`, `Rng` |
| `crates/drivers` | PCI, virtio-blk/net, AHCI, NVMe, e1000/e1000e, r8169 (polling) |
| `crates/net` | `smoltcp` (DHCP/DNS/TCP), HTTP/1.1 client, `rustls` no_std + `rustls-rustcrypto` + `webpki-roots`, RDRAND (on older CPUs without RDRAND, a weaker fallback source from timing jitter) |
| `crates/pgp-lite` | verification of v4 signatures (RSA PKCS#1 v1.5, EdDSA) against the embedded keyring |
| `crates/pkg` | "pacman-lite": db parser, `vercmp`, resolver, tar/pax, `ruzstd`, `miniz_oxide` |
| `crates/ext4w` | mkfs + write-once ext4 writer |
| `crates/disk` | GPT + protective MBR, FAT32 (`fatfs`), a port of `limine bios-install` |
| `crates/initrd` | cpio newc, module dependencies from the ELF `.modinfo`, `.ko.zst` decompression |
| `tiny-init/` | `no_std` static init: `finit_module`, mount root, `switch_root` |
| `examples/config.lua` | a sample config |

## Phases

### 1. Skeleton and build
1. Cargo workspace, `xtask`: Lua → `config.bin`, Limine, `xorriso` + `limine bios-install`, `xtask run` (QEMU SeaBIOS and OVMF).
2. Kernel: heap, IDT for exceptions, TSC calibrated through the PIT, framebuffer + COM1, time from the Limine boot-time request.

### 2. Drivers (depends on 1)
3. `hal` traits.
4. PCI enumeration, virtio-blk/net, AHCI, NVMe, e1000/e1000e, r8169 – all polling, no IRQs.

### 3. Network and verification (depends on 2)
5. `smoltcp` + HTTP/1.1 + TLS (`rustls` no_std).
6. `pgp-lite` + keyring blob.

### 4. Packages and file systems (in parallel with 3)
7. `pkg`: db (gzip/zstd by magic), `%NAME%/%VERSION%/%FILENAME%/%SHA256SUM%/%PGPSIG%/%DEPENDS%/%PROVIDES%/%CONFLICTS%/%GROUPS%`,
   a `vercmp` port from libalpm, version constraints, soname provides, groups, repo priority (core > extra),
   provider choice from the config, conflict checks, tar/pax including `SCHILY.xattr.*`.
   Do not extract `.PKGINFO`, `.MTREE`, `.INSTALL`, `.BUILDINFO`.
8. `ext4w`: extents, filetype, xattr blocks, fast symlinks, hard links; with an internal journal (created together with the file system), `metadata_csum`, `dir_index`.
   Metadata in RAM, data streamed to disk. The biggest risk of the project.
9. `disk`: GPT – BIOS boot 1 MiB, ESP 1 GiB FAT32 (`/boot`), root ext4 for the rest; a `limine bios-install` port.
10. Initramfs: `tiny-init` + modules (ext4, jbd2, mbcache, crc32c, nvme, ahci, libahci, libata, sd_mod, virtio_blk, …)
    with dependencies from `.modinfo` (Arch ships no `modules.dep`).

### 5. Install flow (depends on 2–4)
11. Load `config.bin`, enumerate disks; the selector must hit **exactly one** disk and its serial must match
    `confirm_serial`, otherwise nothing is written (a substitute for a keyboard).
12. DHCP → download `core.db`, `extra.db` → resolve.
13. Partitioning + formatting.
14. For every package: download into `/var/cache/pacman/pkg` on ext4, stream the sha256 + PGP hash → verify → only then extract.
15. `/etc`: `fstab`, `hostname`, `locale.conf`, `vconsole.conf`, `localtime`, `pacman.d/mirrorlist`.
16. Initramfs + Limine on the ESP, a default entry with `systemd.unit=archstaler-firstboot.target` → reboot.

### 6. First boot (in parallel with 5)
17. `archstaler-firstboot.target` + `.service`:
    `pacman-key --init` / `--populate archlinux`,
    `pacman -U --overwrite '*'` from the cache (explicit / `--asdeps`) – runs scriptlets and hooks and writes the local DB,
    `locale-gen`, `useradd` with the password hashes from the config, enable services, `tune2fs -j`, `mkinitcpio -P`,
    `efibootmgr`, rewrite `limine.conf`, reboot.

## Config (`config.lua`)

Disk selector + `confirm_serial`, mirrors, packages, provider choices, hostname, timezone, locale, keymap,
users with `password_hash` (SHA-512 crypt, never plaintext), root hash, services, kernel parameters.

## Keyring and package verification

**Build (`xtask`):** the pinned `archlinux-keyring` (`archlinux.gpg`, `-trusted`, `-revoked`) → `sq`/`gpg` evaluates
the web of trust (≥ 3 signatures from main keys), drops revoked/expired keys → a compact blob
`[fingerprint, algorithm, pubkey (RSA n,e / Ed25519), signing subkeys, expiry]` (~60–100 KB).

**Runtime:** the signature from `%PGPSIG%` in the db (no `.sig` downloads), v4, type 0x00, lookup by issuer fingerprint
(subpacket 33) / key ID (16), the hash is computed while downloading, verify RSA/EdDSA + expiry, then extract.

**Limitations:**
- A new packager after the ISO was built → unknown key → "rebuild the ISO" error (remedy: a regular CI build).
- The root of trust is the build host.
- The Arch dbs are not signed → the integrity of the db relies on HTTPS.
- Safety net: the first-boot `pacman -U` verifies everything again with full GnuPG.

## ISO size

Estimate: ~3.5–5 MB by default, ~1.5–2 MB after optimizations, a minimal variant ~0.7–1 MB.

| Optimization | Saving |
|---|---|
| A compact keyring instead of `archlinux.gpg` | ~1.5 MB |
| HTTP-only (no TLS) / a pinned CA instead of `webpki-roots` | ~1–1.3 MB / ~150 KB |
| Profile: `opt-level="z"`, `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip`, build-std + `panic_immediate_abort` | 30–50 % of the kernel |
| A compressed kernel + a small stub | ~50 % of the rest |
| `tiny-init` as no_std with raw syscalls | ~300 KB → ~10 KB |
| An EFI FAT image with only `BOOTX64.EFI`; Limine binaries as modules, not `include_bytes!` | hundreds of KB |
| xorriso without `-hfsplus`, `-apm-block-size`, `-J`; `-no-pad` | ~300–500 KB |
| A PSF font, no `Debug`/`format!` in errors | ~100–300 KB |

`xtask size` prints the breakdown (`cargo bloat` + the files in the ISO) and fails in CI above a limit (for example 2 MB).

## Verification

1. Host unit tests: `vercmp` (vectors from libalpm), `pgp-lite` (real signatures), the db parser (the current `core.db`).
2. `ext4w`: an image from a tarball → `e2fsck -fn` + comparison with the tar through `debugfs`.
3. `disk`: `sgdisk -v`, `fsck.fat -n`.
4. `xtask test`: QEMU OVMF + SeaBIOS × (virtio-blk | ahci | nvme) × (virtio-net | e1000) → install → boot
   the disk in both modes → the serial log confirms that the first boot finished and `pacman -Qk` is clean.
5. Manually on a real machine with an Intel/Realtek NIC.

## Implementation status (deviations from the plan)

All phases 1–6 are done and verified in QEMU (BIOS and UEFI). Details and usage instructions are in `README.md` and `docs/`.
Differences from the plan above:

- **Network drivers:** on top of the planned e1000/e1000e, r8169 and virtio there are igb, igc, RTL8125/8126 and RTL8139.
  In QEMU, virtio, e1000, e1000e, igb and rtl8139 were run; igc, RTL8168/8169 and RTL8125/8126 never ran (QEMU does not
  emulate them). Wi-Fi, USB Ethernet, `tg3`, `atlantic` and `vmxnet3` are missing.
- **Disk selection:** besides `confirm_serial` there is the optional `disk.auto_largest` (wipes the largest disk without asking).
- **Provider from the config:** an ambiguous dependency is not an error; the first candidate by repository
  priority and name is used (like pacman's default answer) and logged. `providers` in the config overrides it.
- **First boot:** with `-U` pacman keeps files from `backup=()` as `.pacnew`, so the configuration in `/etc`
  is also stored as an overlay and re-applied after `pacman -U`. The initramfs also contains `vfat`/`fat`, because before
  the first `depmod` there is no `modules.dep`. `tiny-init` uses `init_module` instead of `finit_module`.
- **Keyring:** the pin is in `xtask/keyring.pin`; `cargo xtask update-keyring` moves it to the newest package.
- **`.sig` files are not prepared**, so the first-boot `pacman -U` does not verify signatures again (the "safety net"
  point of the keyring section is not met); only the installer verifies signatures.
- **Not implemented:** `xtask test` with the whole disk × NIC × mode matrix (there is `xtask e2e` with options and
  manually run combinations), the clean `pacman -Qk` check, a persistent install log, the size optimizations HTTP-only / pinned CA
  and kernel compression, `cargo bloat` in `xtask size`. The ISO is ~2.4 MB (2.26 MB with `--small`); the 1.5–2 MB goal was not met.
- **iPhone tethering over USB (optional, feature `usb-tethering`, off by default):** `crates/usb` (xHCI,
  verified in QEMU with `usb-storage`) and `crates/imobiledevice` (plist, the usbmux device protocol, pairing through
  lockdownd, the `ipheth` tethering interface as a `NetDevice`; it worked once on a real iPhone, the pairing is not
  stored) and `crates/usbnet` (Android and USB Ethernet adapters: RNDIS, CDC-ECM, CDC-NCM and ASIX AX88179; verified in QEMU with `usb-net` and on one real phone, CDC-NCM is missing).
- **Added on top:** presets (`presets/`), `user_files` and `user_archives` (downloading files/zips into the users' homes),
  `ly` as the display manager, `xtask e2e`, `xtask check-presets`, `xtask linux-test`, tests with Ventoy (1.1.17).
