//! Lua Language Server annotations for the `as` config, generated from the JSON Schema of
//! [`crate::asconfig`] so that the editor types and the Rust loader cannot drift apart.
//! `cargo xtask gen-luals` writes `configs/archstaller.lua`; `--check` fails when it is stale.
use serde_json::Value;
use std::fmt::Write;

/// Class name for a schema definition: everything lives under the `As` prefix.
fn class_name(def: &str) -> String {
    if def.starts_with("As") {
        def.to_string()
    } else {
        format!("As{def}")
    }
}

fn lua_literal(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(format!("{s:?}")),
        Value::Null => Some("nil".into()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The LuaLS type of a schema node, and whether it admits `nil`.
fn ty(node: &Value) -> (String, bool) {
    if let Some(r) = node.get("$ref").and_then(Value::as_str) {
        return (class_name(r.rsplit('/').next().unwrap_or(r)), false);
    }
    if let Some(any) = node.get("anyOf").and_then(Value::as_array) {
        let mut nullable = false;
        let mut parts = Vec::new();
        for a in any {
            if a.get("type").and_then(Value::as_str) == Some("null") {
                nullable = true;
            } else {
                let (t, n) = ty(a);
                nullable |= n;
                parts.push(t);
            }
        }
        return (parts.join("|"), nullable);
    }
    if let Some(vals) = node.get("enum").and_then(Value::as_array) {
        let nullable = vals.iter().any(Value::is_null);
        let lits: Vec<String> = vals.iter().filter(|v| !v.is_null()).filter_map(lua_literal).collect();
        return (lits.join("|"), nullable);
    }
    let types: Vec<&str> = match node.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    let nullable = types.contains(&"null");
    let main = types.iter().copied().find(|t| *t != "null").unwrap_or("any");
    let t = match main {
        "string" => "string".to_string(),
        "integer" => "integer".to_string(),
        "number" => "number".to_string(),
        "boolean" => "boolean".to_string(),
        "array" => {
            let (inner, _) = node.get("items").map(ty).unwrap_or(("any".into(), false));
            if inner.contains('|') {
                format!("({inner})[]")
            } else {
                format!("{inner}[]")
            }
        }
        "object" => match node.get("additionalProperties") {
            Some(v) if v.is_object() => format!("table<string, {}>", ty(v).0),
            _ => "table".to_string(),
        },
        _ => "any".to_string(),
    };
    (t, nullable)
}

fn comment(desc: Option<&str>, prefix: &str, out: &mut String) {
    if let Some(d) = desc {
        for line in d.lines() {
            writeln!(out, "{prefix}{line}").unwrap();
        }
    }
}

/// The text of `configs/archstaller.lua`.
pub fn annotations(schema: &Value) -> String {
    let mut o = String::new();
    writeln!(o, "-- archstaller: kind=module").unwrap();
    writeln!(o, "-- LuaLS type definitions for the Archstaller `as` config.").unwrap();
    writeln!(o, "-- GENERATED from the Rust schema (crates/hostcfg/src/asconfig.rs) by `cargo xtask gen-luals`; do not edit.").unwrap();
    writeln!(o, "---@meta").unwrap();
    let defs = schema.get("$defs").and_then(Value::as_object);
    // The root is the envelope; the definitions are everything it refers to.
    let mut classes: Vec<(String, &Value)> = vec![("Envelope".into(), schema)];
    if let Some(d) = defs {
        classes.extend(d.iter().map(|(k, v)| (k.clone(), v)));
    }
    for (name, node) in classes {
        writeln!(o).unwrap();
        comment(node.get("description").and_then(Value::as_str), "---", &mut o);
        writeln!(o, "---@class (exact) {}", class_name(&name)).unwrap();
        let required: Vec<&str> = node.get("required").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if let Some(props) = node.get("properties").and_then(Value::as_object) {
            for (field, p) in props {
                let (t, nullable) = ty(p);
                let optional = nullable || !required.contains(&field.as_str());
                let desc = p.get("description").and_then(Value::as_str).map(|d| d.replace('\n', " "));
                let q = if optional { "?" } else { "" };
                match desc {
                    Some(d) => writeln!(o, "---@field {field}{q} {t} {d}").unwrap(),
                    None => writeln!(o, "---@field {field}{q} {t}").unwrap(),
                }
            }
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_annotations_cover_the_schema() {
        let text = annotations(&crate::asconfig::json_schema());
        assert!(text.contains("---@class (exact) AsEnvelope"));
        assert!(text.contains("---@field as AsConfig"));
        assert!(text.contains("---@class (exact) AsConfig"));
        assert!(text.contains("---@field hostname string "));
        assert!(text.contains("---@field esp_mib integer "));
        assert!(text.contains("---@field providers? table<string, string> "));
        assert!(text.contains("---@field profile? \"super-small\"|\"large\" "));
        assert!(text.contains("---@field installer_drivers? (\"virtio-blk\"|"));
        assert!(text.contains("---@field users? AsUser[] "));
        assert!(text.contains("---@field root_password_hash? string "));
        assert!(!text.contains(": any"));
    }
}
