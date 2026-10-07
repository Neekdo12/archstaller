-- KDE Plasma (Wayland session) with NetworkManager, PipeWire and Firefox; ly starts the session.
-- The biggest preset (well over 1 GiB to download); use a disk of 20 GiB or more.
local common = dofile(CONFIG_DIR .. "/common.lua")

return common.build{
  hostname = "plasma",
  packages = {
    "plasma-desktop", "plasma-nm", "plasma-pa", "kscreen", "konsole", "dolphin", "kate",
    "polkit", "mesa", "xorg-xwayland",
    "networkmanager", "pipewire", "pipewire-pulse", "wireplumber",
    "ttf-dejavu", "noto-fonts", "firefox",
  },
  firmware = { "linux-firmware" },
  providers = { { "jack", "pipewire-jack" } },
  services = { "NetworkManager.service" },
}
