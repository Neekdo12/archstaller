-- Tester: minimal console system that checks its own installation. On the boot of the installed
-- system, archstaler-selftest.service prints a PASS/FAIL report (users, packages, services, mounts,
-- journal, network, ...) to the console and reboots about 60 seconds later.
-- WARNING: it reboots forever. With auto_largest, pull the installer medium (or change the boot
-- order) before the first reboot of the installed system, or the ISO will erase it again.
-- Needs a disk of about 4 GiB or more.
local common = dofile(CONFIG_DIR .. "/common.lua")

return common.build{
  hostname = "tester",
  packages = { "networkmanager", "curl" },
  services = { "NetworkManager.service", "archstaler-selftest.service" },
  -- Report on the screen and on the serial port (harmless where there is no serial port).
  kernel_params = { "console=tty0", "console=ttyS0,115200" },
}
