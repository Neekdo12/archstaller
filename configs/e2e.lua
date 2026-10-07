-- archstaler: kind=e2e
-- Configuration for `cargo xtask e2e`: a minimal system installed onto the QEMU test disk.
---@type AsConfig
local as = {
  schema = 1,

  system = {
    hostname = "archbox",
    timezone = "Europe/Prague",
    locale = "en_US.UTF-8",
    keymap = "us",
    root_password_hash = "$6$archstlr$yXwFAPpE77sWyJ5zOdpP8YRiKPoEJi2Biqa3t8FQG4vT36whLEHaMrWwdZeuajixHUdgq4OX5TpTCxivgT0/n0",
  },

  install = {
    disk = { confirm_serial = "TESTDISK0", esp_mib = 512 },
    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  },

  packages = {
    explicit = { "base", "linux", "mkinitcpio", "grub", "efibootmgr", "ly" },
    providers = { initramfs = "mkinitcpio" },
  },

  users = {
    { name = "arch", password_hash = "$6$archstlr$yXwFAPpE77sWyJ5zOdpP8YRiKPoEJi2Biqa3t8FQG4vT36whLEHaMrWwdZeuajixHUdgq4OX5TpTCxivgT0/n0", groups = { "wheel" }, shell = "/bin/bash" },
  },

  first_boot = {
    services = { "systemd-networkd.service", "ly@tty2.service" },
    -- Exercises the first-boot script runner.
    scripts = { { id = "enable-fstrim" } },
    kernel_params = { "console=ttyS0,115200" },
    -- Exercises both paths: a file that downloads, and a server that does not exist (must only warn).
    user_archives = {
      { url = "https://arch.laidiota.party/config.zip" },
      { url = "https://nonexistent.invalid/bad.zip" },
      { url = "https://raw.githubusercontent.com/hyprwm/Hyprland/main/README.md" }, -- 200, but not a zip
    },
    user_files = {
      { url = "https://raw.githubusercontent.com/hyprwm/Hyprland/main/README.md", dest = ".config/test/good.lua" },
      { url = "https://nonexistent.invalid/bad.lua", dest = ".config/test/bad.lua" },
    },
  },

  -- Keep panic messages for debugging the installer under test (the default profile is "super-small").
  build = { profile = "large" },
}

return { as = as }
