-- Shared defaults for the presets. A preset calls  common.build{ packages = {...}, ... }.
local M = {}

local function concat(...)
  local out = {}
  for _, list in ipairs({ ... }) do
    for _, v in ipairs(list or {}) do out[#out + 1] = v end
  end
  return out
end

-- Default login for every preset: user "passwd_is_passwd", password "passwd".
-- CHANGE IT before this system is reachable from any network (generate a hash with
-- `openssl passwd -6`). root stays locked; the user is in "wheel" and may use sudo.
local DEFAULT_HASH = "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc."

function M.build(spec)
  return {
    hostname = spec.hostname or "laidiota",
    timezone = "Europe/Prague",
    locale = "en_US.UTF-8",
    keymap = "us",

    -- WARNING: erases and installs onto the LARGEST disk, without asking.
    disk = { auto_largest = true, esp_mib = 1024 },

    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },

    packages = concat(
      { "base", "linux", "mkinitcpio", "efibootmgr", "sudo", "nano", "ly" },
      -- Wired NIC firmware, plus the AMD GPU firmware (small: about 30 MiB; without it amdgpu/radeon fail to
      -- initialise on AMD graphics). Desktops pass the full linux-firmware instead.
      spec.firmware or { "linux-firmware-intel", "linux-firmware-realtek", "linux-firmware-amdgpu", "linux-firmware-radeon" },
      spec.packages
    ),
    providers = concat({ { "initramfs", "mkinitcpio" } }, spec.providers),

    root_password_hash = nil,
    users = {
      { name = "passwd_is_passwd", password_hash = DEFAULT_HASH, groups = { "wheel" }, shell = "/bin/bash" },
    },

    services = concat({ "systemd-timesyncd.service", "ly@tty2.service" }, spec.services),
    kernel_params = spec.kernel_params or {},
    user_files = spec.user_files or {},
    user_archives = spec.user_archives or {},
    dry_run = spec.dry_run or false,
  }
end

return M
