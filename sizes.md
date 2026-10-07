# ISO size

Measured on 2026-10-02 with `cargo xtask size` (config `configs/config.lua`, profile `release`).
The kernel breakdown by crate is from an unstripped build (`CARGO_PROFILE_RELEASE_STRIP=false`) through `llvm-nm --print-size`.

**Total: 798,720 B (0.76 MiB).** With `--small`: 739,328 B (0.71 MiB). Before our own boot loader (Limine) it was
2,449,408 B (2.34 MiB), and 1,470,464 B (1.40 MiB) with `--super-small`. The kernel is without `--tethering`; tethering adds about 105 KiB.

## What is on the ISO

The ISO has two files. The payload (`payload.bin`) lies once, inside the FAT12 image `efi.img`; BIOS stage 1 and 2 read it
directly from the ISO at an offset that `xtask` fills in after the ISO is built.

| File | Size | Read by | Purpose |
|---|---:|---|---|
| `boot/efi.img` | 657 KiB | UEFI firmware, BIOS stage 2 | FAT12: `EFI/BOOT/BOOTX64.EFI` (23 KiB) + `payload.bin` |
| `boot/bios.img` | 21 KiB | BIOS (El Torito, hybrid MBR) | stage 1 (512 B, padded to 2048) + stage 2 (19 KiB) |
| ISO9660 / hybrid overhead | 103 KiB | – | system area (32 KiB), descriptors, Rock Ridge, the El Torito catalog, GPT + MBR, alignment |

Contents of `payload.bin` (each entry is deflated separately, only when it gets smaller):

| Entry | Before | After | Purpose |
|---|---:|---:|---|
| `kernel` | 1058 KiB | 533 KiB | the whole installer (drivers, network, TLS, pacman-lite, ext4, GPT/FAT) |
| `keyring.bin` | 32 KiB | 32 KiB | the packagers' public keys for verifying PGP signatures |
| `bios-boot` | 19.5 KiB | 12.4 KiB | stage 1 + 2 for the BIOS boot of the **target** disk; the kernel writes them to the MBR and the BIOS boot partition |
| `bootx64.efi` | 23 KiB | 13.3 KiB | the UEFI loader for the first boot of the target system (copied to the target ESP) |
| `tiny-init` | 37 KiB | 10 KiB | `/init` for the target system's initramfs |
| `config.bin` | 0.4 KiB | 0.3 KiB | the evaluated `config.lua` |

The loader (UEFI 23 KiB, BIOS 19 KiB) is roughly 4 % of the ISO together, against 49 % with Limine.

## What is in the kernel

Sections: `.text` 811 KiB, `.rodata` 135 KiB, `.data` 51 KiB (`.bss`, 17 KiB, is not stored in the file).

Code by crate (`.text`, KiB):

| Group | Crate | KiB | Sum |
|---|---|---:|---:|
| **TLS** | `rustls` | 164 | **~332** |
| | `rustls_rustcrypto` | 39 | |
| | `webpki` | 34 | |
| | `p384`, `primeorder`, `ecdsa`, `p256`, `elliptic_curve` | 53 | |
| | `der`, `pkcs8`, `sec1`, `rustls_pki_types` | 28 | |
| | `aes`, `aes_gcm`, `ctr`, `cipher` | 14 | |
| **Shared cryptography** (TLS + PGP) | `num_bigint_dig`, `rsa` | 36 | **~63** |
| | `sha2` | 14 | |
| | `curve25519_dalek`, `ed25519_dalek` | 15 | |
| **Runtime** | `core`, `alloc` | 99 | **~139** |
| | unnamed (compiler_builtins, closures, …) | 40 | |
| **Own code** | `kernel` (of which `install::run` 60, `_start` 22) | 103 | **~186** |
| | `net` (HTTP client, stack glue) | 32 | |
| | `ext4w` | 19 | |
| | `pkg` | 18 | |
| | `disk`, `drivers`, `initrd`, `pgp_lite` | 22 | |
| **Libraries** | `smoltcp` | 28 | **~59** |
| | `ruzstd` | 20 | |
| | `miniz_oxide` | 11 | |

