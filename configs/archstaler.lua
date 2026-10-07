-- archstaler: kind=module
-- LuaLS type definitions for the Archstaler `as` config.
-- GENERATED from the Rust schema (crates/hostcfg/src/asconfig.rs) by `cargo xtask gen-luals`; do not edit.
---@meta

---The whole file: exactly one application namespace, `as`.
---@class (exact) AsEnvelope
---@field as AsConfig

---An Archstaler configuration.
---@class (exact) AsConfig
---@field schema integer Schema version; must be 1.
---@field system AsSystem Identity and locale of the installed system.
---@field install AsInstall How and where the installer writes the system.
---@field packages AsPackages What gets installed.
---@field users? AsUser[] Login accounts of the installed system.
---@field first_boot? AsFirstBoot What runs on the installed system's first boot.
---@field build? AsBuild Build-host options: how the ISO is produced. Not part of what the installer receives.

---@class (exact) AsSystem
---@field hostname string Host name, for example `archbox`.
---@field timezone string IANA time zone, for example `Europe/Prague`.
---@field locale string Locale, for example `en_US.UTF-8`.
---@field keymap string Console keymap, for example `us`.
---@field root_password_hash? string SHA-512 crypt hash (`$6$...`) of root's password. Leave it out to keep root locked.

---@class (exact) AsInstall
---@field disk AsDisk Which disk the installer erases.
---@field mirrors string[] Mirror base URLs; `$repo` and `$arch` are substituted (pacman mirrorlist style).
---@field dry_run? boolean Hardware test mode: probe, read disks (never write), resolve packages, print a report, reboot.

---@class (exact) AsDisk
---@field model? string Substring of the disk model; must select exactly one disk. Without it the serial alone selects the disk.
---@field confirm_serial? string Serial of the one disk that may be erased. Not used with `auto_largest`.
---@field auto_largest? boolean WARNING: erase and install onto the largest disk without any confirmation.
---@field esp_mib integer Size of the EFI system partition in MiB.

---@class (exact) AsPackages
---@field explicit string[] Package or group names installed explicitly. Names are suggestions: the resolver decides.
---@field providers? table<string, string> Dependency name to the package that provides it, for example `{ initramfs = "mkinitcpio" }`.
---@field aur? AsAur[] AUR packages, in build order, pinned to the reviewed recipe (see `docs/aur.md`).

---One AUR package, pinned to the reviewed recipe.
---@class (exact) AsAur
---@field name string The package to install (a `pkgname` of the recipe).
---@field pkgbase string The AUR git repository name.
---@field commit string Full 40-digit lower-case hex commit of the reviewed recipe.
---@field sha256 string SHA-256 over the reviewed tree, 64 lower-case hex digits.
---@field vcs? boolean The recipe builds from a VCS source that the commit does not pin.
---@field as_dep? boolean Install it as a dependency of another entry.
---@field deps? string[] Official packages the recipe needs, to build and to run.
---@field build_deps? string[] The part of `deps` that only the build needs.
---@field services? string[] Units to enable once the package is installed.

---@class (exact) AsUser
---@field name string Login name.
---@field password_hash string SHA-512 crypt hash (`$6$...`); plaintext is never accepted.
---@field groups string[] Supplementary groups, for example `wheel`.
---@field shell string Login shell, for example `/bin/bash`.

---@class (exact) AsFirstBoot
---@field services? string[] Units enabled on first boot.
---@field kernel_params? string[] Extra kernel command line parameters.
---@field scripts? AsScript[] Scripts run as root, in order, at the end of the first boot.
---@field user_files? AsUserFile[] Files downloaded into every user's home directory (a failed download only warns).
---@field user_archives? AsUserArchive[] Zip archives extracted into every user's home directory (a failed download only warns).

---A first-boot script. Exactly one source: none of `file`/`url` (a built-in script of that `id`),
---`file` (a local file, relative to the config), or `url` with `sha256`.
---@class (exact) AsScript
---@field id string Built-in script name (`enable-sshd`, `enable-fstrim`) or a name of your own for `file`/`url`.
---@field args? string[] Extra arguments, passed as separate words.
---@field file? string Path of a local script, relative to the config file.
---@field url? string `https://` URL of the script; needs `sha256`.
---@field sha256? string Lower-case hex SHA-256 of the script at `url`.

---@class (exact) AsUserFile
---@field url string `https://` URL of the file (at most 1 MiB).
---@field dest string Destination relative to the home directory, for example `.config/hypr/hyprland.lua`.

---@class (exact) AsUserArchive
---@field url string `https://` URL of a zip file (at most 16 MiB).

---@class (exact) AsBuild
---@field profile? "super-small"|"large" Compiler profile of the installer: `super-small` (default) or `large` (keeps panic messages).
---@field tethering? boolean Include USB tethering (iPhone, Android, USB Ethernet) in the installer.
---@field installer_drivers? ("virtio-blk"|"ahci"|"ata"|"nvme"|"alx"|"vmxnet3"|"virtio-net"|"e1000"|"igb"|"r8169"|"rtl8139")[] Installer-kernel drivers to include; leave it out for all of them.
