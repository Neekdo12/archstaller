# Typed Lua config API

**Status: implemented.** The `as` loader (`crates/hostcfg/src/asconfig.rs`, `lua.rs`), the writer, the flat
`configs/` directory with first-line markers (`crates/hostcfg/src/configs.rs`), the generated LuaLS types
(`configs/archstaler.lua`, `cargo xtask gen-luals [--check]`) and `.luarc.json` exist, and every repository
config uses the new layout. Decisions the text below left open:

- The open question is settled as recommended: the legacy flat layout still loads with a warning
  (`hostcfg::lua::LEGACY_WARNING`); the writer emits only `as`; mixing both in one file is an error.
- Placement of fields the example did not show: `system.root_password_hash`, `install.dry_run`,
  `packages.aur`, `first_boot.user_files`, `first_boot.user_archives`, `build.installer_drivers`.
- `packages.providers` is a table; it is normalized and written sorted by dependency name.
- Preset marker: `-- archstaler: kind=preset|example|e2e|module` on line 1 (no manifest); the next comment line
  is the description. A file with no marker is not listed anywhere.
- Error paths count list positions from 1 (`as.users[1].groups`), like Lua and the existing domain messages.
- `presets/common.lua` became `configs/common.lua`, a documented builder that returns a complete `{ as = ... }`.
- Editor diagnostics were verified with `lua-language-server` 3.19.1 (`--check`): the repository's configs
  report no problems, and a config with a wrong value type, a missing required field and an invalid profile
  reports errors. A misspelled key is reported as the missing required field. `lua-language-server` is not part
  of `cargo test`; the generated file is covered by a test against the schema. README.md has the VS Code and
  Neovim/LazyVim setup.

This spec describes a new host-side Lua config model inspired by the structure of the local
`~/.config/hypr/` setup: one root file composes focused modules, and settings live under a single
namespace. Use `as` for Archstaler. The goal is convenient hand editing with useful Lua Language Server
diagnostics in VS Code and Neovim/LazyVim, while the CLI remains the final authority for accepting a
config.

This remains Lua, not a new DSL. Lua is dynamically typed; editor diagnostics catch mistakes while
editing, and strict Rust-side deserialization/validation must reject them at build time even when a
config is run outside an editor or has diagnostics disabled.

## Current model and migration boundary

Today `examples/config.lua` returns a flat table. `crates/hostcfg/src/lua.rs` evaluates the file with
`mlua`, then deserializes the same table separately into `config::Config` and `HostConfig`. The
installer-facing `Config` is shared with the kernel and serialized into `config.bin`. The deterministic
writer in `crates/hostcfg/src/writer.rs` emits the same flat shape.

The new Lua input should have one `as` root table and explicit sections for system, installation,
packages, users/first boot, and build-host options. For example:

```lua
---@type AsConfig
local as = {
  schema = 1,
  system = {
    hostname = "archbox",
    timezone = "Europe/Prague",
    locale = "en_US.UTF-8",
    keymap = "us",
  },
  install = {
    disk = {
      confirm_serial = "SERIAL-123",
      auto_largest = false,
      esp_mib = 1024,
    },
    mirrors = { "https://geo.mirror.pkgbuild.com/$repo/os/$arch" },
  },
  packages = {
    explicit = { "base", "linux", "mkinitcpio", "grub" },
    providers = { initramfs = "mkinitcpio" },
  },
  users = {},
  first_boot = {
    services = { "systemd-networkd.service" },
    kernel_params = {},
    scripts = {},
  },
  build = {
    profile = "super-small",
    tethering = false,
  },
}

return { as = as }
```

The final returned value has exactly one application namespace, `as`. The source-level local variable is
also named `as` so it reads naturally in config files. Use the existing Rust config types and validation
as the source for field meanings and constraints; the example above illustrates organization, not a
replacement schema.

## One config directory and modular organization

Consolidate user-facing examples, e2e configs, shared helpers, and selectable presets into one flat
top-level `configs/` directory. This is a target layout for the implementation; this spec does not
move files or change current paths. Keep the Lua files directly in that directory rather than splitting
them into separate `examples/`, `presets/`, or nested module directories:

