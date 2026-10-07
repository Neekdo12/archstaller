-- archstaler: kind=preset
-- Hyprland (Wayland) with NetworkManager, PipeWire and Firefox; ly starts the session.
-- Needs a GPU with working KMS; in a VM enable 3D acceleration (virtio-gpu with virgl). 15 GiB+ disk.
local common = require("common")

-- A friend's Hyprland config: a zip with the config laid out relative to the home directory
-- (.config/hypr/hyprland.lua, .config/hypr/modules/..., and so on). The installer downloads it and
-- the first boot extracts it into every user's home, owned by that user. Leave empty to skip.
-- If the server is unreachable at install time, or the answer is not a zip, the archive is skipped
-- with a warning and the installation carries on with Hyprland's defaults.
-- SECURITY: a Hyprland config can run arbitrary commands when the session starts, so only point
-- this at a server you trust; the archive is not verified in any other way than HTTPS.
local FRIEND_CONFIG = "https://arch.laidiota.party/config_extended.zip"

return common.build{
  hostname = "hyprland",
  packages = {
    -- session
    "hyprland", "hyprlock", "hypridle", "xdg-desktop-portal-hyprland", "polkit", "mesa",
    "networkmanager", "pipewire", "pipewire-pulse", "wireplumber", "qt6ct",
    -- terminal, launcher, bar, shell, wallpaper
    "kitty", "rofi", "rofi-calc", "wofi", "waybar", "quickshell", "awww",
    -- clipboard, notifications, on-screen display, screenshots, media keys (used by the key bindings)
    "wl-clipboard", "wtype", "cliphist", "swaync", "swayosd", "hyprshot", "hyprshutdown", "satty",
    "playerctl", "brightnessctl", "jq", "yazi",
    -- Neovim (LazyVim) and what it needs to install plugins and parsers
    "neovim", "git", "ripgrep", "fd", "gcc", "tree-sitter-cli", "nodejs", "npm", "unzip",
    -- fonts named in the config: FantasqueSansM, JetBrains Mono, Iosevka (all Nerd Font builds)
    "ttf-fantasque-nerd", "ttf-jetbrains-mono-nerd", "ttf-iosevka-nerd", "ttf-dejavu", "noto-fonts",
    "firefox",
  },
  firmware = { "linux-firmware" },
  providers = { jack = "pipewire-jack" },
  services = { "NetworkManager.service" },
  user_archives = FRIEND_CONFIG ~= "" and { { url = FRIEND_CONFIG } } or {},
}
