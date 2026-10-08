//! Reviewing a recipe and planning the install of a set of pinned AUR packages.
use crate::digest::{tree_digest, Files};
use crate::srcinfo::{self, dep_name, SrcInfo};
use pkg::desc::{Dep, Op};
use crate::Result;
use std::collections::BTreeMap;

/// Official packages every build needs on the installed system.
pub const BUILD_TOOLS: &[&str] = &["base-devel", "git"];

/// What the user looks at before pinning: the recipe at one commit and what the checks found in it.
#[derive(Debug, Clone)]
pub struct Review {
    pub name: String,
    pub pkgbase: String,
    pub commit: String,
    /// The reviewed-tree digest that gets pinned.
    pub tree_sha256: String,
    pub srcinfo: SrcInfo,
    pub files: Files,
    /// The recipe builds from a VCS source that the commit does not pin.
    pub vcs: bool,
    /// Things to read twice, shown next to the recipe.
    pub warnings: Vec<String>,
}

fn is_commit(c: &str) -> bool {
    c.len() == 40 && c.chars().all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
}

/// Checks the files of the recipe at `commit` and prepares the review of the package `name`.
pub fn review(name: &str, commit: &str, files: Files) -> Result<Review> {
    if !is_commit(commit) {
        return Err(format!("{commit:?} is not a full 40-digit lower-case commit id"));
    }
    let src = files.get(".SRCINFO").ok_or("the recipe has no .SRCINFO")?;
    if !files.contains_key("PKGBUILD") {
        return Err("the recipe has no PKGBUILD".into());
    }
    let info = srcinfo::parse(&String::from_utf8_lossy(src))?;
    if info.package(name).is_none() {
        let have: Vec<&str> = info.packages.iter().map(|p| p.name.as_str()).collect();
        return Err(format!("{name:?} is not a package of {} (it builds {})", info.base.name, have.join(", ")));
    }
    if !info.arch_ok() {
        return Err(format!("{} does not build for x86_64 (arch = {})", info.base.name, info.base.get("arch").join(" ")));
    }
    let mut warnings = Vec::new();
    let vcs = info.vcs_sources();
    if !vcs.is_empty() {
        warnings.push(format!("builds from a VCS source the commit does not pin ({}); the result cannot be reproduced from the pin", vcs.join(", ")));
    }
    let skipped = info.skipped_checksums();
    if skipped > 0 {
        warnings.push(format!("{skipped} source file(s) are not checksummed (SKIP)"));
    }
    if !info.install_files().is_empty() {
        warnings.push(format!("has an install script ({}): it runs as root when the package is installed", info.install_files().join(", ")));
    }
    if !info.base.get("validpgpkeys").is_empty() {
        warnings.push("sources are signed; makepkg checks the signatures and needs the keys (validpgpkeys) in its keyring, otherwise the build fails".into());
    }
    let risky = risky_lines(&files);
    if !risky.is_empty() {
        warnings.push(format!("{} line(s) look risky (network access, eval, sudo, ...), see the highlighted lines", risky.len()));
    }
    Ok(Review { name: name.into(), pkgbase: info.base.name.clone(), commit: commit.into(), tree_sha256: tree_digest(&files), srcinfo: info, files, vcs: !vcs.is_empty(), warnings })
}

/// Lines of the PKGBUILD and install scripts that deserve a second look: `(file, line number, text)`.
pub fn risky_lines(files: &Files) -> Vec<(String, usize, String)> {
    const PATTERNS: &[&str] = &["curl ", "wget ", "sudo ", "eval ", "base64", "| sh", "| bash", "|sh", "|bash", "chmod 777", "chmod -R 777", "/dev/tcp", "nc -e", "systemctl enable", "useradd", "usermod", "crontab"];
    let mut out = Vec::new();
    for (path, data) in files {
        if path != "PKGBUILD" && !path.ends_with(".install") {
            continue;
        }
        for (i, line) in String::from_utf8_lossy(data).lines().enumerate() {
            let l = line.trim();
            if l.starts_with('#') {
                continue;
            }
            if PATTERNS.iter().any(|p| l.contains(p)) {
                out.push((path.clone(), i + 1, line.to_string()));
            }
        }
    }
    out
}