```text
configs/
  config.lua             # documented default/example config, used by default CLI build
  e2e.lua                # serial-locked config for automated install testing
  common.lua             # shared helper, not a selectable preset
  archstaler.lua         # LuaLS API/type definitions, not a selectable preset
  minimal.lua            # selectable preset
  server.lua             # selectable preset
  hyprland.lua           # selectable preset
  ...
.luarc.json              # shared LuaLS project settings at repository root
```

Follow the useful part of the Hyprland pattern: a small entry file composes focused modules with
`require`, and modules own cohesive pieces of configuration. Do not reproduce the Hyprland global
mutable API or add side-effecting global configuration calls. Keep modules flat beside their entry
configs and return typed data sections/fragments that the root file places under `as`:

```lua
---@type AsConfig
local as = {
  schema = 1,
  system = require("system"),
  install = require("install"),
  packages = require("packages"),
  users = require("users"),
  first_boot = require("first_boot"),
  build = { profile = "super-small", tethering = false },
}

return { as = as }
```

Modules must be ordinary Lua modules that return data, not mutate shared globals. Presets may compose
shared module data and override explicit fields, but the merge behavior must be documented and
deterministic. Avoid hidden environment state, metatable magic, implicit filesystem scans, and
configuration values injected through globals. Set the Lua module path so `require("common")` and
other sibling modules resolve relative to `configs/`, while retaining `CONFIG_DIR` for compatibility
with existing `dofile(CONFIG_DIR .. "/common.lua")` modules during migration.

### Config and preset discovery

- Put default/example configs, e2e configs, shared helpers, LuaLS definitions, and presets in the same
  `configs/` directory. Do not treat every `*.lua` file there as a preset.
- Mark selectable preset entry files explicitly, preferably with a first-line metadata comment such as
  `-- archstaler: kind=preset` plus an optional display name/description, or use one explicit manifest
  in the same directory. Choose one source of truth and have both `xtask` and the GUI consume it.
- Mark `config.lua` as the default example, `e2e.lua` as an automated test config, and helpers/type
  definition files as non-presets. The preset build/check commands and GUI preset menu must list only
  entries marked as presets.
- Derive `CONFIG_DIR` from the currently loaded file as today, so modules continue working when the
  config is opened from another path. Repository defaults should resolve to `<repo>/configs/`.
- Update the CLI default build path, `xtask e2e` default/test paths, preset build/check discovery,
  GUI preset discovery, `Model::from_preset` documentation, and all user-facing command examples as
  one migration. Do not copy Lua files to both old and new folders as a long-term compatibility shim.

## Typed editor support

### LuaLS definitions

- Provide Lua Language Server annotations for the complete public API: exact classes for each `as`
  section, required/optional fields, list element types, integer fields, and string-literal aliases for
  bounded values such as build profiles. Include user-file, archive, user, provider, and script shapes.
- Generate the annotations from the Rust schema rather than hand-maintaining a second field list. The
  implementation may use JSON Schema or another deterministic intermediate representation, but it
  must preserve optionality, integer-vs-string distinctions, enum values, and nested types. Check the
  generated output into the repo and add a CI/test command that fails when regeneration changes it.
- Make the `as` root type exact so misspelled or unknown keys are editor diagnostics, not merely
  suggestions. Give module return tables the corresponding section type so extracted modules receive
  the same completion and type checking as the root file.
- Define the known Lua runtime version and module search paths in a repository `.luarc.json`. Include
  `configs/` in LuaLS's workspace library/package path so the API definition and sibling modules
  resolve in root configs and presets. Keep the configuration editor-neutral so both VS Code's Lua
  extension and `lua-language-server` in Neovim/LazyVim use the same project definitions.
- Document the minimal Neovim/LazyVim setup when the server does not automatically load project
  `.luarc.json`: configure the standard `lua_ls` server to use the workspace root and the same
  `configs/` library path. Do not require a custom Archstaler Neovim plugin.
- Enable diagnostics for undefined fields, invalid field types, missing required fields, and invalid
  function/module return types. Do not suppress these errors globally to make old config files appear
  clean.

Completion should feel like a quiet, useful nudge: offer field names, documented choices, and short
descriptions at the cursor; do not aggressively insert a guessed value. Package names can be suggested
from the active Arch sync database when available, but are inherently data-driven and cannot be a
closed static type union. Label package completion as suggestions; actual resolution remains the
existing `hostcfg` resolver's job.

