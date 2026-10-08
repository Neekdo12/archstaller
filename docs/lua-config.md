# Archstaller Lua configuration reference

This file is written to be pasted whole into a chatbot, or read by a person, with no access to the source code. It
describes every setting of an Archstaller config, the rules the build enforces, what the installer does with each
value, and complete working examples. If you are a chatbot: read "Rules for whoever writes a config" first.

## What Archstaller is

Archstaller builds a bootable ISO that installs Arch Linux **unattended**. Everything the installer needs to know
comes from one Lua file, the *config*, evaluated on the machine that builds the ISO. There is no interactive
installer: the machine boots the ISO, picks a disk, downloads packages from an Arch mirror, writes the system and
reboots. A config therefore decides, among other things, **which disk is erased**.

The installer itself is a small custom kernel, not Linux. The installed system is ordinary Arch Linux with the
packages you list, installed from the official `core` and `extra` repositories (plus optional pinned AUR packages).
It boots with GRUB.

How a config is used:

```sh
cargo xtask build --config my.lua --out my.iso    # command line, from the repository checkout
```

or open the file in the `archstaller-gui` desktop app (Linux), which edits the same files, checks them with the same
code and builds the ISO. The build rejects an invalid config with a message that names the field, for example
`as.system.hostname: invalid type: integer 5, expected a string`.

## Rules for whoever writes a config

1. Output one complete Lua file, nothing the user cannot save as `*.lua`. The file must `return { as = as }` (see
   "File layout").
2. **Use only the keys documented here.** An unknown or misspelled key is an error, not a warning. Do not invent
   keys, and do not use the old flat layout (top-level `hostname = ...`); it still loads with a warning but new
   files use `as`.
3. **Never invent secrets or pins.** Passwords are stored only as SHA-512 crypt hashes (`$6$...`); do not write a
   plaintext password, and do not make up a hash: tell the user to run `openssl passwd -6` (or `mkpasswd -m
   sha-512`) and paste the result. AUR entries need a real 40-digit commit and a real SHA-256 from `cargo xtask
   aur-pin NAME` (or the GUI); never guess them, leave `aur` empty instead.
4. **The disk setting is dangerous.** `auto_largest = true` erases the largest disk of whatever machine boots the
   ISO without asking. `confirm_serial = "..."` erases only the disk with that serial. Do not choose
   `auto_largest = true` unless the user clearly says the target machine may lose its largest disk; otherwise ask for
   the serial (see "How to find a disk serial").
5. Numbers are Lua integers (`esp_mib = 1024`), never strings (`"1024"`) or floats (`1024.0`). Booleans are `true`
   or `false`. Lists are `{ "a", "b" }`. A list that is empty is `{}`.
6. Only official-repository package names (`core`, `extra`) go in `packages.explicit`. A name that is not there makes
   the build fail when the installer resolves it. Do not list AUR or `multilib` packages there.
7. Before answering, check the list in "Checklist" at the end.

Questions to ask if the user has not said: which disk to install on (serial, or really the largest one), hostname,
time zone, locale, keymap, user name(s) and a password hash, what the machine is for (server, desktop and which),
how it gets network (wired DHCP is what the installer itself uses; Wi-Fi is not supported by the installer).

## File layout

```lua
---@type AsConfig
local as = {
  schema = 1,
  system     = { ... },   -- identity, locale, root password
  install    = { ... },   -- disk, mirrors, dry-run mode
  packages   = { ... },   -- what to install
  users      = { ... },   -- login accounts (a list)
  first_boot = { ... },   -- services, kernel parameters, scripts, files
  build      = { ... },   -- how the ISO itself is built (not part of the installed system)
}

return { as = as }
```

- The returned table has exactly one key, `as`. Mixing `as` with other top-level keys is an error.
- `schema` must be `1`.
- Required sections: `schema`, `system`, `install`, `packages`. `users`, `first_boot` and `build` may be left out.
- The first line `---@type AsConfig` is a comment for editors (Lua Language Server); it is optional.
- A config is real Lua, so it may use locals, `require("module")` for helper files in the same directory, and
  functions. Keep it simple: the result must be plain data of the shapes below.