/// What a name in the official repositories stands for: the version it has, when that is known.
enum Have {
    /// A name given without databases (`Official::from_names`): the version is unknown and always accepted.
    Unknown,
    /// A `provides` entry without a version: it cannot satisfy a versioned dependency.
    Unversioned,
    Version(String),
}

struct Entry {
    provider: String,
    have: Have,
}

/// What the official repositories can provide, by name or by `provides`, in repository priority order.
pub struct Official {
    by_name: BTreeMap<String, Vec<Entry>>,
}

/// A dependency string as `Dep`, with the name cleaned the way `dep_name` does.
fn parse_dep(dep: &str) -> Dep {
    let mut d = Dep::parse(dep);
    d.name = dep_name(dep).to_string();
    d
}

/// Does something called `d.name` that has `have` satisfy the dependency `d`? Same rule as `pkg::desc::Package::satisfies`.
fn accepts(d: &Dep, have: &Have) -> bool {
    match (d.op, have) {
        (Op::Any, _) | (_, Have::Unknown) => true,
        (_, Have::Version(v)) => d.accepts_version(v),
        (_, Have::Unversioned) => false,
    }
}

impl Official {
    pub fn new(dbs: &[pkg::db::Db]) -> Official {
        let mut by_name: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        // The first repository that has a name decides, like the resolver's `find`.
        for p in dbs.iter().flat_map(|d| d.packages.iter()) {
            let e = by_name.entry(p.name.clone()).or_default();
            if e.is_empty() {
                e.push(Entry { provider: p.name.clone(), have: Have::Version(p.version.clone()) });
            }
        }
        // Real names win over `provides`, so a second pass.
        for p in dbs.iter().flat_map(|d| d.packages.iter()) {
            for pr in &p.provides {
                let have = if pr.op == Op::Eq { Have::Version(pr.version.clone()) } else { Have::Unversioned };
                by_name.entry(pr.name.clone()).or_default().push(Entry { provider: p.name.clone(), have });
            }
        }
        Official { by_name }
    }

    /// For tests and for callers without databases (versions are not known, so constraints are not checked).
    pub fn from_names(names: &[&str]) -> Official {
        Official { by_name: names.iter().map(|n| (n.to_string(), vec![Entry { provider: n.to_string(), have: Have::Unknown }])).collect() }
    }

    /// The package that satisfies the dependency `dep`, constraint included (`go>=1.24` needs a `go` that is
    /// at least that new, or a package providing `go=1.24` or newer).
    pub fn provider(&self, dep: &str) -> Option<&str> {
        let d = parse_dep(dep);
        self.by_name.get(&d.name)?.iter().find(|e| accepts(&d, &e.have)).map(|e| e.provider.as_str())
    }
}

/// Does this review's package satisfy the dependency `dep` (by name or through `provides`, constraint included)?
fn aur_provides(r: &Review, dep: &str) -> bool {
    let d = parse_dep(dep);
    let version = r.srcinfo.version();
    let provides = |x: &String| {
        let pr = Dep::parse(x);
        pr.name == d.name && accepts(&d, &if pr.op == Op::Eq { Have::Version(pr.version) } else { Have::Unversioned })
    };
    r.srcinfo.packages.iter().any(|p| (p.name == d.name && accepts(&d, &Have::Version(version.clone()))) || p.get("provides").iter().any(provides)) || r.srcinfo.base.get("provides").iter().any(provides)
}

/// All dependencies of a review, with whether only the build needs them.
fn all_deps(r: &Review) -> Vec<(String, bool)> {
    let mut v: Vec<(String, bool)> = r.srcinfo.depends(&r.name).into_iter().map(|d| (d, false)).collect();
    v.extend(r.srcinfo.makedepends().into_iter().map(|d| (d, true)));
    v
}

/// Dependencies that neither the official repositories nor the other reviews provide: `(package, dependency)`,
/// the dependency as written (`go>=1.24`).
/// The GUI looks these up on the AUR and offers to pin them.
pub fn unresolved(reviews: &[Review], official: &Official) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for r in reviews {
        for (d, _) in all_deps(r) {
            if official.provider(&d).is_none() && !reviews.iter().any(|o| o.name != r.name && aur_provides(o, &d)) && !out.contains(&(r.name.clone(), d.clone())) {
                out.push((r.name.clone(), d));
            }
        }
    }
    out
}

