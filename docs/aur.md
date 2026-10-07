# Implementation plan: AUR packages

Status: implemented. `aur_packages` in the config pins reviewed AUR recipes; `crates/aurbuild` searches,
reviews and plans on the host (used by `cargo xtask aur-pin` and the GUI's AUR group); the installer adds the
official packages the recipes need and writes `aur.list`; `firstboot/archstaler-aur.service` and
`archstaler-aur.sh` build and install the packages on the boot after the first one. Checked by unit tests
(`cargo test -p aurbuild`, including a run of the real shell script against local git repositories with
stand-ins for `pacman`, `makepkg` and the other root-only commands) and by an end-to-end QEMU install
(`cargo xtask e2e --config configs/e2e-aur.lua`). Differences from the first draft of this plan are listed
under "As implemented". This document replaces the short AUR notes in `docs/implement-gui.md` (phase 6).

## Goal

Let a config name packages from the Arch User Repository (for example `zen-browser-bin` or a cursor theme)
and have them installed on the target system, with nothing AUR-related shipped inside the ISO. The ISO only
carries a small pinned list; the target machine downloads the package recipes, builds them and installs
them.

Non-goals: an AUR helper left on the installed system, updating AUR packages after install, building
anything inside the `no_std` installer, trusting an AUR package any more than the user explicitly agreed to.

## Decisions

Taken by the project owner:

1. **Nothing is shipped in the ISO.** The AUR packages are built on the target system, from the recipe
   fetched there. The ISO stays the size it is today plus one short list.
2. **Build on the target**, as an unprivileged throwaway user, in a first-boot service that runs once the
   network is up (the installer itself has no Linux and cannot run `makepkg`).
3. **VCS (`-git`) packages are allowed**, with a warning and a per-package acknowledgement.
4. **No ISO size cap**, because nothing is embedded; the GUI instead shows what the AUR choice adds to the
   install (extra official packages to download, rough build time).

Still open: should a user-managed signing key be supported later, so built packages are signed? Not needed
for the first version.

## What the research showed

Checked live against aurweb on 2026-10-05, unless marked otherwise.

- **RPC v5** (`https://aur.archlinux.org/rpc/v5/info?arg[]=name1&arg[]=name2`) returns JSON with `Name`,
  `PackageBase`, `Version`, `Depends`, `MakeDepends`, `OptDepends`, `Provides`, `Conflicts`, `Maintainer`,
  `OutOfDate`, `LastModified`, `NumVotes`, `Popularity`, `URL` and `URLPath`. One request can carry many
  names. The documented limits (a daily request cap per IP and a cap on results per query) come from the
  aurweb documentation, which could not be fetched from here because the Arch wiki blocks scripted
  access; confirm them before relying on exact numbers and always cache.
- RPC has no "who provides X" query for `provides` (search `by` covers name, description, maintainer and
  the dependency fields). A dependency that is only satisfied by a `provides` of another AUR package cannot
  be found by name. The plan treats that as an error with a clear message instead of guessing.
- **A commit id is a sufficient pin.** `git ls-remote https://aur.archlinux.org/<pkgbase>.git` gives the
  current HEAD, `git clone` plus `git checkout <commit>` reproduces that exact tree, and the cgit snapshot
  URL `https://aur.archlinux.org/cgit/aur.git/snapshot/<commit>.tar.gz` returns the same tree as a tarball.
- `makepkg` refuses to run as root and needs `base-devel` (plus `git` for VCS sources). On the target, which
  is a fresh Arch system, both can be installed as ordinary packages by the installer.
- The stock `/etc/pacman.conf` has `LocalFileSigLevel = Optional`, so `pacman -U` accepts the unsigned
  package that `makepkg` produces. Nothing cryptographic vouches for it except the pins this project
  records for the recipe.
- How the installer works today (`kernel/src/install.rs`, `firstboot/firstboot.sh`): the installer resolves
  and downloads repository packages with a pinned SHA-256 each, writes them to `/var/cache/pacman/pkg`,
  lists them in `/var/lib/archstaler/packages.list`, writes validated data files into
  `/var/lib/archstaler`, and the first boot runs one `pacman -U` and then configures the system from those
  files. Third-party data has a precedent: `user_archives` and `user_files` are fetched over HTTPS, and
  scripts from a URL need a mandatory SHA-256.
- The first boot does not need a network today, so a network on the target at that moment is a new
  requirement; the plan handles it with a separate, retrying service.
- Third-party binary repositories such as Chaotic-AUR exist (prebuilt popular AUR packages added as a
  `[chaotic-aur]` repository after importing their key). That is a different feature (custom repositories,
  a third-party trust decision) and is out of scope here.

## Design

### Where each part runs

| Where | What |
|---|---|
| Host (GUI or `xtask`), any OS | Search, review, pin, dependency plan, validation. Needs only HTTPS. No `makepkg`, no container, no Arch |
| ISO | A short validated list: package, pinned commit, pinned content hash, order |
| Installer (kernel) | Adds the needed official packages to the normal package plan, writes the list to `/var/lib/archstaler/aur.list` |
| Target, first boot | Installs a one-shot service that waits for the network, then builds and installs each package |

Because no build happens on the host, the GUI stays cross-platform and no Arch host or container runtime is
needed.

### Config

Host-only input, written by the GUI, read by `hostcfg`:

```lua
aur_packages = {
  { name = "zen-browser-bin", commit = "0f3c...40 hex...", sha256 = "...64 hex..." },
}
```

- `commit` is mandatory and must be a full 40-hex commit id. A moving branch is never accepted.
- `sha256` is the digest of the reviewed recipe (see "Pinning" below), computed by the GUI at review time.
- `name` is a package name; the pkgbase is looked up via RPC and recorded.
- Validation lives in `crates/hostcfg` only (AGENTS.md: no duplicated rules in `xtask` or `gui`).
- Validation also requires a working network service in the config (the `networkmanager` package with
  `NetworkManager.service`, or `systemd-networkd` enabled), because the build needs a network at first
  boot. Without it the config is rejected with that explanation.

Installer-visible part, in `config/src/lib.rs` (shared with `xtask` and the kernel; change all sides
together): `aur = { { pkgbase, name, commit, sha256, vcs = false }, ... }` in build order. Validation in
`config::Config::validate`: `pkgbase` and `name` use the package-name alphabet, `commit` is 40 hex, `sha256`
is 64 hex, at most 16 entries, no duplicates.

### Pinning: what the user reviewed is what runs

The GUI downloads the snapshot of the chosen commit, shows it, and computes one digest over the reviewed
tree: the SHA-256 of the sorted lines `<sha256-of-file>  <relative-path>` for every file (`PKGBUILD`,
`.SRCINFO`, `*.install`, patches). On the target the first-boot service fetches the same commit and
recomputes the digest; a mismatch aborts that package (nothing is built or installed from it). The commit id
already pins the content in git terms; the extra digest protects against a SHA-1-level substitution and
makes the check independent of git.

Source files that the PKGBUILD downloads are covered by the PKGBUILD's own checksums, which are part of the
reviewed tree. `SKIP` checksums and VCS sources are flagged during review.

### Resolution (host, in the GUI and `xtask`)

New host-only crate `crates/aurbuild` (std, no GUI dependencies):

1. **RPC client** with a disk cache (key = request, TTL minutes) and batched `info` calls, using the HTTPS
   client `hostcfg` already uses for sync databases.
2. **`.SRCINFO` parser** for the pinned commit: `pkgbase`, `pkgname` entries, `pkgver`, `pkgrel`, `arch`
   (must contain `x86_64` or `any`), `depends`, `makedepends`, `checkdepends`, `source`, checksums,
   `validpgpkeys`, `install`. Split packages: only the requested `pkgname` is installed, the whole pkgbase
   is built.
3. **Dependency closure.** Every dependency string is classified:
   - available in the official repositories: resolved by the existing resolver (`hostcfg::resolve`, which
     wraps `crates/pkg`) and added to the package plan;
   - another AUR package: looked up by exact name via RPC, must itself be pinned (the GUI offers to pin it);
   - neither: error naming the package and the dependency, with the `provides` limitation explained.
   Build-time-only official dependencies (`makedepends`, plus `base-devel` and `git` when needed) are added
   to the plan too and marked as build dependencies. The result is an ordered build list (topological,
   cycles are an error).
4. **Policy checks:**
   - VCS sources (`git+`, `svn+`, `hg+`, `bzr+`) are allowed but flagged: their content is not pinned by the
     commit, so the plan and the review screen warn that the result cannot be reproduced from the pin, and
     the user ticks a separate acknowledgement per VCS package.
   - A checksum of `SKIP` for a non-VCS source is a warning shown in the review screen.
   - `validpgpkeys` are honoured; `--skippgpcheck` and `--skipchecksums` are never passed.

### Installer (kernel)

In `kernel/src/install.rs`: the official dependencies and build dependencies from the plan are resolved and
downloaded like any other package, so they are installed by the existing `pacman -U` step. The kernel writes
`/var/lib/archstaler/aur.list` (one validated `pkgbase:commit:sha256:name:vcs` line per package, in build
order) and a list of the build-only dependencies (`aur_builddeps.list`) so they can be removed afterwards.
It also installs the one-shot service and its script from the overlay. Nothing is downloaded from the AUR by
the installer itself.

### First boot and the build service

`firstboot/firstboot.sh` enables `archstaler-aur.service` (a one-shot unit, `After=network-online.target`,
`Wants=network-online.target`, `ConditionPathExists=/var/lib/archstaler/aur.list`) and does not wait for it,
so a missing network never blocks or fails the first boot. `firstboot/archstaler-aur.sh` does the work:

1. Create a dedicated unprivileged user (`archstaler-build`, home under `/var/lib/archstaler/aur-build`,
   no password, no login shell, no sudo).
2. For each line, in order: as that user, `git clone https://aur.archlinux.org/<pkgbase>.git`, `git checkout
   <commit>`, recompute the reviewed-tree digest and compare it with the pin. Mismatch: skip this package and
   everything that depends on it, log, continue with unrelated ones.
3. As that user, `makepkg --noconfirm --clean` (dependencies are already installed by the installer, so no
   `sudo` or `-s` is needed), under `timeout`, with output bounded in the log.
4. As root, `pacman -U --noconfirm --needed` on the resulting file, marking AUR-only dependencies
   `--asdeps`. The built file stays in `/var/cache/pacman/pkg`.
5. On success for all packages: remove the build user's directory, remove the build-only dependencies listed
   in `aur_builddeps.list` with `pacman -Rns` when the config asks for it (default: keep them, since
   removing `base-devel` is surprising), and disable the service. On a failure caused by the network, keep
   the list and let the service run again on the next boot, at most five times, then stop and write the
   reason to the log.
6. Log to `/var/log/archstaler-aur.log` and the console; every failure is a warning, never a failed
   installation or a boot blocker. Names come from the validated list, never interpolated into shell source
   (the rule of `firstboot.sh`).

Services named in the config that come from an AUR package cannot be enabled by the normal first-boot step,
because the package does not exist yet at that moment. The build script therefore enables, after each
successful install, the units listed in the config for that package (`aur_packages[i].services`, validated
like `services`).

Since the build happens on the installed system against the current repositories, the usual rolling-release
problem of prebuilt AUR binaries (a soname that changed between build and install) does not occur.

### Manifest and audit trail

`<iso>.aur.json` is written next to the ISO (and shown in the GUI): per package `name`, `pkgbase`,
`version` at pin time, `commit`, reviewed-tree digest, maintainer, last modified, source URLs and their
checksums, VCS flag, the official build dependencies that were added, and the date of the review. It is the
record of what the user approved.

### GUI (`gui/`)

- **Mirrors & packages tab**: a new "AUR packages" group, off by default. Enabling it shows the trust
  warning: arbitrary code runs on the installed machine during its first boot, built from a recipe nobody at
  Arch reviewed; the build user is unprivileged but the build still executes untrusted code on that
  machine; the package is not signed by Arch.
- **Add**: a suggest field querying RPC (reuses `style::suggest`), showing maintainer, votes, popularity,
  out-of-date flag, last modified and a link to the AUR page.
- **Pin and review**: choosing a package resolves HEAD to a commit and opens a read-only viewer of the
  `PKGBUILD`, `.SRCINFO` and any `.install` file, with source URLs and checksums listed and risky lines
  (`curl`/`wget`/`sudo`/`eval`, network use in `build()`, an `.install` script that runs commands) marked.
  The pin and its digest are written only after the user ticks "I reviewed this recipe at this commit". When
  the AUR HEAD later differs from the pin, the tab shows "newer commit available" with a diff; it never
  moves the pin silently.
- **Plan**: shows the ordered build list, the added official packages and their download size, VCS
  warnings, and a note that the first boot needs a network and may take a long time on old hardware
  (compiling can take minutes to hours).
- **Build ISO tab**: nothing special beyond the plan; the ISO build only embeds the short list.

## Implementation order

1. **Schema and validation**: `HostConfig.aur_packages` becomes a list of `{name, commit, sha256, ...}`;
   Lua loader, writer round trip, validation errors with field paths, the network-service rule;
   `config::Config.aur` and its validation. Tests: bad commit ids, duplicate names, hash rules.
2. **`crates/aurbuild`**: RPC client with cache, snapshot download and tree digest, `.SRCINFO` parser,
   dependency closure and ordering, policy checks. Fully testable offline with fixtures (recorded RPC
   answers and snapshot tarballs); an opt-in test hits the real AUR.
3. **xtask**: add the plan's official and build dependencies to the package plan, write the list into the
   config payload, `cargo xtask aur-plan --config X.lua` that prints the plan without building an ISO.
4. **Installer**: `aur.list`, `aur_builddeps.list`, overlay with the service and script.
5. **First-boot script**: `archstaler-aur.sh` and the unit, tested in isolation first (shell test with a
   local git repository standing in for the AUR, run in a throwaway Arch container or chroot), then
   end to end in QEMU (`cargo xtask e2e`) with one small real `-bin` package pinned in a test config. Extra
   runs: a wrong digest must skip the package and leave the base system intact; no network must leave the
   service retrying and the system booting normally.
6. **GUI**: AUR group, pin and review viewer, plan view.
7. **Docs**: `OVERVIEW.md` (crate, schema, flow, first-boot behaviour), `README.md`, `sizes.md` if the
   default build changes, `docs/implement-gui.md` status, this file's status line.

## Security model, stated plainly

- AUR content is unreviewed by Arch. The controls are: a pinned commit and a content digest of what the user
  actually read, verified again on the target before anything runs; an unprivileged build user; bounded
  time and output; and a clear warning that no Arch signature covers the result.
- The build code does run on the installed machine. That is a consequence of building on the target, and it
  is the main difference from a host-side build. There is no sandbox in the first version; a later change
  could wrap the build in `bwrap` (present on Arch as a package) with the network unshared after the
  sources are fetched.
- Malicious PKGBUILDs have been published on the AUR before; popular and orphaned packages are both
  targets. The review step and the pin exist so that a compromised package cannot change between review and
  install.
- Credentials are never part of the config or the manifest.

## As implemented

- **No RPC cache.** The GUI only calls the AUR when the user presses Search, Review or Pin, so the RPC client
  does not cache. The sync databases for dependency resolution are cached for an hour like the resolve preview.
- **Build-only packages stay installed.** `build_deps` is recorded in the config and written to
  `/var/lib/archstaler/aur_builddeps.list`, but nothing removes `base-devel` afterwards yet; that needs an
  option in the config first.
- **Digest rule.** The reviewed-tree digest is computed from the cgit snapshot on the host and from the
  `git clone` checkout on the target. Both give the same result for the packages tried (`zen-browser-bin`,
  `yay`, `yay-bin`). A repository whose `.gitattributes` uses `export-ignore` would make the snapshot differ
  from the checkout; the pin then fails safe (the target refuses the package).
- **Dependencies are plain names.** The plan stores the official package that provides each dependency
  (virtual names such as `ttf-font` become the first provider in repository order), and version constraints
  are not checked on the host; `pacman` checks them on the target.
- **Per-package units.** `services` of a pinned package are enabled right after that package is installed;
  they are edited in the GUI next to the package.
- **Test hooks of the script.** `archstaler-aur.sh` reads `ARCHSTALER_STATE`, `ARCHSTALER_LOG`,
  `ARCHSTALER_AUR_BASE` and `ARCHSTALER_CACHE`, which exist for the tests; the systemd unit sets none of them.
- **Reaching a network in the QEMU test.** QEMU's user-mode network takes DNS from the host's
  `/etc/resolv.conf`. On a host whose resolv.conf names no nameserver, `cargo xtask e2e` runs QEMU inside
  `bwrap` with a resolv.conf that names a public resolver.