- Passwords, hashes and URLs are strings in double quotes; escape `"` and `\` with a backslash.

## Complete reference

Types: *string*, *integer*, *boolean*, *list of X*, *table*. "Required" means the build fails without it.

### `schema` (integer, required)

Always `1`.

### `system` (required)

| Field | Type | Required | Meaning and rules |
|---|---|---|---|
| `hostname` | string | yes | Host name. 1–64 characters of `A-Z a-z 0-9 . -`. |
| `timezone` | string | yes | IANA zone such as `Europe/Prague` or `UTC`. 1–64 characters of `A-Z a-z 0-9 / _ + -`, no `..`. The zone must exist on the installed system (`/usr/share/zoneinfo`). |
| `locale` | string | yes | A glibc locale such as `en_US.UTF-8`. 1–64 characters of `A-Z a-z 0-9 . @ _ -`. It is enabled in `locale.gen` and generated at first boot. |
| `keymap` | string | yes | Console keymap such as `us`, `cz`, `de-latin1`. 1–64 characters of `A-Z a-z 0-9 . _ -`. Written to `/etc/vconsole.conf`. |
| `root_password_hash` | string | no | A SHA-512 crypt hash (`$6$...`) for root. Leave it out to keep root **locked** (log in as a user and use `sudo`). |

### `install` (required)

| Field | Type | Required | Meaning and rules |
|---|---|---|---|
| `disk` | table | yes | Which disk to erase, see below. |
| `mirrors` | list of string | yes | At least one Arch mirror base URL, `http://` or `https://`, no spaces or quotes. `$repo` and `$arch` are replaced by pacman's usual values. Tried in order. Example: `"https://geo.mirror.pkgbuild.com/$repo/os/$arch"`. |
| `dry_run` | boolean | no, default `false` | Hardware test mode: the installer probes devices, reads disks (it never writes), brings up the network, resolves the package databases, prints a report and reboots. No disk is selected. Used by the tester preset. |

`install.disk`:

| Field | Type | Required | Meaning and rules |
|---|---|---|---|
| `esp_mib` | integer | yes | Size in MiB of the FAT32 boot partition (EFI system partition), mounted at `/boot`. 64–8192. `1024` is a good default. |
| `confirm_serial` | string | see below | Serial number of the one disk that may be erased. The install aborts, writing nothing, if no disk or more than one disk matches. |
| `model` | string | no | A substring of the disk model name that must also match. Use it with `confirm_serial` to be extra sure. |
| `auto_largest` | boolean | no, default `false` | **Erase and install onto the largest disk without any confirmation.** Fails (writing nothing) if two disks tie for largest. |

Rules: either `auto_largest = true`, or a non-empty `confirm_serial` is required (not needed when `dry_run = true`).
When `auto_largest` is true, `confirm_serial` is ignored.

Disk layout the installer writes: GPT with a 1 MiB BIOS-boot partition, the ESP (`esp_mib`), and the rest of the disk
as one ext4 root partition. A USB stick the installer was booted from is never a candidate for `auto_largest`.

### `packages` (required)

