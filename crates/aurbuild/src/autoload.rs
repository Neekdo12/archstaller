//! Detecting and autoloading AUR packages from the host system.
use crate::Result;
#[cfg(feature = "net")]
use std::collections::BTreeMap;
#[cfg(feature = "net")]
use crate::plan::{self, Official, Review};
#[cfg(feature = "net")]
use crate::rpc;

/// Summary of an autoload operation.
#[derive(Debug, Clone, Default)]
pub struct AutoloadReport {
    /// The complete, planned list of AUR packages (existing plus newly added).
    pub entries: Vec<config::AurPackage>,
    /// Package names that were newly added.
    pub added: Vec<String>,
    /// Package names that were already pinned.
    pub already: Vec<String>,
    /// Package names that were skipped, with the reason.
    pub skipped: Vec<(String, String)>,
    /// A human-readable summary message.
    pub summary: String,
}

/// Parses the output of `pacman -Qqem` into clean, valid package names.
pub fn parse_foreign_list(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for l in text.lines().map(str::trim) {
        let ok = !l.is_empty()
            && !l.starts_with(['-', '.'])
            && l.chars()
                .all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c));
        if ok && !names.iter().any(|x| x == l) {
            names.push(l.to_string());
        }
    }
    names
}

/// Returns the package names of foreign packages explicitly installed on this computer (`pacman -Qqem`),
/// dropping any invalid package names or empty lines.
pub fn system_foreign_packages() -> Result<Vec<String>> {
    let out = std::process::Command::new("pacman")
        .arg("-Qqem")
        .output()
        .map_err(|e| {
            format!("pacman could not be run ({e}); reading system AUR packages requires an Arch-based system")
        })?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() && !(out.status.code() == Some(1) && text.trim().is_empty()) {
        return Err(format!(
            "pacman -Qqem failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(parse_foreign_list(&text))
}

/// Takes a list of package names (e.g. from `system_foreign_packages`), inspects the AUR,
/// fetches reviews for them, resolves dependencies, and plans them alongside `existing`.
#[cfg(feature = "net")]
pub fn autoload_packages(
    names: &[String],
    existing: &[config::AurPackage],
    official: &Official,
) -> Result<AutoloadReport> {
    let mut already = Vec::new();
    let mut candidates = Vec::new();
    for n in names {
        if existing.iter().any(|e| &e.name == n) {
            if !already.contains(n) {
                already.push(n.clone());
            }
        } else if !candidates.contains(n) {
            candidates.push(n.clone());
        }
    }

    if candidates.is_empty() {
        let summary = if already.is_empty() {
            "No AUR packages to autoload.".to_string()
        } else {
            format!("All {} package(s) are already pinned.", already.len())
        };
        return Ok(AutoloadReport {
            entries: existing.to_vec(),
            added: Vec::new(),
            already,
            skipped: Vec::new(),
            summary,
        });
    }

    let infos = rpc::info(&candidates)?;
    let mut info_map: BTreeMap<String, rpc::Info> = BTreeMap::new();
    for info in infos {
        info_map.insert(info.name.clone(), info);
    }

    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut to_fetch = Vec::new();
    for c in &candidates {
        if let Some(info) = info_map.get(c) {
            to_fetch.push(info.clone());
        } else {
            skipped.push((c.clone(), "not found on the AUR".to_string()));
        }
    }

    let mut new_reviews: Vec<Review> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    for info in to_fetch {
        match rpc::fetch_review(&info.name, &info.pkgbase, None) {
            Ok(rev) => {
                added.push(info.name.clone());
                new_reviews.push(rev);
            }
            Err(e) => {
                skipped.push((info.name.clone(), format!("could not fetch recipe: {e}")));
            }
        }
    }

    let mut all_reviews = new_reviews;
    for e in existing {
        if !all_reviews.iter().any(|r| r.name == e.name) {
            let snap = rpc::snapshot(&e.commit)?;
            let r = plan::review(&e.name, &e.commit, snap)?;
            if r.tree_sha256 != e.sha256 {
                let commit_short = e.commit.get(..8).unwrap_or(&e.commit);
                return Err(format!(
                    "{}: the recipe at pinned commit {commit_short} no longer matches its pin",
                    e.name
                ));
            }
            all_reviews.push(r);
        }
    }

    // Resolve transitive AUR dependencies (up to 5 passes)
    for _ in 0..5 {
        let missing = plan::unresolved(&all_reviews, official);
        if missing.is_empty() {
            break;
        }
        let mut dep_names = Vec::new();
        for (_owner, dep) in &missing {
            let clean = crate::srcinfo::dep_name(dep);
            if !all_reviews.iter().any(|r| r.name == clean) && !dep_names.iter().any(|d| d == clean) {
                dep_names.push(clean.to_string());
            }
        }
        if dep_names.is_empty() {
            break;
        }
        let dep_infos = rpc::info(&dep_names)?;
        let mut fetched_any = false;
        for info in dep_infos {
            if !all_reviews.iter().any(|r| r.name == info.name) {
                if let Ok(rev) = rpc::fetch_review(&info.name, &info.pkgbase, None) {
                    all_reviews.push(rev);
                    fetched_any = true;
                }
            }
        }
        if !fetched_any {
            break;
        }
    }

    // Prune any newly added candidate packages whose dependencies could not be resolved
    loop {
        let missing = plan::unresolved(&all_reviews, official);
        if missing.is_empty() {
            break;
        }
        let bad_owners: Vec<String> = missing.iter().map(|(owner, _)| owner.clone()).collect();
        let mut removed_any = false;
        all_reviews.retain(|r| {
            if bad_owners.contains(&r.name) && !existing.iter().any(|e| e.name == r.name) {
                let dep = missing
                    .iter()
                    .find(|(o, _)| o == &r.name)
                    .map(|(_, d)| d.as_str())
                    .unwrap_or("unknown");
                skipped.push((r.name.clone(), format!("missing dependency: {dep}")));
                added.retain(|a| a != &r.name);
                removed_any = true;
                false
            } else {
                true
            }
        });
        if !removed_any {
            break;
        }
    }

    let mut explicit: Vec<String> = Vec::new();
    for e in existing {
        if !explicit.contains(&e.name) {
            explicit.push(e.name.clone());
        }
    }
    for a in &added {
        if !explicit.contains(a) {
            explicit.push(a.clone());
        }
    }

    let mut entries = plan::plan(&all_reviews, &explicit, official)?;
    for e in &mut entries {
        if let Some(old) = existing.iter().find(|o| o.name == e.name) {
            e.services = old.services.clone();
        }
    }

    let summary = format!(
        "Autoloaded {} AUR package(s) from system ({} already pinned, {} skipped).",
        added.len(),
        already.len(),
        skipped.len()
    );

    Ok(AutoloadReport {
        entries,
        added,
        already,
        skipped,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_foreign_package_list() {
        let text = "zen-browser-bin\nyay\n\n  custom-pkg  \n-bad-pkg\nweird name\n";
        let parsed = parse_foreign_list(text);
        assert_eq!(parsed, ["zen-browser-bin", "yay", "custom-pkg"]);
    }

    #[test]
    #[cfg(feature = "net")]
    fn empty_candidates_returns_early() {
        let official = plan::Official::from_names(&["base"]);
        let existing = vec![config::AurPackage {
            name: "pkg-a".into(),
            pkgbase: "pkg-a".into(),
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            ..Default::default()
        }];
        let rep = autoload_packages(&["pkg-a".into()], &existing, &official).expect("autoloads ok");
        assert_eq!(rep.already, ["pkg-a"]);
        assert!(rep.added.is_empty());
        assert_eq!(rep.entries.len(), 1);
    }
}
