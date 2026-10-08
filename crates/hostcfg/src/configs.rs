//! The repository's `configs/` directory: example, e2e, shared modules, the LuaLS definitions and
//! the selectable presets, all flat in one folder. A file says what it is on its first line:
//! `-- archstaller: kind=preset`. Only `kind=preset` files are presets; everything else (including
//! files with no marker) is left alone. `xtask` and the GUI both list presets through [`presets`].
use std::path::{Path, PathBuf};

/// The directory name under the repository root.
pub const DIR: &str = "configs";

/// What a file in `configs/` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A selectable starting point (`xtask presets`, `check-presets`, the GUI's "From preset" menu).
    Preset,
    /// The documented default config.
    Example,
    /// A config for automated install tests.
    E2e,
    /// Shared Lua code or type definitions that a config loads.
    Module,
}

/// The kind named by a file's first line, if it has a marker.
pub fn kind_of(text: &str) -> Option<Kind> {
    let rest = text.lines().next()?.trim().strip_prefix("--")?.trim().strip_prefix("archstaller:")?;
    let value = rest.split_whitespace().find_map(|t| t.strip_prefix("kind="))?;
    match value {
        "preset" => Some(Kind::Preset),
        "example" => Some(Kind::Example),
        "e2e" => Some(Kind::E2e),
        "module" => Some(Kind::Module),
        _ => None,
    }
}

/// A selectable preset.
#[derive(Debug, Clone)]
pub struct Preset {
    /// The file name without `.lua`.
    pub name: String,
    /// The first comment line after the marker.
    pub description: String,
    pub path: PathBuf,
}

/// The presets in a `configs/` directory, sorted by name.
pub fn presets(dir: &Path) -> Vec<Preset> {
    let Ok(read) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<Preset> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lua"))
        .filter_map(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            if kind_of(&text) != Some(Kind::Preset) {
                return None;
            }
            let description = text.lines().skip(1).find_map(|l| l.trim().strip_prefix("--")).map(|l| l.trim().to_string()).unwrap_or_default();
            Some(Preset { name: p.file_stem()?.to_string_lossy().into_owned(), description, path: p })
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers() {
        assert_eq!(kind_of("-- archstaller: kind=preset\n-- x"), Some(Kind::Preset));
        assert_eq!(kind_of("--archstaller:kind=module"), Some(Kind::Module));
        assert_eq!(kind_of("-- archstaller: kind=e2e extra"), Some(Kind::E2e));
        assert_eq!(kind_of("-- archstaller: kind=nope"), None);
        assert_eq!(kind_of("-- just a comment"), None);
        assert_eq!(kind_of("return {}"), None);
        assert_eq!(kind_of(""), None);
    }

    #[test]
    fn only_marked_files_are_presets() {
        let dir = std::env::temp_dir().join(format!("hostcfg-configs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.lua"), "-- archstaller: kind=preset\n-- First.\nreturn {}").unwrap();
        std::fs::write(dir.join("common.lua"), "-- archstaller: kind=module\nreturn {}").unwrap();
        std::fs::write(dir.join("config.lua"), "-- archstaller: kind=example\nreturn {}").unwrap();
        std::fs::write(dir.join("unmarked.lua"), "-- not marked\nreturn {}").unwrap();
        std::fs::write(dir.join("notes.txt"), "-- archstaller: kind=preset").unwrap();
        let list = presets(&dir);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].description.as_str()), ("a", "First."));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
