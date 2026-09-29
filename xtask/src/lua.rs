use crate::Result;
use mlua::{Lua, LuaSerdeExt};
use std::path::Path;

/// Evaluates a Lua config and returns it serialized as postcard.
pub fn eval_config(path: &Path, extra_kernel_params: &[String]) -> Result<Vec<u8>> {
    let mut cfg = load_config(path)?;
    cfg.kernel_params.extend(extra_kernel_params.iter().cloned());
    cfg.validate()?;
    Ok(postcard::to_stdvec(&cfg)?)
}

/// Evaluates and validates a Lua config.
pub fn load_config(path: &Path) -> Result<config::Config> {
    let path = path.canonicalize()?;
    let path = path.as_path();
    let src = std::fs::read_to_string(path)?;
    let lua = Lua::new();
    // Presets share code through `dofile(CONFIG_DIR .. "/common.lua")`.
    let dir = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into());
    lua.globals().set("CONFIG_DIR", dir)?;
    let value: mlua::Value = lua.load(&src).set_name(path.to_string_lossy()).eval()?;
    let cfg: config::Config = lua.from_value(value)?;
    cfg.validate()?;
    Ok(cfg)
}
