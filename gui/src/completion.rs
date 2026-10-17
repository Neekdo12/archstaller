//! What the TOML editor offers at the cursor. Pure functions over the text before the cursor, so they are
//! tested without a display. The lists come from `hostcfg` (driver ids, built-in scripts, the schema
//! of the config) and from what the GUI already loaded; nothing is invented here, and a suggestion
//! is only a hint: the loader and validation decide what a config means.
use hostcfg::host::DRIVERS;
use hostcfg::scripts::BUILTINS;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// What gets inserted in place of the typed prefix.
    pub insert: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Context {
    pub in_string: bool,
    /// The table field the cursor is a value of or inside (`explicit`, `profile`, ...).
    pub key: Option<String>,
    pub prefix: String,
}

/// Lists the editor may draw from.
#[derive(Default, Clone)]
pub struct Sources {
    pub services: Rc<Vec<String>>,
    pub kernel_params: Rc<Vec<String>>,
    pub timezones: Rc<Vec<String>>,
    pub locales: Rc<Vec<String>>,
    pub keymaps: Rc<Vec<String>>,
    pub shells: Rc<Vec<String>>,
    pub groups: Rc<Vec<String>>,
    /// Package names from the last resolution (empty before one ran).
    pub packages: Vec<String>,
}

const KEYWORDS: &[&str] = &["true", "false"];

fn ident_before(s: &str) -> Option<String> {
    let t = s.trim_end();
    let t = t.strip_suffix('=')?.trim_end();
    let name: String = t
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (!name.is_empty()).then_some(name)
}

fn section_before(before: &str) -> Option<String> {
    for line in before.lines().rev() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if t.starts_with("[[") && t.ends_with("]]") {
            let inner = t.get(2..t.len().saturating_sub(2)).unwrap_or("").trim();
            let last_part = inner.rsplit('.').next().unwrap_or(inner);
            return Some(last_part.to_string());
        }
        if t.starts_with('[') && t.ends_with(']') {
            let inner = t.get(1..t.len().saturating_sub(1)).unwrap_or("").trim();
            let last_part = inner.rsplit('.').next().unwrap_or(inner);
            return Some(last_part.to_string());
        }
    }
    None
}

