//! Official packages (core and extra) in the GUI: a local search over the parsed sync databases, the live
//! total of what the installer will download, and importing this machine's package list. The databases are
//! fetched once on a thread (the same files and cache as the resolution preview); every search afterwards
//! is a pure function over the loaded index, so it runs per keystroke without the network and is tested
//! without a display. The resolver decides what a name means; nothing here validates a config.
use hostcfg::resolve::Catalog;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

pub type Reply<T> = Result<T, String>;

/// Shortest query that searches (one letter matches half the repository).
pub const MIN_QUERY: usize = 2;
/// Rows shown for a query.
pub const LIMIT: usize = 30;

/// One searchable package: the highest-priority repository's entry for its name.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub name: String,
    pub version: String,
    pub repo: String,
    pub description: String,
    pub groups: Vec<String>,
    pub csize: u64,
    pub isize: u64,
    lname: String,
    ldesc: String,
}

/// The loaded databases: what the search reads and what the resolver resolves against.
pub struct Index {
    pub items: Vec<Candidate>,
    pub catalog: Catalog,
    /// The mirror it came from, so the page can say when the configured one changed since.
    pub mirror: String,
}

impl Index {
    pub fn new(catalog: Catalog, mirror: &str) -> Index {
        let mut items: Vec<Candidate> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Repository priority: a name in core hides the same name in extra, like the resolver's `find`.
        for (db, descs) in catalog.dbs.iter().zip(&catalog.descriptions) {
            for (p, d) in db.packages.iter().zip(descs) {
                if seen.insert(p.name.clone()) {
                    items.push(Candidate { name: p.name.clone(), version: p.version.clone(), repo: p.repo.clone(), description: d.clone(), groups: p.groups.clone(), csize: p.csize, isize: p.isize, lname: p.name.to_lowercase(), ldesc: d.to_lowercase() });
                }
            }
        }
        Index { items, catalog, mirror: mirror.to_string() }
    }

    /// Whether the repositories have a package or a group with this exact name (what `packages` accepts).
    pub fn knows(&self, name: &str) -> bool {
        self.items.iter().any(|c| c.name == name || c.groups.iter().any(|g| g == name))
    }
}

/// Why a package matched, best first: the order the results are shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Match {
    Exact,
    Prefix,
    InName,
    /// A name within a small edit distance (`fierfox` for `firefox`).
    Typo,
    Group,
    Description,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Index into `Index::items`.
    pub item: usize,
    pub why: Match,
}

/// Edit distance with adjacent transpositions (optimal string alignment), over characters.
fn distance(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

/// Typos allowed for a query of `len` characters: none below 4 (too many short names are one edit apart).
fn typo_budget(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=6 => 1,
        _ => 2,
    }
}

/// How `c` matches the lower-cased `q` (its characters in `qc`, its words in `words`), with the typo distance.
fn classify(c: &Candidate, q: &str, qc: &[char], words: &[&str]) -> Option<(Match, usize)> {
    if c.lname == q {
        return Some((Match::Exact, 0));
    }
    if c.lname.starts_with(q) {
        return Some((Match::Prefix, 0));
    }
    if c.lname.contains(q) {
        return Some((Match::InName, 0));
    }
    let budget = typo_budget(qc.len());
    if budget > 0 {
        let nc: Vec<char> = c.lname.chars().collect();
        // Against the whole name, and against its start, so a misspelt beginning (`fierf`) still finds it.
        let mut d = distance(qc, &nc);
        if nc.len() > qc.len() {
            d = d.min(distance(qc, &nc[..qc.len()]));
        }
        if d <= budget {
            return Some((Match::Typo, d));
        }
    }
    if c.groups.iter().any(|g| g.to_lowercase().contains(q)) {
        return Some((Match::Group, 0));
    }
    if !words.is_empty() && words.iter().all(|w| c.ldesc.contains(w) || c.lname.contains(w)) {
        return Some((Match::Description, 0));
    }
    None
}

