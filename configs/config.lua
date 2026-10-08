-- archstaller: kind=example
-- Example archstaller config. Evaluated on the host at ISO build time.
-- Generate password hashes with:  openssl passwd -6
---@type AsConfig
local as = {
  schema = 1,

  system = {
    hostname = "archbox",
    timezone = "Europe/Prague",
    locale = "en_US.UTF-8",
    keymap = "us",
    -- root_password_hash = "$6$...",   -- leave it out to keep root locked
  },

  install = {
    disk = {
      -- model = "...",             -- optional substring of the disk model (with confirm_serial)
      auto_largest = true,          -- ERASE and install onto the largest disk, no questions asked
      -- confirm_serial = "...",    -- safer alternative: only this exact disk may be erased
      esp_mib = 1024,
    },
    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  },

  packages = {
    explicit = { "base", "linux", "mkinitcpio", "grub", "efibootmgr", "openssh", "sudo", "ly" },
    providers = { initramfs = "mkinitcpio" },
  },

  -- Default login: user "passwd_is_passwd", password "passwd" (change it before real use!).
  users = {
    { name = "passwd_is_passwd", password_hash = "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.", groups = { "wheel" }, shell = "/bin/bash" },
  },

  first_boot = {
    services = { "systemd-networkd.service", "systemd-resolved.service", "sshd.service", "ly@tty2.service" },
    kernel_params = {},
  },

  -- Build-host settings (not part of what the installer receives):
  build = {
    profile = "super-small",  -- "super-small" (default, smallest installer) or "large" (keeps panic messages)
    tethering = false,        -- include USB tethering (iPhone, Android, USB Ethernet) in the installer
    -- Installer drivers to include; leave it out for all of them. Needs at least one storage and one
    -- network driver (or tethering). Ids: virtio-blk ahci ata nvme virtio-net e1000 igb r8169 rtl8139 alx vmxnet3
    -- installer_drivers = { "virtio-blk", "ahci", "nvme", "virtio-net", "e1000" },
  },
}

return { as = as }
