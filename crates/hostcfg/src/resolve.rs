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
    let r = Resolver::new(dbs, &providers).resolve(&cfg.packages).map_err(|e| format!("{e:?}"))?;
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

/// Downloads `core.db` and `extra.db` from a mirror (a pacman mirrorlist URL with `$repo`/`$arch`),
/// caching them in `cache` for an hour.
#[cfg(feature = "net")]
pub fn fetch_dbs(mirror: &str, cache: &std::path::Path) -> Result<Vec<Db>> {
    std::fs::create_dir_all(cache)?;
    let mut dbs = Vec::new();
    for repo in ["core", "extra"] {
        let file = cache.join(format!("{repo}.db"));
        let fresh = std::fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 3600);
        if !fresh {
            let url = format!("{}/{repo}.db", mirror.replace("$repo", repo).replace("$arch", "x86_64").trim_end_matches('/'));
            let mut resp = ureq::get(&url).call().map_err(|e| format!("{url}: {e}"))?;
            let data = resp.body_mut().with_config().limit(128 << 20).read_to_vec().map_err(|e| format!("{url}: {e}"))?;
            std::fs::write(&file, data)?;
        }
        dbs.push(parse_db(repo, &std::fs::read(&file)?)?);
    }
    Ok(dbs)
}