/// The best `limit` matches for `query`: exact name, then prefix, substring of the name, a near miss of
/// the name, a group, a description containing every word. Within a kind: fewer typos, shorter names,
/// then by name. Empty for a query shorter than [`MIN_QUERY`] characters. Each package appears once.
pub fn search(items: &[Candidate], query: &str, limit: usize) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let qc: Vec<char> = q.chars().collect();
    if qc.len() < MIN_QUERY {
        return Vec::new();
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<(Match, usize, usize)> = items.iter().enumerate().filter_map(|(i, c)| classify(c, &q, &qc, &words).map(|(m, d)| (m, d, i))).collect();
    hits.sort_by(|a, b| (a.0, a.1, items[a.2].name.len(), &items[a.2].name).cmp(&(b.0, b.1, items[b.2].name.len(), &items[b.2].name)));
    hits.truncate(limit);
    hits.into_iter().map(|(why, _, item)| Hit { item, why }).collect()
}

/// `pacman -Qq...` output: one package name per line. Anything that is not a package name is dropped, and
/// each name is kept once, in order.
pub fn parse_package_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines().map(str::trim) {
        let ok = !l.is_empty() && !l.starts_with(['-', '.']) && l.chars().all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c));
        if ok && !out.iter().any(|x| x == l) {
            out.push(l.to_string());
        }
    }
    out
}

/// What importing this machine's package list does to `packages`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportReport {
    /// Appended to the list.
    pub added: Vec<String>,
    /// Installed and already in the list.
    pub already: usize,
    /// Installed natively but not in core or extra (other repositories, or renamed or dropped since).
    pub not_official: Vec<String>,
    /// Installed from outside the repositories (`pacman -Qm`, mostly AUR): shown, never pinned.
    pub foreign: Vec<String>,
}

/// Merges the native names that the repositories know into `existing` (order kept, no duplicates).
pub fn plan_import(native: &[String], foreign: &[String], existing: &[String], knows: impl Fn(&str) -> bool) -> ImportReport {
    let mut r = ImportReport { foreign: foreign.to_vec(), ..Default::default() };
    for n in native {
        if existing.contains(n) || r.added.contains(n) {
            r.already += 1;
        } else if knows(n) {
            r.added.push(n.clone());
        } else {
            r.not_official.push(n.clone());
        }
    }
    r
}

