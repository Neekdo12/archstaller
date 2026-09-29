-- Example archstaler config. Evaluated on the host at ISO build time.
-- Generate password hashes with:  openssl passwd -6
return {
  hostname = "archbox",
  timezone = "Europe/Prague",
  locale = "en_US.UTF-8",
  keymap = "us",

  disk = {
    -- model = "...",           -- optional substring of the disk model (with confirm_serial)
    auto_largest = true,         -- ERASE and install onto the largest disk, no questions asked
    -- confirm_serial = "...",    -- safer alternative: only this exact disk may be erased
    esp_mib = 1024,
  },

  mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },

  packages = { "base", "linux", "mkinitcpio", "limine", "efibootmgr", "openssh", "sudo" },
  providers = { initramfs = "mkinitcpio" },

  root_password_hash = nil,
  users = {
    { name = "arch", password_hash = "$6$example$0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123", groups = { "wheel" }, shell = "/bin/bash" },
  },

  services = { "systemd-networkd.service", "systemd-resolved.service", "sshd.service" },
  kernel_params = {},
}