### Runtime type and schema validation

- Deserialize one host-side `LuaConfig` envelope containing `as: AsConfig`, rather than deserializing
  the same flat table twice. Normalize the typed sections into the existing `config::Config` and
  `HostConfig` after the Lua evaluation succeeds.
- Reject unknown keys at every config-owned section, reject wrong Lua value types, and include the
  full path in errors (for example, `as.system.hostname: expected string, got integer`). Do not
  silently coerce numbers to strings, truncate floats to integers, or ignore misspelled fields.
- Keep domain validation in the existing Rust owners: `config::Config::validate`,
  `HostConfig::validate`, and `hostcfg::scripts::validate`. Extend those owners when a new field or
  constraint is introduced; do not implement a second validation policy in Lua or in an editor plugin.
- Keep `CONFIG_DIR` and any module-loading support as host-evaluation implementation details, not
  user-config fields. Run Lua with the existing host-only `mlua` runtime; never embed the config Lua
  interpreter into the installer kernel.
- Preserve the serialized `config::Config` wire format consumed by the kernel when possible. The
  `as` namespace is a host-side authoring model; it should normalize into the existing installer
  config so a Lua API redesign does not accidentally change `config.bin` or the kernel contract.

## Writer, CLI, GUI, and preset migration

1. Define the typed `AsConfig` DTO and mapping to/from the existing `Config` plus `HostConfig`. Add
   strict serde behavior without applying `deny_unknown_fields` independently to the existing split
   structs; the combined envelope owns the namespace and can reject unrecognized keys correctly.
2. Add error-path tests for wrong primitive types, unknown/misspelled fields, missing required values,
   invalid enum strings, and invalid nested list entries. Confirm an integer in a string field fails in
   both the editor diagnostics and host loader.
3. Implement deterministic annotation generation and `.luarc.json`. Add a check mode suitable for CI
   that verifies generated type files are current.
4. Update `hostcfg::lua::load` and `writer::to_lua` to consume and emit the new `return { as = ... }`
   format. Preserve deterministic ordering, comments that explain unsafe settings, and hash-only
   password storage.
5. Decide a transition for old flat-schema configs. Recommended: continue accepting the legacy shape for one
   migration window with a clear warning, but write only the new namespaced shape; migrate examples
  and repository presets immediately. Never ambiguously interpret a partially migrated document.
6. Consolidate `examples/*.lua` and `presets/*.lua` under `configs/` as part of the implementation.
  Update the default config and e2e paths, preset marker/manifest, Lua `require` paths, GUI
  open/save/preset loading, `xtask`, docs, and tests together. Presets should remain ordinary
  composable Lua modules and validate through the same loader as user configs.
7. Keep the existing GUI form model backed by normalized `Config`/`HostConfig`. The raw Lua editor
   should retain user-authored modules/source as source; form edits should serialize through the new
   writer. Make source/form mode transitions explicit to avoid losing comments or custom Lua.
8. Verify that a config written by the new writer reloads to equivalent normalized host and installer
   values, produces the same installer `config.bin` for equivalent inputs, and passes `cargo xtask
  check-presets` and the GUI round-trip tests. Verify that `check-presets` and the GUI menu omit
  `config.lua`, `e2e.lua`, shared helpers, and type-definition files.

## Compatibility and safety

- Keep disk selection semantics explicit in `as.install.disk`. In particular,
  `auto_largest = true` still means the installer erases the largest disk without asking; preserve the
  warning in generated examples and GUI preset review.
- Do not put plaintext passwords into the typed API. Accept only password hashes, or keep password
  creation in the GUI and serialize only the resulting hash.
- Keep build-host-only fields under `as.build`; they must not leak into the serialized installer config.
- Keep installer drivers distinct from installed-system packages, and expose driver IDs from the
  existing hostcfg catalogue for completion/validation.
- Existing `presets/common.lua` uses a helper builder and mutable spec input. Convert it deliberately
  to typed section-returning modules or a documented typed builder; do not let convenience helpers
  bypass final envelope validation.

## Open question

Should the new loader retain one-window read compatibility for existing flat configs, as recommended
above, or should the schema change be a hard break that requires every existing user config to be
migrated immediately?