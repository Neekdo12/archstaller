//! Evaluating a Lua config.
use crate::asconfig::Envelope;
use crate::host::HostConfig;
use crate::Result;
use mlua::{Lua, LuaSerdeExt};
use std::path::Path;

/// Everything a Lua config says: what the installer gets and how the ISO is built.
pub struct Loaded {
    pub config: config::Config,
    pub host: HostConfig,
    /// The directory of the config file (local script paths are relative to it).
    pub dir: std::path::PathBuf,
    /// Things worth telling the user that do not stop the build (for example a legacy flat config).
    pub warnings: Vec<String>,
}

/// The note printed for a config in the old flat shape.
pub const LEGACY_WARNING: &str = "this config uses the legacy flat layout; it is still read, but new files are written as `return { as = { ... } }` (see docs/lua-config-api.md)";

/// Evaluates a Lua config and validates both halves.
pub fn load(path: &Path) -> Result<Loaded> {
    let loaded = load_unvalidated(path)?;
    loaded.config.validate()?;
    loaded.host.validate()?;
    crate::scripts::validate(&loaded.config)?;
    Ok(loaded)
}

/// Evaluates a Lua config with the strict shape and type checks but without the domain validation, for an
/// editor that wants to show an incomplete config and let its forms point at what is missing.
pub fn load_unvalidated(path: &Path) -> Result<Loaded> {
    let path = path.canonicalize()?;
    let path = path.as_path();
    let src = std::fs::read_to_string(path)?;
    let lua = Lua::new();
    // Modules beside the config load with `require("common")`; `dofile(CONFIG_DIR .. "/common.lua")`
    // still works for older configs.
    let dir = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into());
    #[cfg(windows)]
    let dir = if let Some(unc_path) = dir.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc_path}")
    } else {
        dir.strip_prefix(r"\\?\").unwrap_or(&dir).to_owned()
    };
    let package: mlua::Table = lua.globals().get("package")?;
    let old: String = package.get("path")?;
    package.set("path", format!("{dir}/?.lua;{old}"))?;
    lua.globals().set("CONFIG_DIR", dir)?;
    let value: mlua::Value = lua.load(&src).set_name(format!("@{}", path.to_string_lossy())).eval()?;
    let mut warnings = Vec::new();
    let (config, host) = match namespaced(&value)? {
        true => Envelope::from_lua(value)?.as_.into_parts()?,
        false => {
            warnings.push(LEGACY_WARNING.to_string());
            let config: config::Config = lua.from_value(value.clone())?;
            let host: HostConfig = lua.from_value(value)?;
            (config, host)
        }
    };
    Ok(Loaded { config, host, dir: path.parent().map(Path::to_path_buf).unwrap_or_default(), warnings })
}

/// Whether the config uses the `as` namespace. A table that mixes it with other keys is refused:
/// a partially migrated file has no single meaning.
fn namespaced(value: &mlua::Value) -> Result<bool> {
    let mlua::Value::Table(t) = value else { return Err(format!("the config must return a table, got {}", value.type_name()).into()) };
    if !t.contains_key("as")? {
        return Ok(false);
    }
    let mut others = Vec::new();
    for pair in t.pairs::<mlua::Value, mlua::Value>() {
        if let (mlua::Value::String(k), _) = pair? {
            let k = k.to_string_lossy();
            if k != "as" {
                others.push(k);
            }
        }
    }
    if others.is_empty() {
        Ok(true)
    } else {
        others.sort();
        Err(format!("the config mixes the `as` namespace with legacy top-level keys ({}); move them under `as` or remove `as`", others.join(", ")).into())
    }
}

/// Evaluates and validates a Lua config, returning only the installer's part.
pub fn load_config(path: &Path) -> Result<config::Config> {
    Ok(load(path)?.config)
}

/// The installer's config, with `extra_kernel_params` appended, serialized for `config.bin`.
pub fn config_bin(loaded: &Loaded, extra_kernel_params: &[String]) -> Result<Vec<u8>> {
    let mut cfg = crate::scripts::resolve(&loaded.config, &loaded.dir)?;
    cfg.kernel_params.extend(extra_kernel_params.iter().cloned());
    cfg.validate()?;
    Ok(postcard::to_stdvec(&cfg)?)
}

