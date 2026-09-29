-- Example archstaler config. Evaluated on the host at ISO build time.
return {
  hostname = "archbox",
  timezone = "Europe/Prague",
  locale = "en_US.UTF-8",
  keymap = "us",
  disk = {
    model = "QEMU",
    confirm_serial = "CHANGE-ME",
  },
  packages = { "base", "linux", "linux-firmware", "limine", "efibootmgr" },
}
