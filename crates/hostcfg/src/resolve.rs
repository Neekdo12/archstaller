//! Package resolution for a config, with the installer's own resolver (`crates/pkg`), so a preview
//! on the host shows what the installer will resolve.
use crate::Result;
use config::Config;
use pkg::db::Db;
use pkg::resolve::Resolver;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub repo: String,
    pub name: String,
    pub version: String,
    pub csize: u64,
    /// Named in `packages` (as opposed to pulled in as a dependency).
    pub explicit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ambiguity {
    pub dep: String,
    pub chosen: String,
    pub candidates: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub packages: Vec<Entry>,
    pub ambiguities: Vec<Ambiguity>,
}

impl Resolution {
    pub fn download_bytes(&self) -> u64 {
        self.packages.iter().map(|p| p.csize).sum()
    }
}

pub fn parse_db(name: &str, data: &[u8]) -> Result<Db> {
    Db::parse(name, data).map_err(|e| format!("{name}.db: {e:?}").into())
}

/// Resolves `cfg.packages` against `dbs` with the config's `providers`.
pub fn resolve(cfg: &Config, dbs: &[Db]) -> Result<Resolution> {
    let providers: BTreeMap<String, String> = cfg.providers.iter().cloned().collect();
    // The official packages the AUR recipes need are installed with the rest (the installer does the same).
    let mut wanted = cfg.packages.clone();
    for d in cfg.aur.iter().flat_map(|a| a.deps.iter()) {
        if !wanted.contains(d) {
            wanted.push(d.clone());
        }
    }
    let r = Resolver::new(dbs, &providers).resolve(&wanted).map_err(|e| format!("{e:?}"))?;
    Ok(Resolution {
        packages: r.packages.iter().map(|s| Entry { repo: s.pkg.repo.clone(), name: s.pkg.name.clone(), version: s.pkg.version.clone(), csize: s.pkg.csize, explicit: s.explicit }).collect(),
        ambiguities: r.ambiguities.iter().map(|a| Ambiguity { dep: a.dep.clone(), chosen: a.chosen.clone(), candidates: a.candidates.clone() }).collect(),
    })
}

/// The sync databases of the machine's own pacman (`core`, `extra`), if it has them.
pub fn local_dbs() -> Result<Vec<Db>> {
    ["core", "extra"]
        .iter()
        .map(|n| parse_db(n, &std::fs::read(format!("/var/lib/pacman/sync/{n}.db"))?))
        .collect()
}

/// What the GUI's package search reads: the parsed databases and, for every package, its one-line
/// description (`descriptions[i][j]` belongs to `dbs[i].packages[j]`; empty when the entry has none).
pub struct Catalog {
    pub dbs: Vec<Db>,
    pub descriptions: Vec<Vec<String>>,
}

/// Parses `(repo, compressed database)` pairs, in priority order, keeping the descriptions.
pub fn parse_catalog(files: &[(&str, Vec<u8>)]) -> Result<Catalog> {
    let mut cat = Catalog { dbs: Vec::new(), descriptions: Vec::new() };
    for (name, data) in files {
        let mut descs = Vec::new();
        let db = Db::parse_with(name, data.as_slice(), |_, text| descs.push(pkg::desc::field(text, "DESC").unwrap_or("").to_string())).map_err(|e| format!("{name}.db: {e:?}"))?;
        cat.dbs.push(db);
        cat.descriptions.push(descs);
    }
    Ok(cat)
}

/// `core.db` and `extra.db` from a mirror (a pacman mirrorlist URL with `$repo`/`$arch`), cached in
/// `cache` for an hour.
#[cfg(feature = "net")]
fn download(mirror: &str, cache: &std::path::Path) -> Result<Vec<(&'static str, Vec<u8>)>> {
    if mirror.trim().is_empty() {
        return Err("no mirror is configured (add one under Mirrors)".into());
    }
    std::fs::create_dir_all(cache)?;
    let mut out = Vec::new();
    for repo in ["core", "extra"] {
        let file = cache.join(format!("{repo}.db"));
        let fresh = std::fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 3600);
        if !fresh {
            let url = format!("{}/{repo}.db", mirror.replace("$repo", repo).replace("$arch", "x86_64").trim_end_matches('/'));
            let mut resp = ureq::get(&url).call().map_err(|e| format!("{url}: {e}"))?;
            let data = resp.body_mut().with_config().limit(128 << 20).read_to_vec().map_err(|e| format!("{url}: {e}"))?;
            std::fs::write(&file, data)?;
        }
        out.push((repo, std::fs::read(&file)?));
    }
    Ok(out)
}

/// Downloads `core.db` and `extra.db` from a mirror and parses them (see [`download`] for the cache).
#[cfg(feature = "net")]
pub fn fetch_dbs(mirror: &str, cache: &std::path::Path) -> Result<Vec<Db>> {
    download(mirror, cache)?.iter().map(|(repo, data)| parse_db(repo, data)).collect()
}

/// [`fetch_dbs`] with the package descriptions, for the GUI's search.
#[cfg(feature = "net")]
pub fn fetch_catalog(mirror: &str, cache: &std::path::Path) -> Result<Catalog> {
    parse_catalog(&download(mirror, cache)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a host with pacman's sync databases: descriptions line up with their packages.
    #[test]
    fn catalog_keeps_descriptions_aligned() {
        let read = |n: &str| std::fs::read(format!("/var/lib/pacman/sync/{n}.db")).ok();
        let (Some(core), Some(extra)) = (read("core"), read("extra")) else { return };
        let cat = parse_catalog(&[("core", core), ("extra", extra)]).unwrap();
        assert_eq!(cat.dbs.len(), 2);
        for (db, d) in cat.dbs.iter().zip(&cat.descriptions) {
            assert_eq!(db.packages.len(), d.len(), "{}", db.name);
        }
        let i = cat.dbs[0].packages.iter().position(|p| p.name == "pacman").unwrap();
        assert!(cat.descriptions[0][i].to_lowercase().contains("package manager"), "{}", cat.descriptions[0][i]);
    }

    #[test]
    fn a_broken_database_names_itself() {
        let e = parse_catalog(&[("core", b"not a database".to_vec())]).err().unwrap().to_string();
        assert!(e.starts_with("core.db"), "{e}");
    }
}
