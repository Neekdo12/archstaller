use crate::Result;
use mlua::{Lua, LuaSerdeExt};
use std::path::Path;

/// Evaluates a Lua config and returns it serialized as postcard.
pub fn eval_config(path: &Path) -> Result<Vec<u8>> {
    let src = std::fs::read_to_string(path)?;
    let lua = Lua::new();
    let value: mlua::Value = lua.load(&src).set_name(path.to_string_lossy()).eval()?;
    let cfg: config::Config = lua.from_value(value)?;
    Ok(postcard::to_stdvec(&cfg)?)
}
