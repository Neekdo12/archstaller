-- archstaller: kind=preset
-- Minimal: console-only system with networkd. Needs a disk of about 4 GiB or more.
local common = require("common")

return common.build{
  hostname = "minimal",
  packages = {},
  services = { "systemd-networkd.service", "systemd-resolved.service" },
}
