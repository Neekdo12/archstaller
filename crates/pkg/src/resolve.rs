//! Dependency resolution across repositories in priority order.
use crate::db::Db;
use crate::desc::{Dep, Package};
use crate::{Error, Result};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub struct Selected<'a> {
    pub pkg: &'a Package,
    /// Requested by name or group (installed explicit) vs pulled in as a dependency.
    pub explicit: bool,
}

/// A dependency several packages could satisfy where the config did not pick one.
/// The first candidate (repo priority, then name) was chosen, like pacman's default answer.
#[derive(Debug, Clone)]
pub struct Ambiguity {
    pub dep: String,
    pub chosen: String,
    pub candidates: Vec<String>,
}

pub struct Resolution<'a> {
    pub packages: Vec<Selected<'a>>,
    pub ambiguities: Vec<Ambiguity>,
}

pub struct Resolver<'a> {
    dbs: &'a [Db],
    /// dependency name -> chosen package name
    providers: &'a BTreeMap<String, String>,
}

impl<'a> Resolver<'a> {
    /// `dbs` are in priority order (first wins).
    pub fn new(dbs: &'a [Db], providers: &'a BTreeMap<String, String>) -> Self {
        Resolver { dbs, providers }
    }

    fn all(&self) -> impl Iterator<Item = &'a Package> + '_ {
        self.dbs.iter().flat_map(|d| d.packages.iter())
    }

    /// First package with exactly this name, honoring repo priority.
    pub fn find(&self, name: &str) -> Option<&'a Package> {
        self.all().find(|p| p.name == name)
    }

    fn expand_request(&self, req: &str) -> Result<Vec<&'a Package>> {
        if let Some(p) = self.find(req) {
            return Ok(alloc::vec![p]);
        }
        // Group members; the highest-priority repo's version of each name wins.
        let mut out: Vec<&Package> = Vec::new();
        for p in self.all().filter(|p| p.groups.iter().any(|g| g == req)) {
            if !out.iter().any(|q| q.name == p.name) {
                out.push(p);
            }
        }
        if out.is_empty() {
            Err(Error::UnknownPackage(req.into()))
        } else {
            Ok(out)
        }
    }

    fn pick_for_dep(&self, dep: &Dep, selected: &[Selected<'a>], amb: &mut Vec<Ambiguity>) -> Result<&'a Package> {
        // Already selected packages satisfy it.
        if let Some(s) = selected.iter().find(|s| s.pkg.satisfies(dep)) {
            return Ok(s.pkg);
        }
        // Exact name in the highest-priority repo that has it.
        if let Some(p) = self.find(&dep.name) {
            if p.satisfies(dep) {
                return Ok(p);
            }
        }
        if let Some(name) = self.providers.get(&dep.name) {
            let p = self.find(name).ok_or_else(|| Error::UnknownPackage(name.clone()))?;
            if p.satisfies(dep) {
                return Ok(p);
            }
            return Err(Error::Unsatisfied(dep.name.clone()));
        }
        let mut cands: Vec<&Package> = Vec::new();
        for db in self.dbs {
            let mut in_repo: Vec<&Package> = db.packages.iter().filter(|p| p.satisfies(dep)).collect();
            in_repo.sort_by(|a, b| a.name.cmp(&b.name));
            for p in in_repo {
                if !cands.iter().any(|q| q.name == p.name) {
                    cands.push(p);
                }
            }
        }
        match cands.len() {
            0 => Err(Error::Unsatisfied(dep.name.clone())),
            1 => Ok(cands[0]),
            _ => {
                let names: Vec<String> = cands.iter().map(|p| p.name.clone()).collect();
                if !amb.iter().any(|a| a.dep == dep.name) {
                    amb.push(Ambiguity { dep: dep.name.clone(), chosen: names[0].clone(), candidates: names });
                }
                Ok(cands[0])
            }
        }
    }

    /// Resolves `requested` (package or group names). The result lists dependencies before their
    /// dependents.
    pub fn resolve(&self, requested: &[String]) -> Result<Resolution<'a>> {
        // Phase 1: explicit targets, so their provides count as "already selected" for phase 2.
        let mut selected: Vec<Selected<'a>> = Vec::new();
        for r in requested {
            for p in self.expand_request(r)? {
                if !selected.iter().any(|s| s.pkg.name == p.name) {
                    selected.push(Selected { pkg: p, explicit: true });
                }
            }
        }
        // Phase 2: pull in dependencies breadth-first, remembering how each was reached.
        let mut ambiguities = Vec::new();
        let mut i = 0;
        while i < selected.len() {
            let pkg = selected[i].pkg;
            for dep in &pkg.depends {
                let p = self.pick_for_dep(dep, &selected, &mut ambiguities)?;
                if !selected.iter().any(|s| s.pkg.name == p.name) {
                    selected.push(Selected { pkg: p, explicit: false });
                }
            }
            i += 1;
        }
        self.check_conflicts(&selected)?;
        Ok(Resolution { packages: self.order(selected), ambiguities })
    }

    fn check_conflicts(&self, sel: &[Selected<'a>]) -> Result<()> {
        for a in sel {
            for c in &a.pkg.conflicts {
                for b in sel {
                    if core::ptr::eq(a.pkg, b.pkg) {
                        continue;
                    }
                    if b.pkg.satisfies(c) {
                        return Err(Error::Conflict(a.pkg.name.clone(), b.pkg.name.clone()));
                    }
                }
            }
        }
        Ok(())
    }

    /// Depth-first post-order so dependencies come first (cycles are broken arbitrarily).
    fn order(&self, sel: Vec<Selected<'a>>) -> Vec<Selected<'a>> {
        let n = sel.len();
        let mut state = alloc::vec![0u8; n]; // 0 new, 1 visiting, 2 done
        let mut out: Vec<usize> = Vec::with_capacity(n);
        fn visit(i: usize, sel: &[Selected<'_>], state: &mut [u8], out: &mut Vec<usize>) {
            if state[i] != 0 {
                return;
            }
            state[i] = 1;
            for dep in &sel[i].pkg.depends {
                if let Some(j) = sel.iter().position(|s| s.pkg.satisfies(dep)) {
                    visit(j, sel, state, out);
                }
            }
            state[i] = 2;
            out.push(i);
        }
        for i in 0..n {
            visit(i, &sel, &mut state, &mut out);
        }
        let mut slots: Vec<Option<Selected<'a>>> = sel.into_iter().map(Some).collect();
        out.into_iter().map(|i| slots[i].take().unwrap()).collect()
    }
}
