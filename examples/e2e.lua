-- Configuration for `cargo xtask e2e`: a minimal system installed onto the QEMU test disk.
return {
  hostname = "archbox",
  timezone = "Europe/Prague",
  locale = "en_US.UTF-8",
  keymap = "us",

  disk = { confirm_serial = "TESTDISK0", esp_mib = 512 },

  mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },

  packages = { "base", "linux", "mkinitcpio", "efibootmgr", "ly" },
  providers = { initramfs = "mkinitcpio" },

  root_password_hash = "$6$archstlr$yXwFAPpE77sWyJ5zOdpP8YRiKPoEJi2Biqa3t8FQG4vT36whLEHaMrWwdZeuajixHUdgq4OX5TpTCxivgT0/n0",
  users = {
    { name = "arch", password_hash = "$6$archstlr$yXwFAPpE77sWyJ5zOdpP8YRiKPoEJi2Biqa3t8FQG4vT36whLEHaMrWwdZeuajixHUdgq4OX5TpTCxivgT0/n0", groups = { "wheel" }, shell = "/bin/bash" },
  },

  services = { "systemd-networkd.service", "ly@tty2.service" },
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
}