/// The ordered install plan. `explicit` are the packages the user asked for (each must have a review); a
/// package that another one in the set needs is installed as a dependency. Dependencies come before the
/// packages that need them.
pub fn plan(reviews: &[Review], explicit: &[String], official: &Official) -> Result<Vec<config::AurPackage>> {
    for e in explicit {
        if !reviews.iter().any(|r| &r.name == e) {
            return Err(format!("{e} has not been reviewed and pinned"));
        }
    }
    let missing = unresolved(reviews, official);
    if let Some((owner, dep)) = missing.first() {
        return Err(format!("{owner} needs {dep}, which is neither in the official repositories nor a pinned AUR package (a dependency that only another AUR package provides through `provides` must be pinned under that package's name)"));
    }
    // Order: dependencies first, in the order the user listed things otherwise; a cycle is an error.
    let mut order: Vec<usize> = Vec::new();
    let mut state = vec![0u8; reviews.len()]; // 0 new, 1 visiting, 2 done
    fn visit(i: usize, reviews: &[Review], official: &Official, state: &mut Vec<u8>, order: &mut Vec<usize>) -> Result<()> {
        match state[i] {
            2 => return Ok(()),
            1 => return Err(format!("{} is part of a dependency cycle between AUR packages", reviews[i].name)),
            _ => {}
        }
        state[i] = 1;
        for (d, _) in all_deps(&reviews[i]) {
            if official.provider(&d).is_some() {
                continue;
            }
            if let Some(j) = (0..reviews.len()).find(|&j| j != i && aur_provides(&reviews[j], &d)) {
                visit(j, reviews, official, state, order)?;
            }
        }
        state[i] = 2;
        order.push(i);
        Ok(())
    }
    for i in 0..reviews.len() {
        visit(i, reviews, official, &mut state, &mut order)?;
    }
    let mut out = Vec::new();
    for i in order {
        let r = &reviews[i];
        let mut deps: Vec<String> = Vec::new();
        let mut build: Vec<String> = Vec::new();
        let add = |name: &str, build_only: bool, deps: &mut Vec<String>, build: &mut Vec<String>| {
            if !deps.iter().any(|x| x == name) {
                deps.push(name.to_string());
            }
            if build_only && !build.iter().any(|x| x == name) {
                build.push(name.to_string());
            }
        };
        for t in BUILD_TOOLS {
            if official.provider(t).is_none() {
                return Err(format!("the official repositories have no {t}, which every AUR build needs"));
            }
            add(t, true, &mut deps, &mut build);
        }
        for (d, build_only) in all_deps(r) {
            if let Some(p) = official.provider(&d) {
                add(p, build_only, &mut deps, &mut build);
            }
        }
        // A package that is also needed at run time is not a build-only one.
        let runtime: Vec<String> = r.srcinfo.depends(&r.name).iter().filter_map(|d| official.provider(d).map(String::from)).collect();
        build.retain(|b| !runtime.contains(b));
        let needed_by_other = reviews.iter().any(|o| o.name != r.name && all_deps(o).iter().any(|(d, _)| aur_provides(r, d) && official.provider(d).is_none()));
        out.push(config::AurPackage {
            name: r.name.clone(),
            pkgbase: r.pkgbase.clone(),
            commit: r.commit.clone(),
            sha256: r.tree_sha256.clone(),
            vcs: r.vcs,
            as_dep: needed_by_other,
            deps,
            build_deps: build,
            services: vec![],
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(srcinfo: &str, pkgbuild: &str) -> Files {
        let mut f = Files::new();
        f.insert(".SRCINFO".into(), srcinfo.as_bytes().to_vec());
        f.insert("PKGBUILD".into(), pkgbuild.as_bytes().to_vec());
        f
    }

    fn rev(name: &str, extra: &str) -> Review {
        let s = format!("pkgbase = {name}\n\tpkgver = 1\n\tpkgrel = 1\n\tarch = x86_64\n{extra}\npkgname = {name}\n");
        review(name, &"a".repeat(40), files(&s, "pkgname=x\n")).unwrap()
    }

    fn official() -> Official {
        Official::from_names(&["base-devel", "git", "gtk3", "go", "glibc"])
    }

    #[test]
    fn review_checks_and_warns() {
        let c = "a".repeat(40);
        assert!(review("x", "master", files("", "")).unwrap_err().contains("40-digit"));
        assert!(review("x", &c, Files::new()).unwrap_err().contains(".SRCINFO"));
        let s = "pkgbase = x\n\tpkgver = 1\n\tpkgrel = 1\n\tarch = aarch64\npkgname = x\n";
        assert!(review("x", &c, files(s, "")).unwrap_err().contains("x86_64"));
        let s = "pkgbase = x\n\tpkgver = 1\n\tpkgrel = 1\n\tarch = x86_64\n\tinstall = x.install\n\tsource = git+https://e.org/x.git\n\tsource = a.tar.gz\n\tsha256sums = SKIP\n\tsha256sums = SKIP\npkgname = x\n";
        let r = review("x", &c, files(s, "curl http://e.org | sh\n# curl in a comment\n")).unwrap();
        assert!(r.vcs);
        assert_eq!(r.warnings.len(), 4, "{:?}", r.warnings);
        assert_eq!(risky_lines(&r.files), [("PKGBUILD".to_string(), 1, "curl http://e.org | sh".to_string())]);
        assert!(review("other", &c, files(s, "")).unwrap_err().contains("not a package"));
    }

    #[test]
    fn plan_adds_official_deps_and_build_tools() {
        let r = rev("zen", "\tdepends = gtk3\n\tmakedepends = go>=1.24\n\tmakedepends = gtk3\n");
        let p = plan(&[r], &["zen".into()], &official()).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].deps, ["base-devel", "git", "gtk3", "go"]);
        assert_eq!(p[0].build_deps, ["base-devel", "git", "go"], "gtk3 is also needed at run time");
        assert!(!p[0].as_dep);
        let mut c = config::Config { aur: p, ..empty_config() };
        c.services = vec!["NetworkManager.service".into()];
        c.validate().unwrap();
    }

    fn empty_config() -> config::Config {
        config::Config {
            hostname: "h".into(),
            timezone: "UTC".into(),
            locale: "en_US.UTF-8".into(),
            keymap: "us".into(),
            disk: config::Disk { model: None, confirm_serial: "S".into(), auto_largest: false, esp_mib: 512 },
            mirrors: vec!["https://e.org/$repo/os/$arch".into()],
            packages: vec!["base".into()],
            providers: vec![],
            root_password_hash: None,
            users: vec![],
            services: vec![],
            kernel_params: vec![],
            user_files: vec![],
            user_archives: vec![],
            dry_run: false,
            scripts: vec![],
            aur: vec![],
        }
    }

    #[test]
    fn aur_dependencies_come_first_and_become_dependencies() {
        let top = rev("top", "\tdepends = lib-aur\n\tdepends = glibc\n");
        let lib = rev("lib-aur", "\tprovides = libx\n");
        let p = plan(&[top, lib], &["top".into()], &official()).unwrap();
        assert_eq!(p.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["lib-aur", "top"]);
        assert!(p[0].as_dep && !p[1].as_dep);
        // Through `provides`.
        let top = rev("top", "\tdepends = libx\n");
        let lib = rev("lib-aur", "\tprovides = libx\n");
        assert_eq!(plan(&[top, lib], &["top".into()], &official()).unwrap().len(), 2);
    }

    #[test]
    fn plan_errors_are_specific() {
        let top = rev("top", "\tdepends = nowhere\n");
        assert_eq!(unresolved(&[top.clone()], &official()), [("top".to_string(), "nowhere".to_string())]);
        assert!(plan(&[top.clone()], &["top".into()], &official()).unwrap_err().contains("nowhere"));
        assert!(plan(&[], &["top".into()], &official()).unwrap_err().contains("not been reviewed"));
        let a = rev("a", "\tdepends = b\n");
        let b = rev("b", "\tdepends = a\n");
        assert!(plan(&[a, b], &["a".into()], &official()).unwrap_err().contains("cycle"));
        assert!(plan(&[rev("x", "")], &["x".into()], &Official::from_names(&["git"])).unwrap_err().contains("base-devel"));
    }

    #[test]
    fn official_prefers_real_names_over_provides() {
        let mk = |name: &str, provides: &[&str]| {
            let mut d = format!("%NAME%\n{name}\n\n%VERSION%\n1-1\n\n%FILENAME%\n{name}.pkg.tar.zst\n\n");
            if !provides.is_empty() {
                d.push_str("%PROVIDES%\n");
                for p in provides {
                    d.push_str(&format!("{p}\n"));
                }
            }
            pkg::desc::Package::parse("core", &d).unwrap()
        };
        let mut db = pkg::db::Db { name: "core".into(), packages: vec![mk("a", &["ttf-font"]), mk("ttf-font", &[])] };
        db.name = "core".into();
        let o = Official::new(&[db]);
        assert_eq!(o.provider("ttf-font"), Some("ttf-font"));
        assert_eq!(o.provider("a>=1"), Some("a"));
    }

    fn official_with(pkgs: &[(&str, &str, &[&str])]) -> Official {
        let packages = pkgs
            .iter()
            .map(|(name, version, provides)| {
                let mut d = format!("%NAME%\n{name}\n\n%VERSION%\n{version}\n\n%FILENAME%\n{name}.pkg.tar.zst\n\n");
                if !provides.is_empty() {
                    d.push_str("%PROVIDES%\n");
                    for p in *provides {
                        d.push_str(&format!("{p}\n"));
                    }
                }
                pkg::desc::Package::parse("core", &d).unwrap()
            })
            .collect();
        Official::new(&[pkg::db::Db { name: "core".into(), packages }])
    }

    #[test]
    fn official_checks_version_constraints() {
        let o = official_with(&[("go", "2:1.23.4-1", &[]), ("gcc-go", "14.2-1", &["go=1.24"]), ("libfoo", "6-1", &["libfoo.so=6-64", "libbar"])]);
        assert_eq!(o.provider("go"), Some("go"));
        assert_eq!(o.provider("go>=1.20"), Some("go"));
        // The epoch counts: 2:1.23.4 is newer than 1.30.
        assert_eq!(o.provider("go>=1.30"), Some("go"));
        assert_eq!(o.provider("go>=3:1"), None);
        // A real `go` that is too old does not hide a provide that is new enough.
        let o2 = official_with(&[("go", "1.23-1", &[]), ("gcc-go", "14.2-1", &["go=1.24"])]);
        assert_eq!(o2.provider("go>=1.24"), Some("gcc-go"));
        assert_eq!(o2.provider("go>=1.25"), None);
        // Versioned provides.
        assert_eq!(o.provider("libfoo.so=6-64"), Some("libfoo"));
        assert_eq!(o.provider("libfoo.so=7-64"), None);
        // An unversioned provide cannot satisfy a versioned dependency, but satisfies a plain one.
        assert_eq!(o.provider("libbar"), Some("libfoo"));
        assert_eq!(o.provider("libbar>=1"), None);
        // Without databases versions are unknown and accepted.
        assert_eq!(Official::from_names(&["go"]).provider("go>=99"), Some("go"));
    }

    #[test]
    fn unmet_constraints_are_unresolved() {
        let o = official_with(&[("base-devel", "1-1", &[]), ("git", "2-1", &[]), ("foo", "1.0-1", &[])]);
        let top = rev("top", "\tdepends = foo>=2\n");
        assert_eq!(unresolved(&[top.clone()], &o), [("top".to_string(), "foo>=2".to_string())]);
        assert!(plan(&[top], &["top".into()], &o).unwrap_err().contains("foo>=2"));
        assert!(unresolved(&[rev("top", "\tdepends = foo>=1\n")], &o).is_empty());
        // Another reviewed AUR package must be new enough too (the helper's version is 1-1).
        let lib = rev("foo2", "\tprovides = libx=1\n");
        let needs_new = rev("top", "\tdepends = libx>=2\n");
        assert_eq!(unresolved(&[needs_new, lib.clone()], &o).len(), 1);
        let needs_ok = rev("top", "\tdepends = libx>=1\n");
        assert!(unresolved(&[needs_ok, lib], &o).is_empty());
    }
}
