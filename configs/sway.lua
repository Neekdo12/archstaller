-- archstaler: kind=preset
-- Sway (Wayland) with waybar, wofi, foot, NetworkManager, PipeWire and Firefox; ly starts the session. 10 GiB+ disk.
-- Needs a GPU with working KMS; in a VM enable 3D acceleration (virtio-gpu with virgl).
local common = require("common")

return common.build{
  hostname = "sway",
  packages = {
    -- session
    "sway", "xorg-xwayland", "mesa", "polkit",
    -- bar, launcher, terminal, notifications (used by the shipped config)
    "waybar", "otf-font-awesome", "wofi", "foot", "mako",
    -- locking, idle, wallpaper
    "swaylock", "swayidle", "swaybg",
    -- portals (screen sharing, file pickers)
    "xdg-desktop-portal", "xdg-desktop-portal-wlr", "xdg-desktop-portal-gtk",
    -- clipboard, screenshots, brightness and media keys (bound in the shipped config)
    "wl-clipboard", "grim", "slurp", "brightnessctl", "playerctl",
    "networkmanager", "pipewire", "pipewire-pulse", "wireplumber",
    "ttf-dejavu", "noto-fonts", "firefox",
  },
  firmware = { "linux-firmware" },
  providers = { jack = "pipewire-jack" },
  services = { "NetworkManager.service" },
  -- Writes ~/.config/sway/config for every user (and /etc/skel) unless one is already there.
  scripts = { { id = "sway-config", file = "sway-config.sh" } },
}
