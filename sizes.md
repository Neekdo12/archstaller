# Velikost ISO

Změřeno 2026-10-02 příkazem `cargo xtask size` (config `examples/config.lua`, profil `release`).
Rozpad kernelu podle crate je z nestripnutého buildu (`CARGO_PROFILE_RELEASE_STRIP=false`) přes `llvm-nm --print-size`.

**Celkem: 798 720 B (0,76 MiB).** S `--small`: 739 328 B (0,71 MiB). Před vlastním boot loaderem (Limine) to bylo
2 449 408 B (2,34 MiB), s `--super-small` 1 470 464 B (1,40 MiB). Kernel bez `--tethering`; tethering přidává asi 105 KiB.

## Co je na ISO

ISO má dva soubory. Payload (`payload.bin`) leží jednou, uvnitř FAT12 image `efi.img`; BIOS stage 1 a 2 ho čtou
přímo z ISO podle offsetu, který `xtask` do nich vyplní po sestavení ISO.

| Soubor | Velikost | Kdo to čte | K čemu |
|---|---:|---|---|
| `boot/efi.img` | 657 KiB | UEFI firmware, BIOS stage 2 | FAT12: `EFI/BOOT/BOOTX64.EFI` (23 KiB) + `payload.bin` |
| `boot/bios.img` | 21 KiB | BIOS (El Torito, hybridní MBR) | stage 1 (512 B, doplněno na 2048) + stage 2 (19 KiB) |
| režie ISO9660 / hybrid | 103 KiB | – | system area (32 KiB), deskriptory, Rock Ridge, El Torito katalog, GPT + MBR, zarovnání |

Obsah `payload.bin` (každá položka zvlášť deflate, jen když se zmenší):

| Položka | Před | Po | K čemu |
|---|---:|---:|---|
| `kernel` | 1058 KiB | 533 KiB | celý installer (drivery, síť, TLS, pacman-lite, ext4, GPT/FAT) |
| `keyring.bin` | 32 KiB | 32 KiB | veřejné klíče packagerů pro ověření PGP podpisů |
| `bios-boot` | 19,5 KiB | 12,4 KiB | stage 1 + 2 pro BIOS boot **cílového** disku, kernel je zapíše do MBR a BIOS boot partition |
| `bootx64.efi` | 23 KiB | 13,3 KiB | UEFI loader pro první boot cílového systému (kopíruje se na cílovou ESP) |
| `tiny-init` | 37 KiB | 10 KiB | `/init` do initramfs cílového systému |
| `config.bin` | 0,4 KiB | 0,3 KiB | vyhodnocený `config.lua` |

Loader (UEFI 23 KiB, BIOS 19 KiB) je dohromady zhruba 4 % ISO, proti 49 % u Limine.

## Co je v kernelu

Sekce: `.text` 811 KiB, `.rodata` 135 KiB, `.data` 51 KiB (`.bss` 17 KiB se do souboru neukládá).

Kód podle crate (`.text`, KiB):

| Skupina | Crate | KiB | Součet |
|---|---|---:|---:|
| **TLS** | `rustls` | 164 | **~332** |
| | `rustls_rustcrypto` | 39 | |
| | `webpki` | 34 | |
| | `p384`, `primeorder`, `ecdsa`, `p256`, `elliptic_curve` | 53 | |
| | `der`, `pkcs8`, `sec1`, `rustls_pki_types` | 28 | |
| | `aes`, `aes_gcm`, `ctr`, `cipher` | 14 | |
| **Sdílená kryptografie** (TLS + PGP) | `num_bigint_dig`, `rsa` | 36 | **~63** |
| | `sha2` | 14 | |
| | `curve25519_dalek`, `ed25519_dalek` | 15 | |
| **Runtime** | `core`, `alloc` | 99 | **~139** |
| | bez jména (compiler_builtins, closures, …) | 40 | |
| **Vlastní kód** | `kernel` (z toho `install::run` 60, `_start` 22) | 103 | **~186** |
| | `net` (HTTP klient, stack glue) | 32 | |
| | `ext4w` | 19 | |
| | `pkg` | 18 | |
| | `disk`, `drivers`, `initrd`, `pgp_lite` | 22 | |
| **Knihovny** | `smoltcp` | 28 | **~59** |
| | `ruzstd` | 20 | |
| | `miniz_oxide` | 11 | |

