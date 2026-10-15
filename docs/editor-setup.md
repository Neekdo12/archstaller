# Config files and editor setup

Configs are standard TOML files evaluated on the **build host**; the result is serialized into the ISO.
`configs/config.toml` is a documented example, `configs/e2e.toml` is the config of `cargo xtask e2e`, and
`configs/*.toml` marked `# archstaller: kind=preset` on their first line are the presets.

`docs/toml-config.md` is the full reference for every setting, with checked examples; it is written to be pasted whole into a chatbot. All config files live flat in `configs/`.

## JSON Schema and Editor Support

Archstaller generates a JSON Schema from the Rust types in `crates/hostcfg`: `configs/archstaller.schema.json`
(generated with `cargo xtask gen-schema`; `cargo xtask gen-schema --check` verifies it is up to date).

The repository root includes a `.taplo.toml` file that associates `configs/*.toml` with this schema. This provides:
- Autocompletion for section and key names
- Hover documentation describing each field
- Type and structural validation right in your editor

### Editor Setup

- **VS Code:** Install the **Even Better TOML** extension (`tamasfe.even-better-toml`). It detects `.taplo.toml` and applies schema validation and completion automatically when editing files in `configs/`.
- **Neovim / LazyVim:** Install `taplo` (Arch: `pacman -S taplo-cli` or via Mason: `:MasonInstall taplo`) and configure the `taplo` LSP server (LazyVim: `opts.servers.taplo = {}` in `nvim-lspconfig`). Taplo automatically picks up `.taplo.toml` at the repository root.
- **Helix:** Helix supports `taplo` out of the box if `taplo` is installed on your `PATH`.
- **CLI validation:** Run `taplo check configs/*.toml` or `cargo xtask check-presets` to validate configs from the command line.

The editor provides real-time guidance while typing. The build strictly validates all settings and rejects unknown keys, wrong types, or missing required fields.