#[cfg(test)]
pub(crate) mod tests_support {
    /// A valid config for unit tests.
    pub fn minimal_config() -> config::Config {
        config::Config {
            hostname: "box".into(),
            timezone: "Europe/Prague".into(),
            locale: "en_US.UTF-8".into(),
            keymap: "us".into(),
            disk: config::Disk { model: None, confirm_serial: "SER".into(), auto_largest: false, esp_mib: 512 },
            mirrors: vec!["https://example.org/$repo/os/$arch".into()],
            packages: vec!["base".into()],
            providers: vec![],
            root_password_hash: None,
            users: vec![],
            services: vec![],
            kernel_params: vec![],
            user_files: vec![],
            user_archives: vec![],
            dry_run: false,
            aur: vec![],
            scripts: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("hostcfg-lua-{}-{name}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn load_text(name: &str, text: &str) -> Result<Loaded> {
        let d = tmp(name);
        let p = d.join("c.lua");
        std::fs::write(&p, text).unwrap();
        let r = load(&p);
        let _ = std::fs::remove_dir_all(&d);
        r
    }

    /// A valid namespaced config with one line replaced (`from` -> `to`).
    fn namespaced(from: &str, to: &str) -> String {
        let base = r#"return { as = {
  schema = 1,
  system = { hostname = "box", timezone = "Europe/Prague", locale = "en_US.UTF-8", keymap = "us" },
  install = { disk = { confirm_serial = "SER", esp_mib = 512 }, mirrors = { "https://example.org/$repo/os/$arch" } },
  packages = { explicit = { "base" }, providers = { initramfs = "mkinitcpio" } },
} }"#;
        assert!(base.contains(from), "{from}");
        base.replace(from, to)
    }

    fn err(text: &str) -> String {
        load_text("err", text).err().expect("must fail").to_string()
    }

    #[test]
    fn namespaced_config_loads_without_warnings() {
        let l = load_text("ok", &namespaced("keymap = \"us\"", "keymap = \"us\"")).unwrap();
        assert!(l.warnings.is_empty());
        assert_eq!(l.config.hostname, "box");
        assert_eq!(l.config.disk.confirm_serial, "SER");
        assert_eq!(l.config.providers, vec![("initramfs".to_string(), "mkinitcpio".to_string())]);
        assert_eq!(l.host, HostConfig::default());
    }

    #[test]
    fn errors_carry_the_full_path() {
        assert!(err(&namespaced("hostname = \"box\"", "hostname = 5")).starts_with("as.system.hostname:"), "wrong type");
        assert!(err(&namespaced("esp_mib = 512", "esp_mib = \"512\"")).starts_with("as.install.disk.esp_mib:"), "string for integer");
        assert!(err(&namespaced("esp_mib = 512", "esp_mib = 512.5")).starts_with("as.install.disk.esp_mib:"), "float for integer");
        assert!(err(&namespaced("esp_mib = 512", "esp_mib = 512.0")).starts_with("as.install.disk.esp_mib:"), "no float to integer coercion");
        assert!(err(&namespaced("keymap = \"us\"", "keymap = \"us\", hostnam = \"x\"")).contains("unknown field `hostnam`"), "misspelled key");
        assert!(err(&namespaced("keymap = \"us\"", "keymap = \"us\", hostnam = \"x\"")).starts_with("as.system"), "path of the misspelled key");
        assert!(err(&namespaced("hostname = \"box\", ", "")).contains("missing field `hostname`"), "missing value");
        assert!(err(&namespaced("explicit = { \"base\" }", "explicit = { \"base\", 3 }")).starts_with("as.packages.explicit[2]:"), "bad list entry");
        assert!(err(&namespaced("schema = 1", "schema = 2")).starts_with("as.schema:"), "schema version");
    }

    #[test]
    fn nested_entries_reject_unknown_and_mistyped_fields() {
        let with_user = |fields: &str| namespaced("packages = {", &format!("users = {{ {{ {fields} }} }},\n  packages = {{"));
        let ok = "name = \"a\", password_hash = \"$6$salt$abcdefghijklmnopqrstuvwxyz./\", groups = {}, shell = \"/bin/sh\"";
        assert!(load_text("u", &with_user(ok)).is_ok());
        let e = err(&with_user(&format!("{ok}, uid = 1000")));
        assert!(e.starts_with("as.users[1].uid:"), "{e}");
        assert!(err(&with_user(&ok.replace("groups = {}", "groups = \"wheel\""))).starts_with("as.users[1].groups:"));
    }

    #[test]
    fn invalid_enum_strings_are_domain_errors() {
        let e = err(&namespaced("packages = {", "build = { profile = \"huge\" },\n  packages = {"));
        assert!(e.contains("unknown build profile"), "{e}");
        let e = err(&namespaced("packages = {", "build = { installer_drivers = { \"virtio-blk\", \"bogus\" } },\n  packages = {"));
        assert!(e.contains("installer_drivers[2]"), "{e}");
    }

    #[test]
    fn the_namespace_cannot_be_mixed_with_legacy_keys() {
        let e = err(&namespaced("schema = 1", "schema = 1").replace("return { as = {", "return { hostname = \"x\", as = {"));
        assert!(e.contains("mixes the `as` namespace") && e.contains("hostname"), "{e}");
    }

