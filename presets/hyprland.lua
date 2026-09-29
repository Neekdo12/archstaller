-- Hyprland (Wayland) with NetworkManager, PipeWire and Firefox; ly starts the session.
-- Needs a GPU with working KMS; in a VM enable 3D acceleration (virtio-gpu with virgl). 10 GiB+.
local common = dofile(CONFIG_DIR .. "/common.lua")

-- A friend's Hyprland config, downloaded during installation into every user's
-- ~/.config/hypr/hyprland.lua. Leave empty to skip. If the server is unreachable at install
-- time the file is skipped with a warning and the installation carries on.
-- SECURITY: a Hyprland config can run arbitrary commands when the session starts, so only
-- point this at a server you trust; the file is not verified in any other way.
local FRIEND_CONFIG = ""  -- e.g. "https://arch.laidiota.party/configs/NAME.lua"

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
  user_files = FRIEND_CONFIG ~= "" and { { url = FRIEND_CONFIG, dest = ".config/hypr/hyprland.lua" } } or {},
}
