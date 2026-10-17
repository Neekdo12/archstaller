//! Generates the JSON Schema for Archstaller TOML configurations.
//! Editors (VS Code Even Better TOML, Neovim/Helix Taplo) use this for autocompletion,
//! hover documentation, and structural validation.

use crate::asconfig;

/// Returns the formatted JSON schema string for `AsConfig`.
pub fn generate() -> String {
    let schema = asconfig::json_schema();
    serde_json::to_string_pretty(&schema).expect("schema serializes to pretty JSON") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid_json() {
        let text = generate();
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert!(parsed.is_object());
        assert!(parsed.get("properties").is_some());
    }
}
