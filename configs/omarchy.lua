-- archstaler: kind=preset
-- Omarchy package set on Hyprland, from the official Arch repositories only (a subset of Omarchy, see below).
--
-- Derived from Omarchy v4.0.4 (basecamp/omarchy, install/omarchy-base.packages plus the generic entries of
-- install/omarchy-other.packages). This is NOT Omarchy: it has no Omarchy scripts, themes or dotfiles and does
-- not run Omarchy's installer. It is the same application set on Archstaler's own Arch pipeline: linux,
-- mkinitcpio and grub as in every preset, and ly as the login manager (pick the Hyprland session).
--
-- Left out on purpose:
--   * sddm: ly is the login manager here (two display managers would fight over the seat).
--   * plymouth: boot splash needs an initramfs hook that the first boot does not configure.
--   * limine, snapper, linux-omarchy and the other hardware-specific entries (NVIDIA, Apple T2, Asus, ...).
--   * Not in core/extra, so the installer cannot fetch them (Omarchy ships them from its own repository or the AUR):
--     aether, asdcontrol, cliamp, herdr, hyprland-preview-share-picker, localsend, mise-bin, nvim,
--     omacalc, omacut, omarchy-nvim, omawrite, tensaku, tobi-try, ttf-ia-writer,
--     ttf-jetbrains-mono-nerd-basic, ttfx, tzupdate, ufw-docker, yaru-icon-theme, yay
-- Services are enabled only for NetworkManager, Bluetooth and power profiles; Docker, CUPS, ufw and the rest are
-- installed but not enabled. 20 GiB+ disk, a GPU with working KMS, a large download.
-- SECURITY: the default login is passwd_is_passwd / passwd (see common.lua); change it before connecting to a network.
local common = require("common")

return common.build{
  hostname = "omarchy",
  packages = {
    "alsa-utils", "avahi", "base-devel", "bash-completion", "bat", "bluez", "bluez-tools", "bluez-utils", "bolt",
    "brightnessctl", "btop", "chromium", "clang", "cups", "cups-filters", "cups-pk-helper", "ddcutil", "docker",
    "docker-buildx", "docker-compose", "dosfstools", "dotnet-runtime", "dua-cli", "evince", "exfatprogs", "expac",
    "eza", "fakeroot", "fastfetch", "fcitx5", "fcitx5-gtk", "fcitx5-qt", "fd", "ffmpegthumbnailer", "fontconfig",
    "foot", "fzf", "git", "gnome-disk-utility", "gnome-keyring", "gnome-themes-extra", "gpu-screen-recorder",
    "grim", "gst-plugin-pipewire", "gtk4-layer-shell", "gum", "gvfs-mtp", "gvfs-nfs", "gvfs-smb", "hyprland",
    "hyprland-guiutils", "hyprpicker", "hyprsunset", "imagemagick", "imv", "inetutils", "inotify-tools", "inxi",
    "jq", "kdenlive", "kernel-modules-hook", "lazydocker", "lazygit", "less", "libpulse", "libreoffice-fresh",
    "libsecret", "libvips", "libyaml", "llvm", "lua51", "luarocks", "man-db", "mariadb-libs", "mesa",
    "moonlight-qt", "mpv", "mpv-mpris", "nautilus", "nautilus-python", "networkmanager", "noto-fonts",
    "noto-fonts-cjk", "noto-fonts-emoji", "nss-mdns", "obsidian", "obs-studio", "pacman-contrib", "pamixer",
    "pinta", "pipewire", "pipewire-alsa", "pipewire-pulse", "plocate", "polkit", "postgresql-libs",
    "power-profiles-daemon", "python-gobject", "python-poetry-core", "qemu-user-static-binfmt", "qrencode",
    "qt6-imageformats", "quickshell", "ripgrep", "ruby", "slurp", "socat", "sof-firmware", "starship", "sushi",
    "system-config-printer", "tesseract", "tesseract-data-eng", "tldr", "tmux", "tree-sitter-cli", "udiskie",
    "ufw", "unzip", "usage", "uwsm", "vulkan-intel", "vulkan-radeon", "webp-pixbuf-loader", "whois",
    "wireless-regdb", "wireplumber", "wl-clipboard", "woff2-font-awesome", "wtype", "xdg-desktop-portal-gtk",
    "xdg-desktop-portal-hyprland", "xdg-terminal-exec", "xournalpp", "yt-dlp", "zbar", "zoxide",
  },
  firmware = { "linux-firmware" },
  providers = { jack = "pipewire-jack", ["qt6-multimedia-backend"] = "qt6-multimedia-ffmpeg" },
  services = { "NetworkManager.service", "bluetooth.service", "power-profiles-daemon.service" },
}