    #[test]
    fn modules_beside_the_config_load_with_require() {
        let d = tmp("require");
        std::fs::write(d.join("sys.lua"), "return { hostname = \"fromrequire\", timezone = \"Europe/Prague\", locale = \"en_US.UTF-8\", keymap = \"us\" }").unwrap();
        let text = namespaced("system = { hostname = \"box\", timezone = \"Europe/Prague\", locale = \"en_US.UTF-8\", keymap = \"us\" }", "system = require(\"sys\")");
        std::fs::write(d.join("c.lua"), text).unwrap();
        let l = load(&d.join("c.lua")).unwrap();
        assert_eq!(l.config.hostname, "fromrequire");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn legacy_flat_configs_still_load_with_a_warning_and_give_the_same_installer_config() {
        let legacy = r#"return {
  hostname = "box", timezone = "Europe/Prague", locale = "en_US.UTF-8", keymap = "us",
  disk = { confirm_serial = "SER", esp_mib = 512 },
  mirrors = { "https://example.org/$repo/os/$arch" },
  packages = { "base" },
  providers = { { "initramfs", "mkinitcpio" } },
  users = {}, services = {}, kernel_params = {},
}"#;
        let old = load_text("legacy", legacy).unwrap();
        assert_eq!(old.warnings, vec![LEGACY_WARNING.to_string()]);
        let new = load_text("new", &namespaced("keymap = \"us\"", "keymap = \"us\"")).unwrap();
        assert_eq!(config_bin(&old, &[]).unwrap(), config_bin(&new, &[]).unwrap());
    }

    #[test]
    fn a_config_must_return_a_table() {
        assert!(err("return 5").contains("must return a table"));
    }

    #[test]
    fn repository_configs_all_load_and_stay_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = root.join(crate::configs::DIR);
        let mut seen = 0;
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            let text = std::fs::read_to_string(&p).unwrap();
            // The LuaLS definitions are not configs; the others (except the AUR e2e one, which needs nothing special) must load.
            if p.file_name().is_some_and(|n| n == "archstaler.lua" || n == "common.lua") || p.extension().is_none_or(|x| x != "lua") {
                continue;
            }
            assert!(crate::configs::kind_of(&text).is_some(), "{} has no `-- archstaler: kind=` marker", p.display());
            let l = load(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert!(l.warnings.is_empty(), "{} still uses the legacy layout", p.display());
            seen += 1;
        }
        assert!(seen >= 8, "found only {seen} configs");
        // Generated types match the schema.
        let generated = std::fs::read_to_string(dir.join("archstaler.lua")).unwrap();
        assert_eq!(generated, crate::luals::annotations(&crate::asconfig::json_schema()), "run `cargo xtask gen-luals`");
    }

    /// `docs/lua-config.md` is meant to be pasted into a chatbot, so its examples must be right: every
    /// example marked `<!-- check: valid -->` has to load and validate, and the tables must name every
    /// installer driver, built-in script and preset that exists.
    #[test]
    fn the_lua_config_documentation_is_checked() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let doc = std::fs::read_to_string(root.join("docs/lua-config.md")).expect("docs/lua-config.md");
        let mut checked = 0;
        let mut rest = doc.as_str();
        while let Some(i) = rest.find("<!-- check: valid -->") {
            rest = &rest[i + "<!-- check: valid -->".len()..];
            let start = rest.find("```lua\n").expect("a lua block after the marker") + "```lua\n".len();
            let end = rest[start..].find("```").expect("the block ends") + start;
            let code = &rest[start..end];
            let l = load_text("doc", code).unwrap_or_else(|e| panic!("a documented example does not load: {e}\n{code}"));
            assert!(l.warnings.is_empty(), "documented examples use the `as` layout");
            checked += 1;
            rest = &rest[end..];
        }
        assert!(checked >= 7, "only {checked} checked examples");
        for d in crate::host::DRIVERS {
            assert!(doc.contains(&format!("| `{}` |", d.id)), "driver {} is not in the documentation", d.id);
        }
        for b in crate::scripts::BUILTINS {
            assert!(doc.contains(&format!("| `{}` |", b.id)), "built-in script {} is not in the documentation", b.id);
        }
        for p in crate::configs::presets(&root.join(crate::configs::DIR)) {
            assert!(doc.contains(&format!("| `{}` |", p.name)), "preset {} is not in the documentation", p.name);
        }
        // Numbers the text states.
        assert!(doc.contains("64–8192"), "ESP size range");
        assert!(doc.contains(&format!("Up to {}", config::AUR_MAX)), "AUR limit");
    }
}
