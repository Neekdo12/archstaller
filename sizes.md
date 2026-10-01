# Velikost ISO

Změřeno 2026-10-01 příkazem `cargo xtask size` (config `examples/config.lua`, profil `release`).
Rozpad kernelu podle crate je z nestripnutého buildu (`CARGO_PROFILE_RELEASE_STRIP=false`) přes `llvm-nm --print-size`.

**Celkem: 2 449 408 B (2,34 MiB).** S `--small`: 2 306 048 B (2,20 MiB). Kernel bez `--tethering`; tethering přidává asi 105 KiB.

## `--super-small`: změřeno

`cargo xtask size --super-small` dává **1 470 464 B (1,40 MiB)** (proti 2 306 048 B u `--small`). Kernel je deflate (`miniz_oxide`, úroveň 10) 936 400 → 487 890 B (52 %, zstd by dal asi 46 %, ale loader pak nepotřebuje alokátor), loader `kstub` má 21 KiB, `BOOTX64.EFI` je jen v `efi.img` (rezerva obrazu 20 KiB místo 48 KiB). Zbytek je Limine (1 130 KiB). Neřešeno: `tiny-init` padding (~20 KiB), TLS dieta (body 2, 5 níže).

## Co je na ISO

| Soubor | Velikost | % ISO | Kdo to čte | K čemu |
|---|---:|---:|---|---|
| `boot/kernel` | 1058 KiB | 43 % | Limine | celý installer (drivery, síť, TLS, pacman-lite, ext4, GPT/FAT) |
| `boot/limine/efi.img` | 416 KiB | 18 % | UEFI firmware | FAT12 image pro El Torito UEFI boot, obsahuje jen `BOOTX64.EFI` |
| `EFI/BOOT/BOOTX64.EFI` | 368 KiB | 16 % | kernel (modul) | Limine pro UEFI boot **nainstalovaného** systému; kopíruje se na cílovou ESP |
| `boot/limine/limine-bios.sys` | 324 KiB | 14 % | Limine BIOS + kernel (modul) | stage 2 Limine: bootování ISO v BIOSu i cílového systému v BIOSu |
| `boot/tiny-init` | 37 KiB | 1,6 % | kernel (modul) | `/init` do initramfs cílového systému |
| `boot/keyring.bin` | 32 KiB | 1,4 % | kernel (modul) | veřejné klíče packagerů pro ověření PGP podpisů |
| `boot/limine/limine-bios-cd.bin` | 26 KiB | 1,1 % | BIOS | El Torito boot sektor pro BIOS |
| `boot/limine-bios-hdd.bin` | 23 KiB | 1,0 % | kernel (modul) | MBR stage 1 pro BIOS boot cílového disku (`bios-install`) |
| `boot/config.bin` | 0,4 KiB | – | kernel (modul) | vyhodnocený `config.lua` |
| `boot/limine/limine.conf` | 0,3 KiB | – | Limine | konfigurace bootu ISO |
| režie ISO9660 / hybrid | 107 KiB | 4,6 % | – | system area (32 KiB), deskriptory, Rock Ridge, El Torito katalog, GPT + MBR, zarovnání na 2 KiB |

Limine (efi.img + BOOTX64.EFI + bios.sys + cd.bin + hdd.bin) dohromady zabírá **1157 KiB, tedy 49 % ISO**. To je víc než samotný kernel.

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

Seřazeno podle poměru úspory a rizika. Úspory jsou odhady, pokud není uvedeno „změřeno“.

### 1. Neukládat `BOOTX64.EFI` dvakrát (hotovo v `--super-small`): −368 KiB (−15 %)
Stejný soubor je na ISO dvakrát: jednou uvnitř `efi.img` (pro firmware), jednou volně (jako modul pro kernel).
Kernel si ho může vzít z `efi.img`: dostane `efi.img` jako modul a přečte `EFI/BOOT/BOOTX64.EFI` z FAT12. Stačí
minimální čtečka root/podadresáře a FAT řetězu v `crates/disk`, nebo xtask při buildu zapíše offset a délku souboru
v image do `config.bin`, protože `mcopy` do čerstvého image zapisuje souvisle. Bez rizika a bez nové závislosti.

Stahovat `BOOTX64.EFI` z balíčku `limine` z mirroru nedoporučuji: verze by se lišila od `limine-bios.sys`
a `limine-bios-hdd.bin` z ISO, které spolu musí verzí sedět.