Data: the `webpki-roots` root certificates (121 CAs) are about **53 KiB** of DER data. The rest of `.rodata`/`.data` is
vtables, message strings, the PSF font (5 KiB), the firstboot files (4.4 KiB), CRC tables (2 KiB) and the unicode tables from `core` (~3 KiB).

**TLS in total (code + roots) ≈ 385 KiB, that is ~38 % of the kernel.**

## How to make it smaller

Sorted by the ratio of saving to risk. Savings are estimates unless marked "measured". What remains is mainly the kernel (~70 % of the ISO).

### 1. Slim down TLS, HTTPS stays: −100 to −150 KiB
- **A custom `CryptoProvider` without `KeyProvider`**: the client never loads a private key, but `rustls_rustcrypto::provider()`
  drags in PKCS#8 parsing, `RsaPrivateKey` and the ECDSA signers (`load_private_key` 5 KiB, the P-256/P-384 signers 9 KiB,
  `pkcs8`, `sec1`, …). Saving ~30–40 KiB.
- **TLS 1.3 only** (without the `tls12` feature in `rustls` and `rustls-rustcrypto`): the TLS 1.2 state machine disappears
  (`ExpectServerDone`, …). Saving ~30–50 KiB. Risk: a mirror with TLS 1.2 only. At build time `xtask` can verify
  that all mirrors in the config and the URLs in `user_files`/`user_archives` support TLS 1.3.
- **Root CAs only for the hosts in the config**: at build time `xtask` finds out the chains of the mirrors and URLs and embeds only the needed roots
  (typically ISRG Root X1/X2). Saving ~50 KiB of `.rodata`. Risk: if a host changes its CA, the install fails until the ISO is rebuilt.
  The full set can stay as an option.
- **Fewer cipher suites and groups** (for example only `TLS13_AES_128_GCM_SHA256` + X25519, ECDSA P-256/P-384 signatures, RSA-PSS,
  RSA PKCS#1): ~5–15 KiB. P-384 must stay, since ISRG Root X2 is P-384.

### Done
- **Our own boot loader instead of Limine** (−1157 KiB, 49 % of the ISO), a **compressed payload** and the **payload only once** in `efi.img`.
- **`--small`**: `build-std` + `panic = "immediate-abort"`: kernel 1058 → 913 KiB (−59 KiB of ISO after compression). Downside: a panic
  ends without a message, so debugging on real hardware is harder. Recommended for `cargo xtask presets`.

### 2. `tiny-init` without padding: ~−20 KiB
The content is 16.4 KiB (`.text` 13.7 + `.rodata` 1.9 + `.data` 0.8). The rest up to 37 KiB is padding from `ALIGN(4K)`
in `tiny-init/linker.ld` and segment alignment. Fewer PT_LOAD segments or `-z max-page-size=4096` and
`-z separate-code=no` would help. After zstd the file shrinks to 9.5 KiB, which confirms the padding.

### 3. Less `Debug`/`format!` in the kernel: ~−10 to −30 KiB (uncertain)
`println!("... {e:?}")` pulls in the `Debug` implementations of the error types from `rustls`, `smoltcp` and other crates. Error codes
or `&'static str` instead of `{:?}` save part of `core::fmt` and the strings. The exact effect will only show up in a measurement.

### What I do not recommend
- **HTTP without TLS** (−~385 KiB): the integrity of the sync databases relies on HTTPS alone (the Arch dbs are not signed), and `user_files`
  and `user_archives` (for example the Hyprland config) are verified by nothing else. Without TLS a stale db could be forged,
  and above all executable content in the home directory.
- **A smaller keyring**: the 32 KiB is already only public keys, and random data does not compress.
- **A single-platform ISO**: it saves only ~20 KiB (the loader is small), not worth losing BIOS or UEFI.
