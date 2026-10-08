# Config files and editor setup

Configs are Lua files evaluated on the **build host**; the result is serialized into the ISO. A config
returns one table with a single namespace, `as` (`return { as = as }`). `configs/config.lua` is a documented
example, `configs/e2e.lua` is the config of `xtask e2e`, and `configs/*.lua` marked
`-- archstaler: kind=preset` on their first line are the presets (`configs/common.lua` holds their shared
code and is not one). Modules beside a config load with `require("common")`.

`docs/lua-config.md` is the full reference for every setting, with checked examples; it is written to be pasted whole into a chatbot. All Lua files live flat in `configs/`. The Lua Language Server types (`configs/archstaler.lua`, generated from
the Rust schema by `cargo xtask gen-luals`; `--check` fails when it is stale) and the root `.luarc.json` give
completion and diagnostics for the `as` table in VS Code and Neovim/LazyVim (point `lua_ls` at the repository
root; it reads `.luarc.json`). The editor only helps while typing: the build rejects unknown keys and wrong
value types with the full path, for example `as.system.hostname: invalid type: integer 5, expected a string`
(list positions count from 1).

Editor setup (the Lua Language Server, `lua-language-server`, 3.x; `.luarc.json` at the repository root already
holds the settings, so open the repository root as the workspace):

- **VS Code:** install the "Lua" extension (sumneko.lua). Nothing else to configure.
- **Neovim / LazyVim:** install `lua-language-server` (Arch: `pacman -S lua-language-server`) and enable the
  standard `lua_ls` server (LazyVim: add `lua_ls` through the `lang.lua` extra, or `opts.servers.lua_ls = {}`
  in an `nvim-lspconfig` spec). It must start with the repository root as `root_dir`: lspconfig picks the
  directory holding `.luarc.json` on its own. If the server does not pick the file up, set
  `settings = { Lua = { workspace = { library = { "configs" } }, runtime = { version = "Lua 5.4" } } }`.
  No custom plugin is needed.
- **Without an editor:** `lua-language-server --check . --configpath .luarc.json` prints the same diagnostics
  for every file (wrong value types, missing required fields, values outside `"super-small"|"large"` or the
  driver ids). A misspelled key shows up as the missing required field it should have been.

Diagnostics and completion come from the generated `configs/archstaler.lua`; they are hints, and the build is
what accepts or rejects a config. Package names cannot be a closed list, so they are not completed.

The old flat layout (`return { hostname = ..., disk = ... }`) is still read, with a warning, for one migration
window; files written by the GUI always use the `as` layout. A table that mixes `as` with flat keys is refused.

Config values end up in shell-read data files, unit files and boot loader configuration, so `xtask`
validates them strictly at build time and rejects anything with unexpected characters.
