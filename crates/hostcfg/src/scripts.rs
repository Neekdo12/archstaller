//! First-boot scripts: the built-in catalogue and turning a config's script list into what the installer gets.
use crate::Result;
use config::{Config, Script, SCRIPT_MAX};
use std::path::Path;

/// A script that ships with archstaller.
pub struct Builtin {
    pub id: &'static str,
    pub description: &'static str,
    /// When it runs.
    pub phase: &'static str,
    pub runs_as: &'static str,
    /// What `args` it understands.
    pub params: &'static str,
    /// Packages it needs in the config's `packages`.
    pub requires: &'static str,
    pub source: &'static str,
}

pub const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "enable-sshd",
        description: "Enable the OpenSSH server at boot",
        phase: "end of first boot",
        runs_as: "root",
        params: "none",
        requires: "openssh",
        source: include_str!("../../../firstboot/scripts/enable-sshd.sh"),
    },
    Builtin {
        id: "enable-fstrim",
        description: "Enable the weekly TRIM timer for SSDs",
        phase: "end of first boot",
        runs_as: "root",
        params: "none",
        requires: "",
        source: include_str!("../../../firstboot/scripts/enable-fstrim.sh"),
    },
];

pub fn builtin(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.id == id)
}

/// Where a script comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Builtin,
    File(String),
    Remote { url: String, sha256: String },
}

pub fn source(s: &Script) -> Source {
    match (&s.file, &s.url) {
        (Some(f), _) => Source::File(f.clone()),
        (None, Some(u)) => Source::Remote { url: u.clone(), sha256: s.sha256.clone().unwrap_or_default() },
        (None, None) => Source::Builtin,
    }
}

/// Checks what `Config::validate` cannot know: that a script without a source names a built-in one.
pub fn validate(cfg: &Config) -> std::result::Result<(), String> {
    for (i, s) in cfg.scripts.iter().enumerate() {
        if source(s) == Source::Builtin && s.content.is_none() && builtin(&s.id).is_none() {
            return Err(format!("scripts[{}]: {:?} is not a built-in script (give it a file or a url + sha256)", i + 1, s.id));
        }
    }
    Ok(())
}

/// The config as the installer receives it: built-in and local scripts carry their text in `content`.
/// `base` is the directory local script paths are relative to.
pub fn resolve(cfg: &Config, base: &Path) -> Result<Config> {
    let mut out = cfg.clone();
    for (i, s) in out.scripts.iter_mut().enumerate() {
        match source(s) {
            Source::Builtin => {
                if s.content.is_none() {
                    let b = builtin(&s.id).ok_or_else(|| format!("scripts[{}]: {:?} is not a built-in script", i + 1, s.id))?;
                    s.content = Some(b.source.as_bytes().to_vec());
                }
            }
            Source::File(f) => {
                let path = base.join(&f);
                let data = std::fs::read(&path).map_err(|e| format!("scripts[{}]: cannot read {}: {e}", i + 1, path.display()))?;
                if data.len() > SCRIPT_MAX {
                    return Err(format!("scripts[{}]: {} is larger than 64 KiB", i + 1, path.display()).into());
                }
                s.content = Some(data);
                s.file = None;
            }
            Source::Remote { .. } => {}
        }
    }
    Ok(out)
}

/// One line per script for the build summary: source, phase, privileges and (where known) the digest.
pub fn summary(cfg: &Config) -> Vec<String> {
    use std::fmt::Write;
    cfg.scripts
        .iter()
        .map(|s| {
            let mut l = format!("script {}: ", s.id);
            match source(s) {
                Source::Builtin => write!(l, "built-in, ").unwrap(),
                Source::File(f) => write!(l, "local file {f}, ").unwrap(),
                Source::Remote { url, sha256 } => write!(l, "{url} (sha256 {sha256}), ").unwrap(),
            }
            l.push_str("runs as root at the end of the first boot");
            l
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(id: &str) -> Script {
        Script { id: id.into(), ..Default::default() }
    }

    #[test]
    fn builtins_exist_and_start_with_a_shebang() {
        for b in BUILTINS {
            assert!(b.source.starts_with("#!/bin/bash"), "{}", b.id);
        }
    }

    #[test]
    fn unknown_builtin_is_rejected() {
        let mut cfg = crate::lua::tests_support::minimal_config();
        cfg.scripts = vec![script("enable-sshd")];
        assert!(validate(&cfg).is_ok());
        cfg.scripts = vec![script("no-such-thing")];
        assert!(validate(&cfg).unwrap_err().contains("not a built-in"));
    }

    #[test]
    fn config_validation_of_script_sources() {
        let mut cfg = crate::lua::tests_support::minimal_config();
        let check = |cfg: &Config| cfg.validate().unwrap_err();
        let sha = "a".repeat(64);
        cfg.scripts = vec![Script { id: "x".into(), url: Some("https://e.org/x.sh".into()), ..Default::default() }];
        assert!(check(&cfg).contains("needs its sha256"));
        cfg.scripts = vec![Script { id: "x".into(), url: Some("http://e.org/x.sh".into()), sha256: Some(sha.clone()), ..Default::default() }];
        assert!(check(&cfg).contains("https://"));
        cfg.scripts = vec![Script { id: "x".into(), url: Some("https://e.org/x.sh".into()), sha256: Some("ABC".into()), ..Default::default() }];
        assert!(check(&cfg).contains("64 lower-case hex"));
        cfg.scripts = vec![Script { id: "x".into(), file: Some("a.sh".into()), url: Some("https://e.org/x.sh".into()), sha256: Some(sha.clone()), ..Default::default() }];
        assert!(check(&cfg).contains("not both"));
        cfg.scripts = vec![Script { id: "x y".into(), ..Default::default() }];
        assert!(check(&cfg).contains("invalid id"));
        cfg.scripts = vec![Script { id: "x".into(), args: vec!["a b".into()], ..Default::default() }];
        assert!(check(&cfg).contains("args"));
        cfg.scripts = vec![script("x"), script("x")];
        assert!(check(&cfg).contains("twice"));
        cfg.scripts = vec![Script { id: "x".into(), url: Some("https://e.org/x.sh".into()), sha256: Some(sha), args: vec!["--now".into()], ..Default::default() }];
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn resolve_fills_in_content() {
        let mut cfg = crate::lua::tests_support::minimal_config();
        cfg.scripts = vec![script("enable-fstrim")];
        let r = resolve(&cfg, Path::new(".")).unwrap();
        assert!(r.scripts[0].content.as_ref().unwrap().starts_with(b"#!/bin/bash"));
    }
}
