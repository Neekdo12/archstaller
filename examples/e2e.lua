-- Configuration for `cargo xtask e2e`: a minimal system installed onto the QEMU test disk.
return {
  hostname = "archbox",
  timezone = "Europe/Prague",
  locale = "en_US.UTF-8",
  keymap = "us",

  disk = { confirm_serial = "TESTDISK0", esp_mib = 512 },

  mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },

  packages = { "base", "linux", "mkinitcpio", "efibootmgr" },
  providers = { initramfs = "mkinitcpio" },

  root_password_hash = "$6$archstlr$yXwFAPpE77sWyJ5zOdpP8YRiKPoEJi2Biqa3t8FQG4vT36whLEHaMrWwdZeuajixHUdgq4OX5TpTCxivgT0/n0",
  users = {
    { name = "arch", password_hash = "$6$archstlr$yXwFAPpE77sWyJ5zOdpP8YRiKPoEJi2Biqa3t8FQG4vT36whLEHaMrWwdZeuajixHUdgq4OX5TpTCxivgT0/n0", groups = { "wheel" }, shell = "/bin/bash" },
  },

  services = { "systemd-networkd.service" },
  kernel_params = { "console=ttyS0,115200" },
}