/// `pacman -Qq<flags>`: the names, or why pacman could not be asked. Only package names are read.
fn pacman_names(flags: &str) -> Reply<Vec<String>> {
    let out = std::process::Command::new("pacman").arg(format!("-Qq{flags}")).output().map_err(|e| format!("pacman could not be run ({e}); importing needs an Arch-based system"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    // pacman exits 1 when the filter matches nothing; that is an empty list, not an error.
    if !out.status.success() && !(out.status.code() == Some(1) && text.trim().is_empty()) {
        return Err(format!("pacman -Qq{flags} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(parse_package_list(&text))
}

/// What the resolver says the config installs: (packages, bytes to download).
pub type Total = Reply<(usize, u64)>;
/// `pacman -Qqen` and `pacman -Qqem`: (native, foreign) package names.
type Installed = Reply<(Vec<String>, Vec<String>)>;

#[derive(Default)]
pub struct OfficialState {
    pub query: String,
    pub index: Option<Reply<Arc<Index>>>,
    pub index_rx: Option<Receiver<Reply<Index>>>,
    pub total: Option<Total>,
    total_rx: Option<Receiver<Total>>,
    /// The inputs of the last total started (packages, providers, AUR dependencies).
    total_key: Option<String>,
    /// `pacman` lists waiting for the databases, then the report.
    import_raw: Option<Installed>,
    import_rx: Option<Receiver<Installed>>,
    pub import: Option<Reply<ImportReport>>,
}

/// What `poll` saw finish.
#[derive(Default)]
pub struct Polled {
    pub changed: bool,
    /// An import changed `packages`: the page holding the list editor must be rebuilt.
    pub packages_changed: bool,
}

impl OfficialState {
    pub fn loading(&self) -> bool {
        self.index_rx.is_some()
    }

    pub fn loaded(&self) -> Option<Arc<Index>> {
        match &self.index {
            Some(Ok(i)) => Some(i.clone()),
            _ => None,
        }
    }

    pub fn importing(&self) -> bool {
        self.import_rx.is_some() || self.import_raw.is_some()
    }

    pub fn resolving(&self) -> bool {
        self.total_rx.is_some()
    }

    /// Fetches and parses core and extra from `mirror` (cached for an hour in `cache`).
    pub fn start_load(&mut self, mirror: String, cache: PathBuf) {
        if self.loading() {
            return;
        }
        let (tx, rx) = channel();
        self.index_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(hostcfg::resolve::fetch_catalog(&mirror, &cache).map(|c| Index::new(c, &mirror)).map_err(|e| e.to_string()));
        });
    }

    /// Reads this machine's explicitly installed packages; the report is made once the databases are there.
    pub fn start_import(&mut self) {
        let (tx, rx) = channel();
        self.import_rx = Some(rx);
        self.import = None;
        std::thread::spawn(move || {
            let _ = tx.send(pacman_names("en").and_then(|native| pacman_names("em").map(|foreign| (native, foreign))));
        });
    }

    /// Takes finished work; starts a new total when the config's package inputs changed.
    pub fn poll(&mut self, cfg: &mut config::Config) -> Polled {
        let mut p = Polled::default();
        if let Some(rx) = &self.index_rx {
            if let Ok(r) = rx.try_recv() {
                self.index_rx = None;
                self.index = Some(r.map(Arc::new));
                // New databases: the old total was against other data.
                self.total_key = None;
                p.changed = true;
            }
        }
        if let Some(rx) = &self.total_rx {
            if let Ok(r) = rx.try_recv() {
                self.total_rx = None;
                self.total = Some(r);
                p.changed = true;
            }
        }
        if let Some(rx) = &self.import_rx {
            if let Ok(r) = rx.try_recv() {
                self.import_rx = None;
                self.import_raw = Some(r);
                p.changed = true;
            }
        }
        // The pacman lists are in: merge them once the databases are loaded (or report why they are not).
        if self.import_raw.is_some() && !self.loading() {
            let raw = self.import_raw.take().unwrap_or_else(|| Err(String::new()));
            self.import = Some(match (raw, &self.index) {
                (Err(e), _) => Err(e),
                (Ok(_), Some(Err(e))) => Err(format!("the package databases are not loaded ({e})")),
                (Ok(_), None) => Err("the package databases are not loaded".into()),
                (Ok((native, foreign)), Some(Ok(idx))) => {
                    let r = plan_import(&native, &foreign, &cfg.packages, |n| idx.knows(n));
                    if !r.added.is_empty() {
                        cfg.packages.extend(r.added.iter().cloned());
                        p.packages_changed = true;
                    }
                    Ok(r)
                }
            });
            p.changed = true;
        }
        // The live total: resolve on a thread whenever what the installer resolves has changed. One at a
        // time; the 100 ms tick that calls this is the debounce.
        if let (Some(idx), None) = (self.loaded(), &self.total_rx) {
            let key = format!("{:?}|{:?}|{:?}", cfg.packages, cfg.providers, cfg.aur.iter().map(|a| &a.deps).collect::<Vec<_>>());
            if self.total_key.as_deref() != Some(key.as_str()) {
                self.total_key = Some(key);
                let cfg = cfg.clone();
                let (tx, rx) = channel();
                self.total_rx = Some(rx);
                std::thread::spawn(move || {
                    let _ = tx.send(hostcfg::resolve::resolve(&cfg, &idx.catalog.dbs).map(|r| (r.packages.len(), r.download_bytes())).map_err(|e| e.to_string()));
                });
                p.changed = true;
            }
        }
        p
    }
}

/// `1.4 MiB`, `812 KiB`.
pub fn size(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{:.1} MiB", bytes as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} KiB", bytes.div_ceil(1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, desc: &str, groups: &[&str]) -> Candidate {
        Candidate { name: name.into(), version: "1-1".into(), repo: "extra".into(), description: desc.into(), groups: groups.iter().map(|g| g.to_string()).collect(), csize: 0, isize: 0, lname: name.to_lowercase(), ldesc: desc.to_lowercase() }
    }

    fn items() -> Vec<Candidate> {
        vec![
            c("firefox-developer-edition", "Developer Edition of the Firefox browser", &[]),
            c("firefox", "Fast, Private & Safe Web Browser", &[]),
            c("firefox-i18n-de", "German language pack for Firefox", &[]),
            c("librewolf", "Community-maintained fork of Firefox", &[]),
            c("xfce4-panel", "Panel for the Xfce desktop environment", &["xfce4"]),
            c("thunar", "Modern, fast and easy-to-use file manager for Xfce", &["xfce4"]),
            c("vim", "Vi Improved, a highly configurable, improved version of the vi text editor", &[]),
            c("neovim", "Fork of Vim aiming to improve user experience, plugins, and GUIs", &[]),
            c("gvim", "Vi Improved, a highly configurable, improved version of the vi text editor (with advanced features, such as a GUI)", &[]),
            c("noto-fonts-cjk", "Google Noto CJK fonts — 中文 日本語 한국어", &[]),
        ]
    }

    fn names(items: &[Candidate], q: &str) -> Vec<String> {
        search(items, q, LIMIT).into_iter().map(|h| items[h.item].name.clone()).collect()
    }

    #[test]
    fn exact_then_prefix_then_substring() {
        let it = items();
        let n = names(&it, "firefox");
        assert_eq!(n[0], "firefox", "exact first: {n:?}");
        // Prefix matches, shorter names first, before names that only contain it and descriptions.
        assert_eq!(&n[1..3], ["firefox-i18n-de", "firefox-developer-edition"]);
        assert_eq!(n.last().map(String::as_str), Some("librewolf"), "description last: {n:?}");
        let n = names(&it, "vim");
        assert_eq!(n[..3], ["vim", "gvim", "neovim"], "{n:?}");
        let hits = search(&it, "vim", LIMIT);
        assert_eq!(hits[0].why, Match::Exact);
        assert_eq!(hits[1].why, Match::InName);
    }

    #[test]
    fn groups_and_descriptions() {
        let it = items();
        let hits = search(&it, "xfce4", LIMIT);
        assert_eq!(it[hits[0].item].name, "xfce4-panel");
        assert_eq!(hits[0].why, Match::Prefix);
        assert!(hits.iter().any(|h| it[h.item].name == "thunar" && h.why == Match::Group), "{hits:?}");
        // Every word must appear, in any order.
        assert_eq!(names(&it, "file manager"), ["thunar"]);
        assert_eq!(names(&it, "manager file"), ["thunar"]);
        assert!(names(&it, "file spreadsheet").is_empty());
    }

    #[test]
    fn typos_find_the_name() {
        let it = items();
        let hits = search(&it, "fierfox", LIMIT);
        assert_eq!(it[hits[0].item].name, "firefox");
        assert_eq!(hits[0].why, Match::Typo);
        assert_eq!(names(&it, "neovmi")[0], "neovim");
        assert_eq!(names(&it, "Thunra")[0], "thunar", "case and a swap");
        // A misspelt start of a longer name.
        assert!(names(&it, "fierfo").contains(&"firefox".to_string()));
        // Short queries get no typo budget: `vin` must not match `vim`.
        assert!(!names(&it, "vin").contains(&"vim".to_string()));
        assert!(names(&it, "qqqqqqqq").is_empty());
    }

    #[test]
    fn short_or_empty_queries_search_nothing() {
        let it = items();
        assert!(names(&it, "").is_empty());
        assert!(names(&it, "   ").is_empty());
        assert!(names(&it, "v").is_empty());
        assert!(names(&it, "中").is_empty(), "one character, even if it is a wide one");
        assert!(!names(&it, "vi").is_empty());
    }

    #[test]
    fn unicode_in_descriptions() {
        let it = items();
        assert_eq!(names(&it, "日本語"), ["noto-fonts-cjk"]);
        assert_eq!(names(&it, "CJK 한국어"), ["noto-fonts-cjk"]);
        // Non-ASCII queries go through the typo path by characters, not bytes: two substitutions.
        assert_eq!(search(&it, "ñoto-fönts", LIMIT).iter().map(|h| (it[h.item].name.as_str(), h.why)).collect::<Vec<_>>(), [("noto-fonts-cjk", Match::Typo)]);
    }

    #[test]
    fn each_package_once_and_limited() {
        let mut it = items();
        // `vim` matches by name and by description: still one row.
        let n = names(&it, "vim");
        let mut d = n.clone();
        d.dedup();
        assert_eq!(n.len(), d.len());
        assert_eq!(n.iter().filter(|x| *x == "vim").count(), 1);
        for i in 0..100 {
            it.push(c(&format!("vim-plugin-{i}"), "", &[]));
        }
        assert_eq!(search(&it, "vim", LIMIT).len(), LIMIT);
    }

    #[test]
    fn index_prefers_the_first_repository() {
        let mk = |repo: &str, name: &str, version: &str| {
            pkg::desc::Package::parse(repo, &format!("%NAME%\n{name}\n\n%VERSION%\n{version}\n\n%FILENAME%\n{name}.pkg.tar.zst\n\n%GROUPS%\nbase-devel\n\n")).unwrap()
        };
        let core = pkg::db::Db { name: "core".into(), packages: vec![mk("core", "make", "4.4-1")] };
        let extra = pkg::db::Db { name: "extra".into(), packages: vec![mk("extra", "make", "9-1"), mk("extra", "cmake", "3-1")] };
        let idx = Index::new(Catalog { dbs: vec![core, extra], descriptions: vec![vec!["GNU make".into()], vec!["".into(), "".into()]] }, "m");
        assert_eq!(idx.items.len(), 2);
        assert_eq!((idx.items[0].repo.as_str(), idx.items[0].version.as_str(), idx.items[0].description.as_str()), ("core", "4.4-1", "GNU make"));
        assert!(idx.knows("make") && idx.knows("base-devel") && !idx.knows("yay"));
    }

    #[test]
    fn import_parses_and_merges() {
        let native = parse_package_list("base\nlinux\n\nfirefox\nfirefox\n  vim  \n-not-a-name\nweird name\nmy-local-pkg\n");
        assert_eq!(native, ["base", "linux", "firefox", "vim", "my-local-pkg"]);
        let foreign = parse_package_list("yay-bin\nzen-browser-bin\n");
        let known = ["base", "linux", "firefox", "vim"];
        let r = plan_import(&native, &foreign, &["base".into(), "vim".into()], |n| known.contains(&n));
        assert_eq!(r.added, ["linux", "firefox"]);
        assert_eq!(r.already, 2);
        assert_eq!(r.not_official, ["my-local-pkg"]);
        assert_eq!(r.foreign, ["yay-bin", "zen-browser-bin"]);
        // Nothing installed, or pacman printed nothing: an empty report.
        assert_eq!(plan_import(&parse_package_list(""), &[], &[], |_| true), ImportReport::default());
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(size(0), "0 KiB");
        assert_eq!(size(1500), "2 KiB");
        assert_eq!(size(3 << 20), "3.0 MiB");
        assert_eq!(size(5 << 30), "5.0 GiB");
    }
}
