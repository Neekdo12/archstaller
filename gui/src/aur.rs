//! AUR packages in the GUI (plans-implement/aur.md): search, review, pin. Network work runs on a thread; the UI
//! polls the channels. Nothing is built here: the installed system builds the pinned recipe at its first boot.
use aurbuild::plan::{plan, unresolved, Official, Review};
use aurbuild::rpc::{self, Info};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

pub type Reply<T> = Result<T, String>;

#[derive(Default)]
pub struct AurState {
    pub search: String,
    pub results: Option<Reply<Vec<Info>>>,
    pub search_rx: Option<Receiver<Reply<Vec<Info>>>>,
    /// The recipe being reviewed.
    pub review: Option<Review>,
    pub review_rx: Option<Receiver<Reply<Review>>>,
    /// Which package is being fetched, for the spinner text.
    pub fetching: Option<String>,
    pub file: String,
    pub ack_reviewed: bool,
    pub ack_vcs: bool,
    pub pin_rx: Option<Receiver<Reply<Pinned>>>,
    pub message: Option<Reply<String>>,
}

/// The result of pinning: the whole new package list and a summary line.
pub struct Pinned {
    pub entries: Vec<config::AurPackage>,
    pub summary: String,
}

impl AurState {
    pub fn busy(&self) -> bool {
        self.search_rx.is_some() || self.review_rx.is_some() || self.pin_rx.is_some()
    }

    pub fn start_search(&mut self) {
        let term = self.search.trim().to_string();
        let (tx, rx) = channel();
        self.search_rx = Some(rx);
        self.results = None;
        std::thread::spawn(move || {
            let _ = tx.send(rpc::search(&term).map(|mut v| {
                v.sort_by(|a, b| b.popularity.partial_cmp(&a.popularity).unwrap_or(std::cmp::Ordering::Equal));
                v.truncate(12);
                v
            }));
        });
    }

    pub fn start_review(&mut self, info: &Info) {
        let (name, base) = (info.name.clone(), info.pkgbase.clone());
        let (tx, rx) = channel();
        self.review_rx = Some(rx);
        self.fetching = Some(name.clone());
        self.review = None;
        self.ack_reviewed = false;
        self.ack_vcs = false;
        self.message = None;
        std::thread::spawn(move || {
            let _ = tx.send(rpc::fetch_review(&name, &base, None));
        });
    }

    /// Pins the reviewed recipe: re-checks the packages that are already pinned, resolves every dependency
    /// against the official repositories and plans the order.
    pub fn start_pin(&mut self, review: Review, existing: Vec<config::AurPackage>, mirror: String, cache: PathBuf) {
        let (tx, rx) = channel();
        self.pin_rx = Some(rx);
        self.message = None;
        std::thread::spawn(move || {
            let _ = tx.send(pin(review, existing, &mirror, &cache));
        });
    }

    /// Takes finished work. Returns the new package list when a pin completed.
    pub fn poll(&mut self) -> Option<Pinned> {
        if let Some(rx) = &self.search_rx {
            if let Ok(r) = rx.try_recv() {
                self.search_rx = None;
                self.results = Some(r);
            }
        }
        if let Some(rx) = &self.review_rx {
            if let Ok(r) = rx.try_recv() {
                self.review_rx = None;
                self.fetching = None;
                match r {
                    Ok(rev) => {
                        self.file = if rev.files.contains_key("PKGBUILD") { "PKGBUILD".into() } else { rev.files.keys().next().cloned().unwrap_or_default() };
                        self.review = Some(rev);
                    }
                    Err(e) => self.message = Some(Err(e)),
                }
            }
        }
        if let Some(rx) = &self.pin_rx {
            if let Ok(r) = rx.try_recv() {
                self.pin_rx = None;
                match r {
                    Ok(p) => {
                        self.message = Some(Ok(p.summary.clone()));
                        self.review = None;
                        return Some(p);
                    }
                    Err(e) => self.message = Some(Err(e)),
                }
            }
        }
        None
    }
}

/// The worker of [`AurState::start_pin`].
pub fn pin(review: Review, existing: Vec<config::AurPackage>, mirror: &str, cache: &std::path::Path) -> Reply<Pinned> {
    // The packages that are already pinned must still be the recipes that were reviewed.
    let mut reviews: Vec<Review> = Vec::new();
    for e in existing.iter().filter(|e| e.name != review.name) {
        let r = aurbuild::plan::review(&e.name, &e.commit, rpc::snapshot(&e.commit)?)?;
        if r.tree_sha256 != e.sha256 {
            return Err(format!("{}: the recipe at the pinned commit {} no longer matches its pin; remove it and review it again", e.name, &e.commit[..8]));
        }
        reviews.push(r);
    }
    let name = review.name.clone();
    reviews.push(review);
    let dbs = hostcfg::resolve::fetch_dbs(mirror, cache).map_err(|e| e.to_string())?;
    let official = Official::new(&dbs);
    let missing = unresolved(&reviews, &official);
    if !missing.is_empty() {
        let list: Vec<String> = missing.iter().map(|(owner, dep)| format!("{dep} (needed by {owner})")).collect();
        return Err(format!("not in the official repositories and not pinned yet: {}. Search the AUR for each, review it and pin it first.", list.join(", ")));
    }
    let explicit: Vec<String> = reviews.iter().map(|r| r.name.clone()).collect();
    let mut entries = plan(&reviews, &explicit, &official)?;
    // Keep what the user typed for the packages that were already there.
    for e in &mut entries {
        if let Some(old) = existing.iter().find(|o| o.name == e.name) {
            e.services = old.services.clone();
        }
    }
    let added = entries.iter().find(|e| e.name == name).map(|e| e.deps.len()).unwrap_or(0);
    Ok(Pinned { summary: format!("Pinned {name}. The plan has {} AUR package(s) and adds {added} official package(s) it needs.", entries.len()), entries })
}

/// Whether the config has a network service the first boot build can rely on.
pub fn has_network_service(services: &[String]) -> bool {
    services.iter().any(|u| ["NetworkManager.service", "systemd-networkd.service", "dhcpcd.service", "connman.service"].contains(&u.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_service_detection() {
        assert!(has_network_service(&["sshd.service".into(), "NetworkManager.service".into()]));
        assert!(!has_network_service(&["sshd.service".into()]));
    }
}
