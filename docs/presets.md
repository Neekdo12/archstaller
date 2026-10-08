# Presets

The ISOs built by `cargo xtask presets`. Shared defaults live in `configs/common.lua`.

| ISO | What you get | Download | Disk |
|---|---|---|---|
| `archstaller-minimal.iso` | console system, systemd-networkd | ~0.4 GiB | 4 GiB+ |
| `archstaller-server.iso` | minimal + OpenSSH, htop, tmux, rsync, vim | ~0.4 GiB | 4 GiB+ |
| `archstaller-i3.iso` | Xorg + i3, NetworkManager, PipeWire, Firefox | ~1.0 GiB | 10 GiB+ |
| `archstaller-sway.iso` | Sway (Wayland) with waybar, wofi, foot, mako, swaylock/swayidle, portals, NetworkManager, PipeWire, Firefox; a first-boot script writes `~/.config/sway/config` (`configs/sway-config.sh`). Not yet resolved or install-tested | ~1.1 GiB (estimate) | 10 GiB+ |
| `archstaller-hyprland.iso` | Hyprland (Wayland) with kitty, rofi, waybar, quickshell, hyprlock/hypridle, Neovim (LazyVim) and Nerd fonts (FantasqueSansM, JetBrains Mono, Iosevka), plus the downloaded config zip | ~1.4 GiB | 15 GiB+ |
| `archstaller-plasma.iso` | KDE Plasma (Wayland session), same extras | ~1.3 GiB | 20 GiB+ |
| `archstaller-omarchy.iso` | Hyprland with the application set of Omarchy v4.0.4, official-repository packages only (not Omarchy itself: no Omarchy scripts, themes or dotfiles; the omitted packages are listed in `configs/omarchy.lua`). Installs and boots to the login prompt in QEMU | ~2.4 GiB | 24 GiB+ |

Every preset installs the `ly` login manager (`ly@tty2.service`) and creates the user `passwd_is_passwd`
with the password `passwd` in group `wheel` (sudo works; root is locked). **Change that password** before
the machine is reachable from a network, especially with the server preset, which enables SSH. On the
console-only presets (minimal, server) `ly` lists both `shell` and `xinitrc` sessions; pick `shell`,
because `xinitrc` needs X and `xauth`. The shared defaults live in `configs/common.lua`; `cargo xtask
check-presets` resolves every preset against your local pacman sync databases and reports package counts,
download sizes and provider choices. All presets install the wired-NIC firmware
(`linux-firmware-intel`, `linux-firmware-realtek`) and the AMD GPU firmware (`linux-firmware-amdgpu`,
`linux-firmware-radeon`, about 30 MiB) or, on the desktop presets, the full `linux-firmware`. The first boot
builds the initramfs without the `kms` hook, so a GPU that cannot initialise (missing firmware, an unsupported
chip) does not stop the boot before the root file system is mounted.

The Hyprland preset also pulls a Hyprland config from a server during installation: `FRIEND_CONFIG` at the
top of `configs/hyprland.lua` is the `https://` URL of a zip whose contents are laid out relative to the
home directory (`.config/hypr/hyprland.lua`, `.config/hypr/modules/...`). It is extracted into every user's
home on first boot, as that user. Set it to an empty string to skip it. If the server is down, or does not
answer with a zip, the archive is skipped with a warning and Hyprland keeps its defaults. **Only point it at a
server you trust**: a Hyprland config can run arbitrary commands when the session starts, and the archive is
checked by nothing but HTTPS. The preset installs the programs the current config launches (kitty, rofi,
waybar, quickshell, awww, swaync, swayosd, hyprlock, hypridle, cliphist, Neovim with the tools LazyVim needs,
and the three Nerd fonts). Things the config refers to that are **not** installed or not in the zip: a
quickshell config, `zen-browser` and the `macOS` cursor theme (AUR only; Firefox is installed instead),
`code`, `kitty-themes`, the wallpaper `~/Images/Wallpapers/special.jpg` and `~/.local/bin/satty-screenshot`.

`configs/tester.lua` is not an installer: it is a read-only hardware test (`archstaller-tester.iso`). It probes the
machine, brings up the network, downloads the package databases, pings the gateway and 1.1.1.1, runs a download speed
test, prints PASS/FAIL with full debug output, and reboots. It never writes a disk. Use it to check whether a machine's
NIC, tethering phone or dongle works before installing.

The presets use `disk.auto_largest = true`. For a machine with several disks, use `disk.confirm_serial`
in your own config instead (see below).
