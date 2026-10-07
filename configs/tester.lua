-- Tester: READ-ONLY hardware test. Boots the installer kernel in dry-run mode: probes PCI, reads
-- every disk (writes are refused by the kernel), brings up the network, downloads and resolves the
-- package databases, prints a PASS/FAIL report and reboots after 60 seconds.
-- It installs nothing and needs no disk selector: no disk is ever written.
return {
  -- Hardware test: keep panic messages (the default profile is "super-small").
  build = { profile = "large" },
  hostname = "tester",
  timezone = "Europe/Prague",
  locale = "en_US.UTF-8",
  keymap = "us",
  dry_run = true,

  disk = { esp_mib = 1024 },
  mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  -- Only resolved, never downloaded.
  packages = { "base", "linux", "mkinitcpio", "grub", "efibootmgr", "sudo", "nano" },
  providers = { { "initramfs", "mkinitcpio" } },
  users = {},
  services = {},
  kernel_params = {},
}