| Field | Type | Required | Meaning and rules |
|---|---|---|---|
| `explicit` | list of string | yes, not empty | Package or package-group names installed explicitly, from the official `core` and `extra` repositories. Names are 1–64 characters of `A-Z a-z 0-9 @ . _ + -`. Dependencies are resolved and added automatically. |
| `providers` | table string→string | no | Which package satisfies a dependency that several packages provide: `{ initramfs = "mkinitcpio", jack = "pipewire-jack" }`. Key = virtual name, value = real package. When a dependency is ambiguous and not listed here, the first candidate by repository priority and name is used (like pacman's default answer). |
| `aur` | list of table | no | Pinned AUR packages, see "AUR packages". Up to 16. |

A system that boots needs at least these packages (all the presets include them):

- `base` and `linux` (kernel; the boot menu entry expects `/vmlinuz-linux` and `/initramfs-linux.img`),
- `mkinitcpio` and `providers = { initramfs = "mkinitcpio" }` (the first boot builds the initramfs with
  `mkinitcpio`; without the provider entry the dependency `initramfs` is ambiguous),
- `grub` (boot loader, installed at first boot) and `efibootmgr` (needed on UEFI machines),
- `sudo` if any user is in the `wheel` group (the installer writes a `%wheel` rule to `/etc/sudoers.d/10-wheel`, but
  the `sudo` package itself must be installed),
- firmware for the hardware: `linux-firmware` (everything, large) or smaller vendor packages such as
  `linux-firmware-intel`, `linux-firmware-realtek`, `linux-firmware-amdgpu`, `linux-firmware-radeon`.

Optional common additions: `nano` or `vim`; `openssh` (+ the `sshd.service` unit); `networkmanager` (+
`NetworkManager.service`) or `systemd-networkd`/`systemd-resolved` (units `systemd-networkd.service`,
`systemd-resolved.service`) for the installed system's network; `ly` (+ `ly@tty2.service`) as a text-mode login
manager that starts desktop sessions.

### `users` (list of table, optional)

Each entry creates one account with a home directory at first boot.

| Field | Type | Required | Meaning and rules |
|---|---|---|---|
| `name` | string | yes | Login name: 1–64 characters of `A-Z a-z 0-9 _ -`, not starting with `-`, not `root`. Lower-case names are the convention. |
| `password_hash` | string | yes | SHA-512 crypt hash starting with `$6$`, longer than 20 characters, only `A-Z a-z 0-9 $ . /`. Never plaintext. |
| `groups` | list of string | yes (may be `{}`) | Supplementary groups (each 1–64 characters of `A-Z a-z 0-9 _ -`). Missing groups are created. Put the administrator in `wheel` to allow `sudo`. |
| `shell` | string | yes | Absolute path such as `/bin/bash` (characters `A-Z a-z 0-9 / _ - .`). The shell package must be installed (`bash` comes with `base`; `zsh`, `fish` need adding). |

With no users and no `root_password_hash` the installed system has only a locked root account and cannot be logged
into. Create a hash with `openssl passwd -6` (it asks for the password and prints the hash).

### `first_boot` (table, optional)

Everything here is applied on the installed system's first boot, in the order of this list: users are created, then
files and archives are placed, then services are enabled, then scripts run, then the initramfs is built and GRUB is
installed.

| Field | Type | Meaning and rules |
|---|---|---|
| `services` | list of string | systemd units to enable, e.g. `"NetworkManager.service"`, `"sshd.service"`, `"ly@tty2.service"`, `"systemd-timesyncd.service"`. 1–64 characters of `A-Z a-z 0-9 @ . _ : - \`. A unit whose package is not installed only produces a warning. |
| `kernel_params` | list of string | Extra kernel command-line words, each without spaces or double quotes, e.g. `"quiet"`, `"console=ttyS0,115200"`. |
| `scripts` | list of table | Scripts run **as root** at the end of the first boot, see "Scripts". Up to 32. |
| `user_files` | list of table | Files downloaded during installation into every user's home, see below. |
| `user_archives` | list of table | Zip archives downloaded during installation and unpacked into every user's home, see below. |

`user_files` entries: `{ url = "https://...", dest = ".config/foo/bar.conf" }`. `url` must be `https://` with no
spaces or quotes (the file is at most 1 MiB). `dest` is a relative path under the home directory made of
`A-Z a-z 0-9 . _ - /` with no empty, `.` or `..` components. The file is installed owned by each user, mode 0644.

`user_archives` entries: `{ url = "https://example.org/dotfiles.zip" }`. `https://` only, at most 16 MiB. Paths in
the zip are relative to the home directory (for example `.config/hypr/hyprland.lua`) and are extracted as each
user.

For both, a server that is down, a non-200 answer, an oversized file, or something that is not a zip only prints a
warning; the installation continues without it. **Security:** these files are not verified beyond HTTPS. A desktop
configuration (for example a Hyprland config) can run arbitrary commands when the session starts, so only use URLs
the user trusts.

### Scripts

`first_boot.scripts` entries describe code that runs as root. Exactly one source per entry:

| Form | Fields | Meaning |
|---|---|---|
| Built-in | `id` only | One of the built-in scripts below. |
| Local file | `id`, `file` | A script file next to the config (path relative to the config file), embedded into the ISO at build time. At most 64 KiB. |
| Download | `id`, `url`, `sha256` | Downloaded by the installer; `url` is `https://`; `sha256` is the 64-digit lower-case hex SHA-256 of the script, and the install fails on a mismatch. |

Other fields: `args` (list of up to 16 words passed to the script as separate arguments, each of
`A-Z a-z 0-9 . _ = : / @ + , -`; no spaces, quotes or shell syntax). `id`: 1–64 characters of `A-Z a-z 0-9 . _ -`,
unique within the config. Scripts run with `bash`, in list order, with a 15-minute timeout; a failing script only
logs a warning. Give `file` and `url` together, or `sha256` without `url`, and the build rejects it.

Built-in scripts:

| `id` | What it does | Needs |
|---|---|---|
| `enable-sshd` | Enables the OpenSSH server at boot. | package `openssh` |
| `enable-fstrim` | Enables the weekly TRIM timer (`fstrim.timer`) for SSDs. | nothing |

Prefer `services = { "sshd.service" }` for simple enabling; use scripts for work a unit cannot do. Do not generate a
`url` script for the user without a real hash.

### AUR packages (`packages.aur`)

Packages from the Arch User Repository are **built on the installed machine** at its second boot, by an
unprivileged user, from a recipe pinned to one git commit that a human reviewed. They are not in the ISO and are
not signed by Arch. Each entry:

| Field | Type | Meaning |
|---|---|---|
| `name` | string | Package to install (a `pkgname` of the recipe). |
| `pkgbase` | string | AUR git repository name. |
| `commit` | string | Full 40-digit **lower-case hex** commit id of the reviewed recipe. A branch name is not a pin. |
| `sha256` | string | SHA-256 over the reviewed tree, 64 lower-case hex digits. |
| `vcs` | boolean, optional | The recipe builds from a VCS source the commit does not pin (the user accepted that). |
| `as_dep` | boolean, optional | Install it as a dependency of another entry. |
| `deps` | list of string, optional | Official packages the recipe needs, to build and to run. |
| `build_deps` | list of string, optional | The part of `deps` that only the build needs (must be inside `deps`). |
| `services` | list of string, optional | Units to enable once the package is installed. |

All pin values come from `cargo xtask aur-pin NAME` or the GUI's AUR search; they cannot be written from memory.
Rules: at most 16 entries, no duplicate `name`, and `first_boot.services` must enable a network service
(`NetworkManager.service`, `systemd-networkd.service`, `dhcpcd.service` or `connman.service`) because the build
needs a network.

### `build` (table, optional)

Settings about producing the ISO. They never reach the installed system.

| Field | Type | Meaning and rules |
|---|---|---|
| `profile` | string | `"super-small"` (default: smallest installer, no panic messages) or `"large"` (regular release build, panic messages kept, useful for debugging). `"extra-large"` is reserved and rejected. |
| `tethering` | boolean | `false` by default. `true` adds USB tethering (iPhone, Android, USB Ethernet adapters) to the installer, about 100 KiB. Lets the installer use a phone's hotspot or a USB network adapter when no wired NIC has a link. |
| `installer_drivers` | list of string | Which installer drivers to include. Leave it **out** to include all of them. If given: ids must be valid and unique, with at least one storage driver and at least one network driver (or `tethering = true`). |

Installer drivers (these are drivers of the installer, not packages of the installed system):

| id | Class | Hardware |
|---|---|---|
| `virtio-blk` | storage | virtio block device (QEMU, most hypervisors) |
| `ahci` | storage | SATA through AHCI |
| `ata` | storage | legacy IDE-mode SATA and parallel ATA disks (PIO, slow) |
| `nvme` | storage | NVMe SSDs |
| `virtio-net` | network | virtio network device |
| `e1000` | network | Intel e1000 / e1000e |
| `igb` | network | Intel igb / igc |
| `r8169` | network | Realtek RTL8168/8111/8125/8126 |
| `rtl8139` | network | Realtek RTL8139 |
| `vmxnet3` | network | VMware vmxnet3 virtual NIC |
| `alx` | network | Qualcomm Atheros AR81xx/AR816x, Killer E2x00 (untested) |

Not supported by the installer: Wi-Fi, SMP (it uses one CPU core), Secure Boot, file systems other than ext4,
architectures other than x86_64, a graphical or interactive installer.

## What the installer and the first boot do with a config

1. The ISO boots (BIOS or UEFI) and loads the installer kernel; the config is embedded in it.
2. The installer picks the disk (`install.disk`); nothing is written before this succeeds.
3. It brings up a wired NIC by DHCP (or USB tethering if built in), downloads the `core` and `extra` databases from
   the first working mirror over HTTPS, and resolves `packages.explicit` plus dependencies.
4. It partitions the disk, downloads every package, checks SHA-256 and the Arch PGP signature of each, and writes an
   ext4 root file system. It writes the hostname, locale, keymap, time zone, mirror list and the `wheel` sudo rule.
5. It reboots into the **first boot**: initializes the pacman keyring, runs `pacman -U` on the downloaded packages
   (all install scriptlets and hooks run), creates users, enables `first_boot.services`, runs scripts, builds the
   initramfs, installs GRUB and reboots again.
6. If `packages.aur` is not empty, the build of those packages happens on the boot after that, once the network is
   up.

Pull the install stick before the installer's final reboot (the end of step 4): if the firmware still prefers the stick, the installer
runs again and, with `auto_largest`, erases the system that was just installed.

Disk space: the root partition holds the package cache (the compressed packages) *and* the installed system, and
`pacman -U` at first boot wants the installed size free once more. A large desktop is the case to plan for: the
`omarchy` preset (2.4 GiB download, 906 packages) failed its install test on a 16 GiB disk with `Partition / too
full` and passed on 24 GiB. The preset table below gives the disk each preset was sized for.

## Presets (ready starting points)

The repository's `configs/` directory holds configs that start with the line `-- archstaller: kind=preset`. They
install onto the **largest disk without asking** (`auto_largest = true`), create the user `passwd_is_passwd` with
password `passwd` (change it before connecting the machine to a network) and keep root locked.

| Preset | What you get | Download | Disk |
|---|---|---|---|
| `minimal` | console system, systemd-networkd | ~0.4 GiB | 4 GiB+ |
| `server` | minimal + OpenSSH, htop, tmux, rsync, vim (sshd enabled) | ~0.4 GiB | 4 GiB+ |
| `i3` | Xorg + i3, NetworkManager, PipeWire, Firefox | ~1.0 GiB | 10 GiB+ |
| `sway` | Sway (Wayland), waybar, wofi, foot, mako, NetworkManager, PipeWire, Firefox, plus a written `~/.config/sway/config` | ~1.1 GiB (estimate) | 10 GiB+ |
| `hyprland` | Hyprland (Wayland), kitty, rofi, waybar, Neovim, Nerd fonts, plus a downloaded config zip | ~1.4 GiB | 15 GiB+ |
| `plasma` | KDE Plasma (Wayland session) | ~1.3 GiB | 20 GiB+ |
| `omarchy` | Hyprland with the application set of Omarchy v4.0.4, official packages only | ~2.4 GiB | 24 GiB+ |
| `tester` | read-only hardware test (`dry_run`), installs nothing | — | none written |

## Examples

Each example below is checked by the project's tests: it loads and validates with the same code the build uses.

### 1. Smallest useful config

One disk named by serial, console only, DHCP through systemd-networkd, one admin user. Replace the serial and the
hash before use.

<!-- check: valid -->
```lua
---@type AsConfig
local as = {
  schema = 1,

  system = {
    hostname = "box",
    timezone = "Europe/Prague",
    locale = "en_US.UTF-8",
    keymap = "us",
  },

  install = {
    disk = { confirm_serial = "S4EVNX0N123456A", esp_mib = 1024 },
    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  },

  packages = {
    explicit = { "base", "linux", "linux-firmware", "mkinitcpio", "grub", "efibootmgr", "sudo", "nano" },
    providers = { initramfs = "mkinitcpio" },
  },

  users = {
    -- hash of the password "passwd": replace it (openssl passwd -6)
    { name = "admin", password_hash = "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.", groups = { "wheel" }, shell = "/bin/bash" },
  },

  first_boot = {
    services = { "systemd-networkd.service", "systemd-resolved.service", "systemd-timesyncd.service" },
  },
}

return { as = as }
```

### 2. A server with SSH

Adds OpenSSH, enabled at boot, a kernel parameter, and the TRIM timer through a built-in script. Uses the largest-disk
mode, which erases the largest disk of the machine without asking: only for a machine that is meant to be wiped.

<!-- check: valid -->
```lua
---@type AsConfig
local as = {
  schema = 1,
  system = { hostname = "srv1", timezone = "UTC", locale = "en_US.UTF-8", keymap = "us" },
  install = {
    disk = {
      -- WARNING: erases and installs onto the LARGEST disk, without asking.
      auto_largest = true,
      esp_mib = 1024,
    },
    mirrors = {
      "https://geo.mirror.pkgbuild.com/$repo/os/$arch",
      "https://mirror.rackspace.com/archlinux/$repo/os/$arch",
    },
  },
  packages = {
    explicit = { "base", "linux", "linux-firmware-intel", "linux-firmware-realtek", "mkinitcpio", "grub", "efibootmgr", "sudo", "openssh", "htop", "tmux", "vim" },
    providers = { initramfs = "mkinitcpio" },
  },
  users = {
    { name = "ops", password_hash = "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.", groups = { "wheel" }, shell = "/bin/bash" },
  },
  first_boot = {
    services = { "systemd-networkd.service", "systemd-resolved.service", "sshd.service" },
    kernel_params = { "quiet" },
    scripts = { { id = "enable-fstrim" } },
  },
}

return { as = as }
```

### 3. A desktop (i3 on Xorg)

Shows a larger package list, a provider choice for a dependency with several candidates, NetworkManager and a text
login manager. Desktop machines normally want the full `linux-firmware`.

<!-- check: valid -->
```lua
---@type AsConfig
local as = {
  schema = 1,
  system = { hostname = "desk", timezone = "Europe/Berlin", locale = "de_DE.UTF-8", keymap = "de-latin1" },
  install = {
    disk = { confirm_serial = "WD-WCC4N1234567", model = "WDC", esp_mib = 1024 },
    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  },
  packages = {
    explicit = {
      "base", "linux", "linux-firmware", "mkinitcpio", "grub", "efibootmgr", "sudo", "nano", "ly",
      "xorg-server", "xorg-xinit", "xf86-input-libinput", "mesa",
      "i3-wm", "i3status", "i3lock", "dmenu", "alacritty",
      "networkmanager", "pipewire", "pipewire-pulse", "wireplumber",
      "ttf-dejavu", "noto-fonts", "firefox",
    },
    providers = { initramfs = "mkinitcpio", jack = "pipewire-jack" },
  },
  users = {
    { name = "anna", password_hash = "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.", groups = { "wheel", "audio", "video" }, shell = "/bin/bash" },
  },
  first_boot = {
    services = { "NetworkManager.service", "systemd-timesyncd.service", "ly@tty2.service" },
  },
}

return { as = as }
```

### 4. Hardware test (installs nothing)

`dry_run = true`: the machine boots, probes, reads disks, tests the network and package resolution, prints a report
and reboots. No disk selector is needed. `profile = "large"` keeps panic messages for debugging.

<!-- check: valid -->
```lua
---@type AsConfig
local as = {
  schema = 1,
  system = { hostname = "tester", timezone = "Europe/Prague", locale = "en_US.UTF-8", keymap = "us" },
  install = {
    dry_run = true,
    disk = { esp_mib = 1024 },
    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  },
  packages = {
    explicit = { "base", "linux", "mkinitcpio", "grub", "efibootmgr", "sudo", "nano" },
    providers = { initramfs = "mkinitcpio" },
  },
  build = { profile = "large" },
}

return { as = as }
```

### 5. Build options: only some installer drivers, with tethering

For a machine whose hardware is known: a smaller installer. The list needs one storage and one network driver (or
`tethering = true`).

<!-- check: valid -->
```lua
---@type AsConfig
local as = {
  schema = 1,
  system = { hostname = "vm1", timezone = "UTC", locale = "en_US.UTF-8", keymap = "us" },
  install = {
    disk = { confirm_serial = "vm-disk-0", esp_mib = 512 },
    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  },
  packages = {
    explicit = { "base", "linux", "mkinitcpio", "grub", "efibootmgr", "sudo" },
    providers = { initramfs = "mkinitcpio" },
  },
  users = {
    { name = "dev", password_hash = "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.", groups = { "wheel" }, shell = "/bin/bash" },
  },
  first_boot = { services = { "systemd-networkd.service", "systemd-resolved.service" } },
  build = {
    profile = "super-small",
    tethering = false,
    installer_drivers = { "virtio-blk", "virtio-net" },
  },
}

return { as = as }
```

### 6. Files, an archive and a downloaded script

User files and archives are fetched by the installer over HTTPS (not verified beyond that). A downloaded script
must be pinned by its SHA-256; the digest below is a placeholder-shaped example and must be replaced by the real
`sha256sum` of the script.

<!-- check: valid -->
```lua
---@type AsConfig
local as = {
  schema = 1,
  system = { hostname = "dots", timezone = "UTC", locale = "en_US.UTF-8", keymap = "us" },
  install = {
    disk = { confirm_serial = "SER123", esp_mib = 1024 },
    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  },
  packages = {
    explicit = { "base", "linux", "mkinitcpio", "grub", "efibootmgr", "sudo", "git" },
    providers = { initramfs = "mkinitcpio" },
  },
  users = {
    { name = "me", password_hash = "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.", groups = { "wheel" }, shell = "/bin/bash" },
  },
  first_boot = {
    user_files = {
      { url = "https://raw.githubusercontent.com/example/dotfiles/main/gitconfig", dest = ".gitconfig" },
    },
    user_archives = { { url = "https://example.org/config.zip" } },
    scripts = {
      { id = "tweak", url = "https://example.org/tweak.sh", sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", args = { "--fast" } },
    },
  },
}

return { as = as }
```

### 7. A pinned AUR package

These pin values are real for the moment they were made and may be out of date; always regenerate with
`cargo xtask aur-pin yay-bin`. Note the network service, which is required.

<!-- check: valid -->
```lua
---@type AsConfig
local as = {
  schema = 1,
  system = { hostname = "aur", timezone = "UTC", locale = "en_US.UTF-8", keymap = "us" },
  install = {
    disk = { confirm_serial = "SER123", esp_mib = 512 },
    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  },
  packages = {
    explicit = { "base", "linux", "mkinitcpio", "grub", "efibootmgr", "networkmanager" },
    providers = { initramfs = "mkinitcpio" },
    aur = {
      { name = "yay-bin", pkgbase = "yay-bin", commit = "13e0a4754d106a9252b7479bf1b370fbe454fc48", sha256 = "b73aa937ff1cd4a76430393670c3316f71bf48ee2d996114db721faab227ee71", deps = { "base-devel", "git", "pacman" }, build_deps = { "base-devel" } },
    },
  },
  first_boot = { services = { "NetworkManager.service" } },
}

return { as = as }
```

### 8. Composing configs from modules

A config may `require` a helper file that sits next to it and returns plain data. This is how the presets share
defaults. `common.lua`:

```lua
local M = {}
function M.base(hostname)
  return {
    system = { hostname = hostname, timezone = "Europe/Prague", locale = "en_US.UTF-8", keymap = "us" },
    install = { disk = { auto_largest = true, esp_mib = 1024 }, mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" } },
  }
end
return M
```

and the config:

```lua
local base = require("common").base("lab")
---@type AsConfig
local as = {
  schema = 1,
  system = base.system,
  install = base.install,
  packages = {
    explicit = { "base", "linux", "mkinitcpio", "grub", "efibootmgr" },
    providers = { initramfs = "mkinitcpio" },
  },
}
return { as = as }
```

When asked for a single file, do not use `require`: write everything into the one file.

## Errors you will see and what they mean

The build prints the first problem. Shape and type errors start with the path of the field under `as` (list positions count from 1). Rule errors (the second half of the table) use the short names `disk.confirm_serial`, `build.profile`, `scripts[N]`, `installer_drivers`, without the `as.install.` or `as.build.` prefix; the field is the one documented above under the same name.

| Message (shortened) | Cause and fix |
|---|---|
| `as.system.hostname: invalid type: integer ..., expected a string` | A value has the wrong Lua type. Strings need quotes; integers must not have quotes. |
| `as.install.disk.esp_mib: invalid type: floating point ..., expected u32` | Wrote `1024.0` or `"1024"`. Use the integer `1024`. |
| `unknown field ... expected one of ...` | Misspelled or invented key. Use the names in this document. |
| `missing field ...` | A required key is absent (`schema`, `system`, `install`, `packages`, `esp_mib`, `mirrors`, ...). |
| `the config mixes the `as` namespace with legacy top-level keys` | Something sits next to `as` in the returned table. Return only `{ as = as }`. |
| `as.schema: this version reads schema 1` | `schema` is not `1`. |
| `disk.confirm_serial must be set (or enable disk.auto_largest)` | No disk chosen. Give a serial or opt in to `auto_largest`. |
| `invalid hostname: "..."` (also timezone, locale, keymap, service, group, shell, mirror, package name) | The value has a forbidden character or is too long; see the character sets above. |
| `invalid user name: "root"` | `root` is configured through `system.root_password_hash`, not `users`. |
| `invalid password_hash (need a $6$ SHA-512 crypt hash) for user ...` | Plaintext or another hash type. Run `openssl passwd -6`. |
| `at least one mirror is required` / `invalid mirror` | `mirrors` is empty or an entry is not an `http(s)://` URL. |
| `packages must not be empty` | `packages.explicit` is `{}`. |
| `build.profile: unknown build profile ...` | Only `"super-small"` and `"large"` are accepted. |
| `installer_drivers: no storage driver selected ...` / `no network driver selected ...` | The list needs at least one of each class (or `tethering = true`). |
| `scripts[N]: "x" is not a built-in script (give it a file or a url + sha256)` | An `id` alone must be `enable-sshd` or `enable-fstrim`. |
| `scripts[N]: a downloaded script needs its sha256` | Add the digest, or use a local `file`. |
| `aur_packages: ... needs a network` | `packages.aur` is set but no network service is enabled in `first_boot.services`. |

## How to find a disk serial

On the machine that will be installed (booted from any Linux live system):

```sh
lsblk -d -o NAME,SIZE,MODEL,SERIAL
```

Use the `SERIAL` value for `confirm_serial` and, optionally, a distinctive part of `MODEL` for `model`. If two disks
could match, the installer refuses to write anything. Virtual machines often report serials set in the VM
configuration.

## Checklist (run through it before giving a config to the user)

- [ ] The file ends with `return { as = as }` and `schema = 1` is present.
- [ ] Only documented keys are used; no top-level keys beside `as`.
- [ ] `system` has `hostname`, `timezone`, `locale`, `keymap` and nothing misspelled.
- [ ] `install.disk` has `esp_mib` (integer) and either `confirm_serial` or `auto_largest = true`, and the user has
      agreed to the disk choice. `auto_largest` carries the warning comment.
- [ ] At least one `http(s)` mirror.
- [ ] `packages.explicit` contains `base`, `linux`, `mkinitcpio`, `grub` (and `efibootmgr` for UEFI) plus firmware,
      and `providers` has `initramfs = "mkinitcpio"`.
- [ ] Every package is in the official `core` or `extra` repository.
- [ ] Every user has a real `$6$` hash (or the user was told to generate one), a groups list and a shell path; `sudo`
      is installed if anyone is in `wheel`.
- [ ] Each enabled service belongs to an installed package; the machine has a network service if it needs one
      (`NetworkManager.service` or `systemd-networkd.service`).
- [ ] No invented hashes, commits or digests anywhere.
- [ ] Integers are integers; booleans are `true`/`false`; lists use braces.
- [ ] Tell the user which disk will be erased, how to build (`cargo xtask build --config FILE --out FILE.iso`, or
      the GUI), and that the install is unattended.

## Appendix: the old flat layout

Configs written for earlier versions return a flat table (`return { hostname = ..., disk = {...}, ... }`). That
layout still loads, with a warning, but should be converted. Mapping from old key to new path:

| Old key | New path |
|---|---|
| `hostname`, `timezone`, `locale`, `keymap` | `as.system.<same name>` |
| `root_password_hash` | `as.system.root_password_hash` |
| `disk` | `as.install.disk` |
| `mirrors` | `as.install.mirrors` |
| `dry_run` | `as.install.dry_run` |
| `packages` (list) | `as.packages.explicit` |
| `providers` (`{ {"initramfs","mkinitcpio"} }`) | `as.packages.providers` (`{ initramfs = "mkinitcpio" }`) |
| `aur_packages` | `as.packages.aur` |
| `users` | `as.users` |
| `services`, `kernel_params`, `scripts`, `user_files`, `user_archives` | `as.first_boot.<same name>` |
| `build` | `as.build` |
| `installer_drivers` | `as.build.installer_drivers` |
