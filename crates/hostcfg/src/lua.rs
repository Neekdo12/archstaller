//! Evaluating a Lua config.
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
}

/// Evaluates a Lua config and validates both halves.
pub fn load(path: &Path) -> Result<Loaded> {
    let path = path.canonicalize()?;
    let path = path.as_path();
    let src = std::fs::read_to_string(path)?;
    let lua = Lua::new();
    // Presets share code through `dofile(CONFIG_DIR .. "/common.lua")`.
    let dir = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into());
    #[cfg(windows)]
    let dir = if let Some(unc_path) = dir.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc_path}")
    } else {
        dir.strip_prefix(r"\\?\").unwrap_or(&dir).to_owned()
    };
    lua.globals().set("CONFIG_DIR", dir)?;
    let value: mlua::Value = lua.load(&src).set_name(path.to_string_lossy()).eval()?;
    let config: config::Config = lua.from_value(value.clone())?;
    let host: HostConfig = lua.from_value(value)?;
    config.validate()?;
    host.validate()?;
    crate::scripts::validate(&config)?;
    Ok(Loaded { config, host, dir: path.parent().map(Path::to_path_buf).unwrap_or_default() })
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
            scripts: vec![],
        }
    }
}
