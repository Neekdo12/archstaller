-- Server: minimal plus OpenSSH and a few admin tools.
-- WARNING: sshd is enabled and the default login is passwd_is_passwd / passwd. Change the
-- password (or replace the hash in presets/common.lua) before connecting this to a network.
local common = dofile(CONFIG_DIR .. "/common.lua")

return common.build{
  hostname = "server",
  packages = { "openssh", "htop", "tmux", "rsync", "vim" },
  services = { "systemd-networkd.service", "systemd-resolved.service", "sshd.service" },
}
