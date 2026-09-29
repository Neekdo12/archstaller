-- Minimal: console-only system with networkd. Needs a disk of about 4 GiB or more.
local common = dofile(CONFIG_DIR .. "/common.lua")

return common.build{
  hostname = "minimal",
  packages = {},
  services = { "systemd-networkd.service", "systemd-resolved.service" },
}
