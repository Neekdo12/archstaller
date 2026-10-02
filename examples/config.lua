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

  packages = { "base", "linux", "mkinitcpio", "grub", "efibootmgr", "openssh", "sudo", "ly" },
  providers = { initramfs = "mkinitcpio" },

  root_password_hash = nil,
  -- Default login: user "passwd_is_passwd", password "passwd" (change it before real use!).
  users = {
    { name = "passwd_is_passwd", password_hash = "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.", groups = { "wheel" }, shell = "/bin/bash" },
  },

  services = { "systemd-networkd.service", "systemd-resolved.service", "sshd.service", "ly@tty2.service" },
  kernel_params = {},
}
