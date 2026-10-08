# Security policy

archstaller erases disks and installs an operating system from the network, so its trust decisions matter.
Please report problems in them privately.

## Reporting a vulnerability

Use GitHub's private reporting: **Security → Report a vulnerability** on this repository
(`https://github.com/Neekdo12/archstaller/security/advisories/new`). Do not open a public issue for it.

Include what you found, how to reproduce it, and which commit or release is affected. You will get an answer
within a week. This is a small project maintained by two people, so fixes are made on a best-effort basis; a
coordinated disclosure date can be agreed in the report.

## What counts

- A way to make the installer accept an unsigned, modified or downgraded package, database or keyring.
- TLS or certificate validation bypasses in `crates/net`, or the installer talking to a host other than the configured mirrors.
- PGP verification flaws in `crates/pgp-lite` (signature forgery, key handling, the web-of-trust derivation in `xtask`).
- Config values that escape their context in shell-read data files, unit files or the boot loader configuration
  (the build validates them strictly; a bypass is a bug).
- Memory-safety problems in the kernel, the boot loader or the parsers that read network or disk data.
- The first-boot scripts running attacker-controlled content, including the AUR build flow and downloaded scripts.

## What does not

- **Erasing the largest disk without asking.** That is what `disk.auto_largest = true` means and it is documented
  in the README. Use `disk.confirm_serial` for the safe mode.
- The default user `passwd_is_passwd` with the password `passwd` in the presets. It is documented; change it.
- Packages being fetched over HTTPS from a mirror you configured: Arch's sync databases are not signed (a
  property of Arch), so their integrity relies on HTTPS. The packages themselves are verified.
- A packager key that was revoked after an ISO was built: rebuild the ISO regularly. See the trust model in
  `docs/architecture.md`.
- Anything in the out-of-scope list: Wi-Fi, Secure Boot, SMP, other architectures.

## Supported versions

Only the latest release and the `master` branch receive fixes. The project is a proof of concept.
