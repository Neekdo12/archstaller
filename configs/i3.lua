-- archstaler: kind=preset
-- i3 on Xorg with NetworkManager, PipeWire and Firefox; ly starts the session. 10 GiB+ disk.
local common = require("common")

return common.build{
  hostname = "i3",
  packages = {
    "xorg-server", "xorg-xinit", "xf86-input-libinput", "mesa",
    "i3-wm", "i3status", "i3lock", "dmenu", "alacritty",
    "networkmanager", "pipewire", "pipewire-pulse", "wireplumber",
    "ttf-dejavu", "noto-fonts", "firefox",
  },
  firmware = { "linux-firmware" },
  providers = { jack = "pipewire-jack" },
  services = { "NetworkManager.service" },
}
