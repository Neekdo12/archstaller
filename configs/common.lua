-- archstaller: kind=module
-- Shared defaults for the presets. A preset calls  common.build{ packages = {...}, ... }  and returns
-- the result, a complete `{ as = ... }` config.
local M = {}

---Concatenates lists into a new list; nil entries are skipped.
---@param ... string[]|nil
---@return string[]
local function concat(...)
  local out = {}
  for _, list in ipairs({ ... }) do
    for _, v in ipairs(list or {}) do out[#out + 1] = v end
  end
  return out
end

---Copies `base` and applies `over` on top: a key in `over` wins.
---@param base table<string, string>
---@param over table<string, string>|nil
---@return table<string, string>
local function merge(base, over)
  local out = {}
  for k, v in pairs(base) do out[k] = v end
  for k, v in pairs(over or {}) do out[k] = v end
  return out
end

-- Default login for every preset: user "passwd_is_passwd", password "passwd".
-- CHANGE IT before this system is reachable from any network (generate a hash with
-- `openssl passwd -6`). root stays locked; the user is in "wheel" and may use sudo.
local DEFAULT_HASH = "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc."

---What a preset may say. Lists are appended to the defaults; `providers` entries override them.
---@class CommonSpec
---@field hostname? string
---@field packages string[]
---@field firmware? string[] replaces the default firmware packages
---@field providers? table<string, string>
---@field services? string[]
---@field kernel_params? string[]
---@field user_files? AsUserFile[]
---@field user_archives? AsUserArchive[]
---@field scripts? AsScript[]
---@field dry_run? boolean
---@field build? AsBuild

---@param spec CommonSpec
---@return AsEnvelope
function M.build(spec)
  ---@type AsConfig
  local as = {
    schema = 1,
    system = {
      hostname = spec.hostname or "laidiota",
      timezone = "Europe/Prague",
      locale = "en_US.UTF-8",
      keymap = "us",
    },
    install = {
      -- WARNING: erases and installs onto the LARGEST disk, without asking.
      disk = { auto_largest = true, esp_mib = 1024 },
      mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
      dry_run = spec.dry_run or false,
    },
    packages = {
      explicit = concat(
        { "base", "linux", "mkinitcpio", "grub", "efibootmgr", "sudo", "nano", "ly" },
        -- Wired NIC firmware, plus the AMD GPU firmware (small: about 30 MiB; without it amdgpu/radeon fail to
        -- initialise on AMD graphics). Desktops pass the full linux-firmware instead.
        spec.firmware or { "linux-firmware-intel", "linux-firmware-realtek", "linux-firmware-amdgpu", "linux-firmware-radeon" },
        spec.packages
      ),
      providers = merge({ initramfs = "mkinitcpio" }, spec.providers),
    },
    users = {
      { name = "passwd_is_passwd", password_hash = DEFAULT_HASH, groups = { "wheel" }, shell = "/bin/bash" },
    },
    first_boot = {
      services = concat({ "systemd-timesyncd.service", "ly@tty2.service" }, spec.services),
      kernel_params = spec.kernel_params or {},
      user_files = spec.user_files or {},
      user_archives = spec.user_archives or {},
      scripts = spec.scripts or {},
    },
    build = spec.build or {},
  }
  return { as = as }
end

return M
