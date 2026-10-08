//! Sync database `desc` entries and dependency expressions.
use crate::vercmp::vercmp;
use crate::{Error, Result};
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Any,
    Eq,
    Ge,
    Le,
    Gt,
    Lt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    pub op: Op,
    pub version: String,
}

impl Dep {
    pub fn parse(s: &str) -> Dep {
        // Two-character operators first.
        for (tok, op) in [(">=", Op::Ge), ("<=", Op::Le), ("=", Op::Eq), (">", Op::Gt), ("<", Op::Lt)] {
            if let Some(i) = s.find(tok) {
                return Dep { name: s[..i].into(), op, version: s[i + tok.len()..].into() };
            }
        }
        Dep { name: s.into(), op: Op::Any, version: String::new() }
    }

    /// Whether `have` satisfies this constraint.
    pub fn accepts_version(&self, have: &str) -> bool {
        let ord = vercmp(have, &self.version);
        match self.op {
            Op::Any => true,
            Op::Eq => ord == Ordering::Equal,
            Op::Ge => ord != Ordering::Less,
            Op::Le => ord != Ordering::Greater,
            Op::Gt => ord == Ordering::Greater,
            Op::Lt => ord == Ordering::Less,
        }
    }
}

/// The first line of the `%SECTION%` of a `desc` entry (`field(text, "DESC")`), the same layout
/// [`Package::parse`] reads.
pub fn field<'a>(desc: &'a str, section: &str) -> Option<&'a str> {
    let mut lines = desc.lines();
    while let Some(l) = lines.next() {
        if l.strip_prefix('%').and_then(|l| l.strip_suffix('%')) == Some(section) {
            return lines.next().filter(|l| !l.is_empty());
        }
    }
    None
}

#[derive(Debug, Clone, Default)]
pub struct Package {
    pub repo: String,
    pub name: String,
    pub version: String,
    pub filename: String,
    pub csize: u64,
    pub isize: u64,
    pub sha256: String,
    pub pgpsig: String,
    pub depends: Vec<Dep>,
    pub provides: Vec<Dep>,
    pub conflicts: Vec<Dep>,
    pub replaces: Vec<Dep>,
    pub groups: Vec<String>,
}

impl Package {
    pub fn parse(repo: &str, desc: &str) -> Result<Package> {
        let mut p = Package { repo: repo.into(), ..Default::default() };
        let mut section = "";
        let mut lines: Vec<&str> = Vec::new();
        let flush = |p: &mut Package, section: &str, lines: &mut Vec<&str>| {
            let first = lines.first().copied().unwrap_or("");
            let deps = |l: &Vec<&str>| l.iter().map(|s| Dep::parse(s)).collect::<Vec<_>>();
            match section {
                "NAME" => p.name = first.into(),
                "VERSION" => p.version = first.into(),
                "FILENAME" => p.filename = first.into(),
                "CSIZE" => p.csize = first.parse().unwrap_or(0),
                "ISIZE" => p.isize = first.parse().unwrap_or(0),
                "SHA256SUM" => p.sha256 = first.into(),
                "PGPSIG" => p.pgpsig = first.into(),
                "DEPENDS" => p.depends = deps(lines),
                "PROVIDES" => p.provides = deps(lines),
                "CONFLICTS" => p.conflicts = deps(lines),
                "REPLACES" => p.replaces = deps(lines),
                "GROUPS" => p.groups = lines.iter().map(|s| String::from(*s)).collect(),
                _ => {}
            }
            lines.clear();
        };
        for line in desc.lines() {
            if let Some(name) = line.strip_prefix('%').and_then(|l| l.strip_suffix('%')) {
                flush(&mut p, section, &mut lines);
                section = name;
            } else if !line.is_empty() {
                lines.push(line);
            }
        }
        flush(&mut p, section, &mut lines);
        if p.name.is_empty() || p.version.is_empty() || p.filename.is_empty() {
            return Err(Error::Format("incomplete desc"));
        }
        Ok(p)
    }

    /// Does this package satisfy `dep`, by name or through a provide?
    pub fn satisfies(&self, dep: &Dep) -> bool {
        if self.name == dep.name && dep.accepts_version(&self.version) {
            return true;
        }
        self.provides.iter().any(|pr| {
            pr.name == dep.name
                && match (dep.op, pr.op) {
                    (Op::Any, _) => true,
                    // An unversioned provide cannot satisfy a versioned dependency.
                    (_, Op::Eq) => dep.accepts_version(&pr.version),
                    _ => false,
                }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_reads_the_first_line_of_a_section() {
        let d = "%NAME%\nfirefox\n\n%DESC%\nFast, Private & Safe Web Browser — ünïcode\n\n%ISIZE%\n1\n\n%EMPTY%\n\n";
        assert_eq!(field(d, "DESC"), Some("Fast, Private & Safe Web Browser — ünïcode"));
        assert_eq!(field(d, "NAME"), Some("firefox"));
        assert_eq!(field(d, "EMPTY"), None);
        assert_eq!(field(d, "URL"), None);
        // A value that looks like a section name is not one.
        assert_eq!(field("%DESC%\n%NAME%\n", "NAME"), None);
    }
}
