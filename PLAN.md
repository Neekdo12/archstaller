# archstaler – instalátor Archu v Rustu bez Linux jádra

Hybridní ISO (BIOS + UEFI) s Limine, které spustí vlastní `no_std` mini-kernel v Rustu (1 jádro, polling drivery).
Ten nainstaluje Arch z mirroru na ext4 podle předem připraveného Lua configu. Věci vyžadující běžící Linux
(pacman scriptlety, alpm hooky, mkinitcpio) se odloží na první boot nainstalovaného systému.

## Rozhodnutí

- Balíčky: online z mirroru (oficiální Arch ISO se nepoužívá)
- Platforma: x86_64, BIOS + UEFI přes Limine, vlastní drivery
- Config: `config.lua` vyhodnocen při buildu ISO (mlua na hostu) → `config.bin` (postcard) jako Limine modul
- Root FS: ext4
- Hooky/scriptlety: při prvním bootu (`pacman -U` z cache)
- Secure Boot: nepodporován (ISO ani cíl) – musí být vypnutý
- Mimo rozsah: Wi-Fi, USB kromě tetheringu iPhonu (viz níže), SMP, Secure Boot, interaktivní TUI, jiné FS než ext4, jiné architektury

## Struktura workspace

| Cesta | Popis |
|---|---|
| `xtask/` | host: eval Lua, build kernelu + tiny-init, keyring + CA blob, Limine (pinned + sha256), xorriso, QEMU run/test, size report |
| `kernel/` | `x86_64-unknown-none`, crate `limine`, heap (`talc`), IDT jen výjimky, TSC timer, framebuffer + serial konzole |
| `config/` | sdílené serde typy configu (xtask + kernel) |
| `crates/hal` | traity `BlockDevice`, `NetDevice`, `Clock`, `Rng` |
| `crates/drivers` | PCI, virtio-blk/net, AHCI, NVMe, e1000/e1000e, r8169 (polling) |
| `crates/net` | `smoltcp` (DHCP/DNS/TCP), HTTP/1.1 klient, `rustls` no_std + `rustls-rustcrypto` + `webpki-roots`, RDRAND |
| `crates/pgp-lite` | ověření v4 podpisů (RSA PKCS#1 v1.5, EdDSA) proti vloženému keyringu |
| `crates/pkg` | „pacman-lite“: parser db, `vercmp`, resolver, tar/pax, `ruzstd`, `miniz_oxide` |
| `crates/ext4w` | mkfs + write-once ext4 writer |
| `crates/disk` | GPT + protective MBR, FAT32 (`fatfs`), port `limine bios-install` |
| `crates/initrd` | cpio newc, závislosti modulů z ELF `.modinfo`, dekomprese `.ko.zst` |
| `tiny-init/` | `no_std` statický init: `finit_module`, mount root, `switch_root` |
| `examples/config.lua` | ukázkový config |

## Fáze

### 1. Kostra a build
1. Cargo workspace, `xtask`: Lua → `config.bin`, Limine, `xorriso` + `limine bios-install`, `xtask run` (QEMU SeaBIOS i OVMF).
2. Kernel: heap, IDT pro výjimky, TSC kalibrovaný přes PIT, framebuffer + COM1, čas z Limine boot-time requestu.

### 2. Drivery (závisí na 1)
3. `hal` traity.
4. PCI enumerace, virtio-blk/net, AHCI, NVMe, e1000/e1000e, r8169 – vše polling bez IRQ.

### 3. Síť a ověřování (závisí na 2)
5. `smoltcp` + HTTP/1.1 + TLS (`rustls` no_std).
6. `pgp-lite` + keyring blob.

### 4. Balíčky a FS (paralelně s 3)
7. `pkg`: db (gzip/zstd dle magic), `%NAME%/%VERSION%/%FILENAME%/%SHA256SUM%/%PGPSIG%/%DEPENDS%/%PROVIDES%/%CONFLICTS%/%GROUPS%`,
   `vercmp` port z libalpm, verzové constrainty, soname provides, skupiny, priorita repo (core > extra),
   výběr providera z configu, kontrola konfliktů, tar/pax včetně `SCHILY.xattr.*`.
   Neextrahovat `.PKGINFO`, `.MTREE`, `.INSTALL`, `.BUILDINFO`.
8. `ext4w`: extents, filetype, xattr bloky, fast symlinky, hardlinky; bez journalu, `metadata_csum`, `dir_index`.
   Metadata v RAM, data streamem na disk. Největší riziko projektu.
9. `disk`: GPT – BIOS boot 1 MiB, ESP 1 GiB FAT32 (`/boot`), root ext4 zbytek; `limine bios-install` port.
10. Initramfs: `tiny-init` + moduly (ext4, jbd2, mbcache, crc32c, nvme, ahci, libahci, libata, sd_mod, virtio_blk, …)
    se závislostmi z `.modinfo` (Arch nedodává `modules.dep`).

### 5. Instalační flow (závisí na 2–4)
11. Načíst `config.bin`, enumerovat disky; selektor musí trefit **přesně jeden** disk a jeho serial musí sedět
    s `confirm_serial`, jinak se nic nezapíše (náhrada za klávesnici).
12. DHCP → stáhnout `core.db`, `extra.db` → resolve.
13. Partitioning + formát.
14. Každý balíček: stáhnout do `/var/cache/pacman/pkg` na ext4, streamově sha256 + PGP hash → ověřit → teprve pak rozbalit.
15. `/etc`: `fstab`, `hostname`, `locale.conf`, `vconsole.conf`, `localtime`, `pacman.d/mirrorlist`.
16. Initramfs + Limine na ESP, default entry s `systemd.unit=archstaler-firstboot.target` → reboot.

### 6. První boot (paralelně s 5)
17. `archstaler-firstboot.target` + `.service`:
    `pacman-key --init` / `--populate archlinux`,
    `pacman -U --overwrite '*'` z cache (explicitní / `--asdeps`) – spustí scriptlety, hooky a zapíše local DB,
    `locale-gen`, `useradd` s hashi hesel z configu, enable služeb, `tune2fs -j`, `mkinitcpio -P`,
    `efibootmgr`, přepsat `limine.conf`, reboot.

## Config (`config.lua`)

Disk selector + `confirm_serial`, mirrory, balíčky, výběr providerů, hostname, timezone, locale, keymap,
uživatelé s `password_hash` (SHA-512 crypt, nikdy plaintext), root hash, služby, kernel parametry.

## Keyring a ověřování balíčků

**Build (`xtask`):** pinned `archlinux-keyring` (`archlinux.gpg`, `-trusted`, `-revoked`) → `sq`/`gpg` vyhodnotí
web of trust (≥ 3 podpisy od main keys), vyřadí revoked/expired → kompaktní blob
`[fingerprint, algoritmus, pubkey (RSA n,e / Ed25519), signing subklíče, expirace]` (~60–100 KB).

**Runtime:** podpis z `%PGPSIG%` v db (žádné `.sig` stahování), v4, typ 0x00, lookup přes issuer fingerprint
(subpacket 33) / key ID (16), hash streamově při stahování, verify RSA/EdDSA + expirace, pak extrakce.

**Omezení:**
- Nový packager po buildu ISO → neznámý klíč → chyba „přebuildit ISO“ (řešení: pravidelný build v CI).
- Kořen důvěry je build host.
- Arch db nejsou podepsané → integritu db drží HTTPS.
- Pojistka: first-boot `pacman -U` znovu ověří vše plným GnuPG.

## Velikost ISO

Odhad: výchozí ~3,5–5 MB, po optimalizacích ~1,5–2 MB, minimální varianta ~0,7–1 MB.

| Optimalizace | Úspora |
|---|---|
| Kompaktní keyring místo `archlinux.gpg` | ~1,5 MB |
| HTTP-only (bez TLS) / pinned CA místo `webpki-roots` | ~1–1,3 MB / ~150 KB |
| Profil: `opt-level="z"`, `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip`, build-std + `panic_immediate_abort` | 30–50 % kernelu |
| Komprimovaný kernel + malý stub | ~50 % zbytku |
| `tiny-init` no_std s raw syscally | ~300 KB → ~10 KB |
| EFI FAT image jen s `BOOTX64.EFI`; Limine binárky jako moduly, ne `include_bytes!` | stovky KB |
| xorriso bez `-hfsplus`, `-apm-block-size`, `-J`; `-no-pad` | ~300–500 KB |
| PSF font, bez `Debug`/`format!` v chybách | ~100–300 KB |

`xtask size` vypíše rozpad (`cargo bloat` + soubory v ISO) a v CI selže nad limitem (např. 2 MB).

## Ověření

1. Host unit testy: `vercmp` (vektory z libalpm), `pgp-lite` (reálné podpisy), db parser (aktuální `core.db`).
2. `ext4w`: image z tarballu → `e2fsck -fn` + porovnání s tarem přes `debugfs`.
3. `disk`: `sgdisk -v`, `fsck.fat -n`.
4. `xtask test`: QEMU OVMF + SeaBIOS × (virtio-blk | ahci | nvme) × (virtio-net | e1000) → instalace → boot
   disku oběma režimy → serial log potvrdí dokončení firstbootu a čisté `pacman -Qk`.
5. Manuálně na reálném stroji s Intel/Realtek NIC.

## Stav implementace (odchylky od plánu)

Všechny fáze 1–6 jsou hotové a ověřené v QEMU (BIOS i UEFI). Podrobnosti a pokyny k použití jsou v `README.md`.
Rozdíly oproti plánu výše:

- **Ovladače sítě:** k plánovaným e1000/e1000e, r8169 a virtio přibyly igb, igc, RTL8125/8126 a RTL8139. V QEMU
  proběhly virtio, e1000, e1000e, igb a rtl8139; igc, RTL8168/8169 a RTL8125/8126 nikdy neběžely (QEMU je
  neemuluje). Wi-Fi, USB Ethernet, `tg3`, `atlantic` a `vmxnet3` chybí.
- **Výběr disku:** kromě `confirm_serial` je volitelné `disk.auto_largest` (smaže největší disk bez ptaní).
- **Provider z configu:** nejednoznačná závislost se nevyhodí jako chyba; použije se první kandidát podle priority
  repozitáře a jména (jako výchozí odpověď pacmanu) a zaloguje se. `providers` v configu ji přepíše.
- **První boot:** pacman při `-U` ponechá soubory z `backup=()` jako `.pacnew`, proto se konfigurace z `/etc`
  ukládá i jako overlay a po `pacman -U` se znovu aplikuje. Initramfs obsahuje také `vfat`/`fat`, protože před
  prvním `depmod` neexistuje `modules.dep`. `tiny-init` používá `init_module` místo `finit_module`.
- **Keyring:** pin je v `xtask/keyring.pin`, `cargo xtask update-keyring` ho posune na nejnovější balíček.
- **`.sig` soubory se nepřipravují**, takže `pacman -U` při prvním bootu podpisy znovu neověřuje (bod „pojistka“
  z části o keyringu není splněn); podpisy ověřuje jen instalátor.
- **Neimplementováno:** `xtask test` s celou maticí disk × NIC × režim (je `xtask e2e` s volbami a ručně spuštěné
  kombinace), kontrola čistého `pacman -Qk`, trvalý log instalace, velikostní optimalizace HTTP-only / pinned CA
  a komprese kernelu, `cargo bloat` v `xtask size`. ISO má ~2,4 MB (2,26 MB s `--small`), cíl 1,5–2 MB nesplněn.
- **Tethering iPhonu přes USB (volitelný, feature `usb-tethering`, ve výchozím stavu vypnutý):** `crates/usb` (xHCI,
  ověřeno v QEMU s `usb-storage`) a `crates/imobiledevice` (plist, usbmux protokol zařízení, párování přes
  lockdownd, tethering rozhraní `ipheth` jako `NetDevice`). Na skutečném iPhonu nikdy plně neproběhl; párování
  se neukládá.
- **Přidáno navíc:** presety (`presets/`), `user_files` a `user_archives` (stažení souborů/zip do domovů uživatelů),
  `ly` jako display manager, `xtask e2e`, `xtask check-presets`, `xtask linux-test`, testy se Ventoy (1.1.17).