Data: kořenové certifikáty `webpki-roots` (121 CA) mají zhruba **53 KiB** DER dat. Zbytek `.rodata`/`.data` tvoří
vtables, řetězce hlášek, PSF font (5 KiB), firstboot soubory (4,4 KiB), CRC tabulky (2 KiB) a unicode tabulky z `core` (~3 KiB).

**TLS dohromady (kód + kořeny) ≈ 385 KiB, tedy ~38 % kernelu.**

## Jak to zmenšit

Seřazeno podle poměru úspory a rizika. Úspory jsou odhady, pokud není uvedeno „změřeno“. Zbývá hlavně kernel (~70 % ISO).

### 1. Zeštíhlit TLS, HTTPS zůstane: −100 až −150 KiB
- **Vlastní `CryptoProvider` bez `KeyProvider`**: klient nikdy nenačítá privátní klíč, ale `rustls_rustcrypto::provider()`
  s sebou táhne parsování PKCS#8, `RsaPrivateKey` a ECDSA signery (`load_private_key` 5 KiB, signery P-256/P-384 9 KiB,
  `pkcs8`, `sec1`, …). Úspora ~30–40 KiB.
- **Jen TLS 1.3** (bez feature `tls12` v `rustls` a `rustls-rustcrypto`): zmizí stavový automat TLS 1.2
  (`ExpectServerDone`, …). Úspora ~30–50 KiB. Riziko: mirror jen s TLS 1.2. `xtask` může při buildu ověřit,
  že všechny mirrory v configu i URL v `user_files`/`user_archives` umí TLS 1.3.
- **Kořenové CA jen pro hosty z configu**: `xtask` při buildu zjistí řetězy mirrorů a URL a vloží jen potřebné kořeny
  (typicky ISRG Root X1/X2). Úspora ~50 KiB `.rodata`. Riziko: když host změní CA, instalace selže až do přebuildu ISO.
  Plná sada může zůstat jako volba.
- **Méně cipher suites a skupin** (např. jen `TLS13_AES_128_GCM_SHA256` + X25519, podpisy ECDSA P-256/P-384, RSA-PSS,
  RSA PKCS#1): ~5–15 KiB. P-384 zůstat musí, ISRG Root X2 je P-384.

### Hotovo
- **Vlastní boot loader místo Limine** (−1157 KiB, 49 % ISO), **komprimovaný payload** a **payload jen jednou** v `efi.img`.
- **`--small`**: `build-std` + `panic = "immediate-abort"`: kernel 1058 → 913 KiB (−59 KiB ISO po kompresi). Nevýhoda: panic
  skončí bez hlášky, takže ladění na reálném HW je těžší. Doporučuji pro `cargo xtask presets`.

### 2. `tiny-init` bez zarovnání: ~−20 KiB
Obsah je 16,4 KiB (`.text` 13,7 + `.rodata` 1,9 + `.data` 0,8). Zbytek do 37 KiB tvoří padding z `ALIGN(4K)`
v `tiny-init/linker.ld` a zarovnání segmentů. Zlepší to méně PT_LOAD segmentů nebo `-z max-page-size=4096` a
`-z separate-code=no`. Soubor se po zstd zmenší na 9,5 KiB, což padding potvrzuje.

### 3. Méně `Debug`/`format!` v kernelu: ~−10 až −30 KiB (nejisté)
`println!("... {e:?}")` táhne `Debug` implementace chybových typů z `rustls`, `smoltcp` a dalších crate. Chybové kódy
nebo `&'static str` místo `{:?}` ušetří část `core::fmt` a řetězců. Přesný dopad ukáže až měření.

### Co nedoporučuji
- **HTTP bez TLS** (−~385 KiB): integritu sync databází drží jen HTTPS (Arch db nejsou podepsané) a `user_files`
  a `user_archives` (např. Hyprland config) nejsou ověřeny ničím jiným. Bez TLS by šlo podvrhnout starou db
  a hlavně spustitelný obsah v home adresáři.
- **Menší keyring**: 32 KiB jsou už jen veřejné klíče a náhodná data se nekomprimují.
- **Jednoplatformní ISO**: odpadne jen ~20 KiB (loader je malý), nestojí to za ztrátu BIOS nebo UEFI.
