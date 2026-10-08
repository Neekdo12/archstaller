//! A parser for `.SRCINFO`, the machine-readable form of a PKGBUILD that the AUR keeps next to it.
use crate::Result;
use std::collections::BTreeMap;

/// The `key = value` lines of one section (`pkgbase` or one `pkgname`), values in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub keys: BTreeMap<String, Vec<String>>,
}

impl Section {
    pub fn get(&self, key: &str) -> &[String] {
        self.keys.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn first(&self, key: &str) -> Option<&str> {
        self.get(key).first().map(String::as_str)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrcInfo {
    pub base: Section,
    pub packages: Vec<Section>,
}

pub fn parse(text: &str) -> Result<SrcInfo> {
    let mut info = SrcInfo::default();
    let mut cur: Option<usize> = None; // None = the pkgbase section, Some(i) = packages[i]
    let mut seen_base = false;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once('=').ok_or_else(|| format!(".SRCINFO line {}: no '='", n + 1))?;
        let (k, v) = (k.trim(), v.trim());
        match k {
            "pkgbase" => {
                if seen_base {
                    return Err(format!(".SRCINFO line {}: a second pkgbase", n + 1));
                }
                seen_base = true;
                info.base.name = v.to_string();
                cur = None;
            }
            "pkgname" => {
                if !seen_base {
                    return Err(format!(".SRCINFO line {}: pkgname before pkgbase", n + 1));
                }
                info.packages.push(Section { name: v.to_string(), keys: BTreeMap::new() });
                cur = Some(info.packages.len() - 1);
            }
            _ => {
                if !seen_base {
                    return Err(format!(".SRCINFO line {}: {k} before pkgbase", n + 1));
                }
                let sec = match cur {
                    None => &mut info.base,
                    Some(i) => &mut info.packages[i],
                };
                sec.keys.entry(k.to_string()).or_default().push(v.to_string());
            }
        }
    }
    if !seen_base {
        return Err(".SRCINFO has no pkgbase".into());
    }
    if info.packages.is_empty() {
        return Err(".SRCINFO has no pkgname".into());
    }
    Ok(info)
}

impl SrcInfo {
    pub fn package(&self, name: &str) -> Option<&Section> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// `epoch:pkgver-pkgrel`.
    pub fn version(&self) -> String {
        let ver = self.base.first("pkgver").unwrap_or("?");
        let rel = self.base.first("pkgrel").unwrap_or("1");
        match self.base.first("epoch") {
            Some(e) if e != "0" => format!("{e}:{ver}-{rel}"),
            _ => format!("{ver}-{rel}"),
        }
    }

    /// Whether the recipe builds for x86_64 (`arch` lists it or `any`).
    pub fn arch_ok(&self) -> bool {
        self.base.get("arch").iter().any(|a| a == "x86_64" || a == "any")
    }

    /// Runtime dependencies of `name`: its own `depends` if it has any, else the pkgbase's (as makepkg does).
    pub fn depends(&self, name: &str) -> Vec<String> {
        let own = self.package(name).map(|p| p.get("depends")).unwrap_or(&[]);
        let list = if own.is_empty() { self.base.get("depends") } else { own };
        let mut v: Vec<String> = list.to_vec();
        v.extend(self.base.get("depends_x86_64").iter().cloned());
        v
    }

    /// Build-time dependencies: `makedepends` (and `checkdepends`, which `makepkg` only needs with `--check`,
    /// so they are left out), plus the architecture specific ones.
    pub fn makedepends(&self) -> Vec<String> {
        let mut v = self.base.get("makedepends").to_vec();
        v.extend(self.base.get("makedepends_x86_64").iter().cloned());
        v
    }

    /// Every source entry, for the `arch`-independent and the x86_64 lists.
    pub fn sources(&self) -> Vec<String> {
        let mut v = self.base.get("source").to_vec();
        v.extend(self.base.get("source_x86_64").iter().cloned());
        v
    }

    /// Sources that are not a pinned archive: `git+`, `svn+`, `hg+`, `bzr+`, `fossil+`.
    pub fn vcs_sources(&self) -> Vec<String> {
        self.sources().into_iter().filter(|s| is_vcs(s)).collect()
    }

    /// How many checksum entries are `SKIP` for sources that are not VCS ones.
    pub fn skipped_checksums(&self) -> usize {
        let sums = ["sha256sums", "sha512sums", "sha384sums", "sha224sums", "sha1sums", "md5sums", "b2sums", "cksums"];
        let mut n = 0;
        for arch in ["", "_x86_64"] {
            let src = self.base.get(&format!("source{arch}"));
            for k in sums {
                for (i, c) in self.base.get(&format!("{k}{arch}")).iter().enumerate() {
                    if c == "SKIP" && src.get(i).is_some_and(|s| !is_vcs(s)) {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    pub fn install_files(&self) -> Vec<String> {
        let mut v = self.base.get("install").to_vec();
        for p in &self.packages {
            v.extend(p.get("install").iter().cloned());
        }
        v
    }
}

/// The package name of a dependency string (`libfoo.so=6-64`, `go>=1.24`, `pacman>6.1`).
pub fn dep_name(dep: &str) -> &str {
    let end = dep.find(['<', '>', '=']).unwrap_or(dep.len());
    // `name: description` is the optdepends form; it never reaches here, but be tolerant.
    dep[..end].split(':').next().unwrap_or("").trim()
}

fn is_vcs(source: &str) -> bool {
    // `name::git+https://...` or `git+https://...`
    let url = source.rsplit_once("::").map(|(_, u)| u).unwrap_or(source);
    ["git+", "svn+", "hg+", "bzr+", "fossil+"].iter().any(|p| url.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "pkgbase = foo\n\tpkgdesc = A foo\n\tpkgver = 1.2\n\tpkgrel = 3\n\tepoch = 2\n\tarch = x86_64\n\tmakedepends = go>=1.24\n\tdepends = gtk3\n\tdepends = libfoo.so=6-64\n\tsource = https://e.org/a.tar.gz\n\tsource = git+https://e.org/r.git\n\tsha256sums = SKIP\n\tsha256sums = SKIP\n\npkgname = foo\n\npkgname = foo-extra\n\tdepends = bar\n";

    #[test]
    fn parses_sections_and_versions() {
        let i = parse(SAMPLE).unwrap();
        assert_eq!(i.base.name, "foo");
        assert_eq!(i.version(), "2:1.2-3");
        assert!(i.arch_ok());
        assert_eq!(i.depends("foo"), ["gtk3", "libfoo.so=6-64"]);
        assert_eq!(i.depends("foo-extra"), ["bar"]);
        assert_eq!(i.makedepends(), ["go>=1.24"]);
        assert_eq!(i.vcs_sources(), ["git+https://e.org/r.git"]);
        assert_eq!(i.skipped_checksums(), 1, "the SKIP of the archive counts, the VCS one does not");
    }

    #[test]
    fn rejects_broken_input() {
        assert!(parse("").is_err());
        assert!(parse("pkgname = x\n").is_err());
        assert!(parse("pkgbase = x\n").is_err());
        assert!(parse("pkgbase = x\npkgbase = y\npkgname = x\n").is_err());
        assert!(parse("pkgbase = x\nnonsense\npkgname = x\n").is_err());
    }

    #[test]
    fn dependency_names() {
        assert_eq!(dep_name("libfoo.so=6-64"), "libfoo.so");
        assert_eq!(dep_name("go>=1.24"), "go");
        assert_eq!(dep_name("pacman>6.1"), "pacman");
        assert_eq!(dep_name("plain"), "plain");
    }
}
