-- Hyprland (Wayland) with NetworkManager, PipeWire and Firefox; ly starts the session.
-- Needs a GPU with working KMS; in a VM enable 3D acceleration (virtio-gpu with virgl). 10 GiB+.
local common = dofile(CONFIG_DIR .. "/common.lua")

return common.build{
  hostname = "hyprland",
  packages = {
    "hyprland", "kitty", "wofi", "waybar", "polkit", "mesa",
    "networkmanager", "pipewire", "pipewire-pulse", "wireplumber",
    "xdg-desktop-portal-hyprland", "ttf-dejavu", "noto-fonts", "firefox",
  },
  firmware = { "linux-firmware" },
  providers = { { "jack", "pipewire-jack" } },
  services = { "NetworkManager.service" },
}