### 2. Zeštíhlit TLS, HTTPS zůstane: −100 až −150 KiB
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

### 3. `--small` jako výchozí pro release ISO (presety): −133 KiB (změřeno)
`build-std` + `panic = "immediate-abort"`: kernel 1058 → 919 KiB. Nevýhoda: panic skončí bez hlášky, takže ladění na
reálném HW je těžší. Doporučuji pro `cargo xtask presets`, pro vývoj a `run`/`e2e` nechat `release`.

### 4. Zmenšit rezervu v `efi.img`: ~−35 KiB
`make_efi_image` přidává 96 sektorů (48 KiB) navíc k velikosti `BOOTX64.EFI`. FAT12 potřebuje boot sektor, 2 FAT
a root directory, což je ~10–15 KiB. Stačí zmenšit rezervu a omezit počet root entries (`mformat -r`).

### 5. `tiny-init` bez zarovnání: ~−20 KiB
Obsah je 16,4 KiB (`.text` 13,7 + `.rodata` 1,9 + `.data` 0,8). Zbytek do 37 KiB tvoří padding z `ALIGN(4K)`
v `tiny-init/linker.ld` a zarovnání segmentů. Zlepší to méně PT_LOAD segmentů nebo `-z max-page-size=4096` a
`-z separate-code=no`. Soubor se po zstd zmenší na 9,5 KiB, což padding potvrzuje.

### 6. Méně `Debug`/`format!` v kernelu: ~−10 až −30 KiB (nejisté)
`println!("... {e:?}")` táhne `Debug` implementace chybových typů z `rustls`, `smoltcp` a dalších crate. Chybové kódy
nebo `&'static str` místo `{:?}` ušetří část `core::fmt` a řetězců. Přesný dopad ukáže až měření.

### 7. Komprimovaný kernel + stub: ~−430 KiB (hotovo v `--super-small`)
Kernel se přes zstd -19 zmenší na ~46 %. Limine kernel nedekomprimuje, takže by Limine načetl malý stub a ten by
rozbalil skutečný kernel (dekodér `ruzstd` ~20 KiB je už v projektu), namapoval ho a skočil do něj. To znamená
vlastní ELF loader a stránkování ve stubu, tedy nejvíc práce ze všech bodů. Limine soubory (`efi.img`, `BOOTX64.EFI`,
`limine-bios.sys`) takhle zmenšit nejdou, protože je firmware a Limine čtou přímo. `keyring.bin` a `limine-bios-hdd.bin`
se téměř nekomprimují.

### 8. Varianty ISO jen pro jednu platformu
- **Jen UEFI**: odpadne `limine-bios.sys`, `limine-bios-cd.bin` a `limine-bios-hdd.bin`, tedy **−373 KiB**. Cíl pak jde jen UEFI.
- **Jen BIOS**: odpadne `efi.img` a `BOOTX64.EFI`, tedy −784 KiB (po bodu 1 −416 KiB). Cíl pak jde jen BIOS.

Jde proti původnímu zadání (BIOS + UEFI), mohlo by to ale být jako volba `--platform`.

### Co nedoporučuji
- **HTTP bez TLS** (−~385 KiB): integritu sync databází drží jen HTTPS (Arch db nejsou podepsané) a `user_files`
  a `user_archives` (např. Hyprland config) nejsou ověřeny ničím jiným. Bez TLS by šlo podvrhnout starou db
  a hlavně spustitelný obsah v home adresáři.
- **Vypustit Rock Ridge (`-R -r`)**: úspora v řádu jednotek KiB a riziko, že Limine nenajde soubory podle jmen.
- **Menší keyring**: 32 KiB jsou už jen veřejné klíče a náhodná data se nekomprimují.

## Odhad po úpravách

| Krok | ISO |
|---|---:|
| dnes (`release`) | 2350 KiB |
| + bod 1 (bez duplicity `BOOTX64.EFI`) | ~1980 KiB |
| + body 2, 4, 5 (TLS dieta, `efi.img`, `tiny-init`) | ~1800 KiB |
| + bod 3 (`--small`) | ~1680 KiB |
| + bod 7 (komprimovaný kernel) | ~1250 KiB |

Spodní hranici pro BIOS + UEFI tvoří Limine (~790 KiB po bodech 1 a 4) a režie ISO (~100 KiB). Všechno nad tím je kernel.