/// The key of the table the cursor is inside: the identifier in front of the nearest unmatched `{` or `[`.
fn enclosing_key(before: &str) -> Option<String> {
    let mut depth = 0i32;
    let mut in_str = false;
    let bytes: Vec<char> = before.chars().collect();
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        let Some(&c) = bytes.get(i) else { continue };
        if c == '"' && (i == 0 || bytes.get(i.saturating_sub(1)) != Some(&'\\')) {
            in_str = !in_str;
        }
        if in_str {
            continue;
        }
        match c {
            '}' | ']' => depth += 1,
            '{' | '[' => {
                if depth == 0 {
                    let head: String = bytes.get(..i).unwrap_or(&[]).iter().collect();
                    return ident_before(&head);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    section_before(before)
}

pub fn context(before: &str) -> Context {
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line = before.get(line_start..).unwrap_or("");
    // Inside a string when an odd number of unescaped quotes precede the cursor on this line.
    let mut quotes = 0;
    let mut last_quote = None;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    for (n, (i, c)) in chars.iter().enumerate() {
        if *c == '"'
            && (n == 0
                || !chars
                    .get(n.saturating_sub(1))
                    .is_some_and(|prev| prev.1 == '\\'))
        {
            quotes += 1;
            last_quote = Some(*i);
        }
    }
    if quotes % 2 == 1 {
        let Some(q) = last_quote else {
            return Context {
                in_string: false,
                key: None,
                prefix: String::new(),
            };
        };
        let prefix = line.get(q + 1..).unwrap_or("").to_string();
        let head_line = line.get(..q).unwrap_or("");
        let before_sub = before.get(..line_start + q).unwrap_or("");
        let key = ident_before(head_line).or_else(|| enclosing_key(before_sub));
        return Context {
            in_string: true,
            key,
            prefix,
        };
    }
    let prefix: String = line
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let head = line.get(..line.len().saturating_sub(prefix.len())).unwrap_or("");
    let before_head = before.get(..before.len().saturating_sub(prefix.len())).unwrap_or("");
    let key = ident_before(head).or_else(|| enclosing_key(before_head));
    Context {
        in_string: false,
        key,
        prefix,
    }
}

/// Every field name of the `as` schema with its description.
pub fn schema_fields() -> Vec<(String, String)> {
    let schema = hostcfg::asconfig::json_schema();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut visit = |node: &serde_json::Value| {
        if let Some(props) = node.get("properties").and_then(|p| p.as_object()) {
            for (k, v) in props {
                let d = v
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string();
                if !out.iter().any(|(n, _)| n == k) {
                    out.push((k.clone(), d));
                }
            }
        }
    };
    visit(&schema);
    if let Some(defs) = schema.get("$defs").and_then(|d| d.as_object()) {
        for v in defs.values() {
            visit(v);
        }
    }
    out
}

fn strings(list: &[String], detail: &str) -> Vec<(String, String)> {
    list.iter()
        .map(|s| (s.clone(), detail.to_string()))
        .collect()
}

/// The values to offer for a key, as (value, detail).
fn values(key: &str, src: &Sources) -> Vec<(String, String)> {
    match key {
        "profile" => vec![
            ("super-small".into(), "smallest installer (default)".into()),
            ("large".into(), "keeps panic messages".into()),
        ],
        "installer_drivers" => DRIVERS
            .iter()
            .map(|d| (d.id.to_string(), d.description.to_string()))
            .collect(),
        "id" => BUILTINS
            .iter()
            .map(|b| (b.id.to_string(), b.description.to_string()))
            .collect(),
        "explicit" => src
            .packages
            .iter()
            .map(|p| (p.clone(), "from the last resolution".to_string()))
            .collect(),
        "services" => strings(&src.services, "unit"),
        "kernel_params" => strings(&src.kernel_params, "kernel parameter"),
        "timezone" => strings(&src.timezones, "time zone"),
        "locale" => strings(&src.locales, "locale"),
        "keymap" => strings(&src.keymaps, "keymap"),
        "shell" => strings(&src.shells, "login shell"),
        "groups" => strings(&src.groups, "group"),
        _ => vec![],
    }
}

/// Up to 40 suggestions for the cursor position, best matches (prefix) first.
pub fn candidates(ctx: &Context, src: &Sources, fields: &[(String, String)]) -> Vec<Item> {
    let p = ctx.prefix.to_lowercase();
    let matches = |s: &str| s.to_lowercase().contains(&p) && s.to_lowercase() != p;
    let rank = |s: &str| !s.to_lowercase().starts_with(&p);
    let mut out: Vec<Item> = Vec::new();
    if let Some(key) = &ctx.key {
        let vals = values(key, src);
        if !vals.is_empty() {
            for (v, d) in vals.into_iter().filter(|(v, _)| matches(v)) {
                let insert = if ctx.in_string {
                    v.clone()
                } else {
                    format!("\"{v}\"")
                };
                out.push(Item {
                    insert,
                    label: v,
                    detail: d,
                });
            }
        }
    }
    if out.is_empty() && !ctx.in_string && !ctx.prefix.is_empty() {
        for (name, d) in fields.iter().filter(|(n, _)| matches(n)) {
            out.push(Item {
                insert: name.clone(),
                label: name.clone(),
                detail: d.clone(),
            });
        }
        for k in KEYWORDS.iter().filter(|k| matches(k)) {
            out.push(Item {
                insert: k.to_string(),
                label: k.to_string(),
                detail: "TOML keyword".into(),
            });
        }
    }
    out.sort_by_key(|i| rank(&i.label));
    out.truncate(40);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_inside_strings_and_tables() {
        let c = context("  build = { profile = \"la");
        assert_eq!(
            c,
            Context {
                in_string: true,
                key: Some("profile".into()),
                prefix: "la".into()
            }
        );
        let c = context("explicit = [ \"base\", \"lin");
        assert!(
            c.in_string && c.key.as_deref() == Some("explicit") && c.prefix == "lin",
            "{c:?}"
        );
        // The key is the enclosing table for a bare string in a list on its own line.
        let c = context("services = [\n  \"sys");
        assert!(
            c.in_string && c.key.as_deref() == Some("services") && c.prefix == "sys",
            "{c:?}"
        );
        // Not in a string: a field name being typed inside a section.
        let c = context("[system]\nhost");
        assert!(
            !c.in_string && c.prefix == "host" && c.key.as_deref() == Some("system"),
            "{c:?}"
        );
        // A closed array before the cursor does not count.
        let c = context("a = [1]\nz");
        assert!(c.key.is_none() && c.prefix == "z", "{c:?}");
        // Escaped quotes do not toggle the state.
        let c = context("x = \"a\\\"b");
        assert!(c.in_string, "{c:?}");
    }

    #[test]
    fn candidates_come_from_the_catalogues() {
        let src = Sources {
            services: Rc::new(vec!["sshd.service".into(), "NetworkManager.service".into()]),
            packages: vec!["base".into(), "linux".into()],
            ..Default::default()
        };
        let fields = schema_fields();
        assert!(
            fields.iter().any(|(n, d)| n == "hostname" && !d.is_empty()),
            "descriptions are carried over"
        );
        let drivers = candidates(&context("installer_drivers = [ \"virt"), &src, &fields);
        assert!(
            drivers.iter().any(|i| i.label == "virtio-blk")
                && drivers.iter().any(|i| i.label == "virtio-net")
        );
        assert!(
            drivers.iter().all(|i| !i.insert.contains('"')),
            "inside a string the quotes are already there"
        );
        let profile = candidates(&context("profile = "), &src, &fields);
        // No prefix typed: nothing is pushed on the user for a value position outside a string... except the catalogue.
        assert!(
            profile.iter().any(|i| i.insert == "\"large\""),
            "{profile:?}"
        );
        let scripts = candidates(&context("scripts = [ { id = \"enable-s"), &src, &fields);
        assert_eq!(
            scripts.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            ["enable-sshd"]
        );
        let pk = candidates(&context("explicit = [ \"li"), &src, &fields);
        assert_eq!(
            pk.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            ["linux"]
        );
        let svc = candidates(&context("services = [ \"ssh"), &src, &fields);
        assert_eq!(svc.len(), 1);
        let f = candidates(&context("[install]\nmirr"), &src, &fields);
        assert!(f.iter().any(|i| i.label == "mirrors"));
        let kw = candidates(&context("loc"), &src, &fields);
        assert!(kw.iter().any(|i| i.label == "locale"));
        let bool_kw = candidates(&context("tr"), &src, &fields);
        assert!(bool_kw.iter().any(|i| i.label == "true"));
        // An unknown key inside a string offers nothing.
        assert!(candidates(&context("mystery = \"ab"), &src, &fields).is_empty());
    }
}
