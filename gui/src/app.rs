//! The egui front end. All decisions live in `model`, `build`, `media` and `hostcfg`; this only draws them.
use crate::aur::{self, AurState};
use crate::build::{self, Build, Msg};
use crate::data;
use crate::media::{self, FolderCopy, MediaTarget, Phase, Volume};
use crate::model::{self, Area, Model, Preset};
use crate::style::{self, ACCENT, BAD, OK, WARN};
use eframe::egui;
use hostcfg::host::{DriverClass, DRIVERS};
use hostcfg::progress::{Event, State};
use hostcfg::resolve::Resolution;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    System,
    Disk,
    Packages,
    Users,
    Services,
    Build,
    Scripts,
    Preview,
    Iso,
}

const TABS: &[(Tab, &str, Option<Area>)] = &[
    (Tab::System, "System", Some(Area::System)),
    (Tab::Disk, "Disk", Some(Area::Disk)),
    (Tab::Packages, "Mirrors & packages", Some(Area::Packages)),
    (Tab::Users, "Users", Some(Area::Users)),
    (Tab::Services, "Services & kernel", Some(Area::Services)),
    (Tab::Build, "Build & drivers", Some(Area::Build)),
    (Tab::Scripts, "Scripts", Some(Area::Scripts)),
    (Tab::Preview, "Lua preview", None),
    (Tab::Iso, "Build ISO", None),
];

/// Suggestion lists, read once at start.
struct Lists {
    timezones: Vec<String>,
    locales: Vec<String>,
    keymaps: Vec<String>,
    shells: Vec<String>,
    groups: Vec<String>,
    services: Vec<String>,
    kernel_params: Vec<String>,
}

impl Lists {
    fn load() -> Lists {
        Lists { timezones: data::timezones(), locales: data::locales(), keymaps: data::keymaps(), shells: data::shells(), groups: data::groups(), services: data::services(), kernel_params: data::kernel_params() }
    }
}

/// The stick already holds earlier builds of this ISO: delete them, or give the new one another name.
struct OlderPrompt {
    files: Vec<PathBuf>,
    iso: PathBuf,
    volume: Volume,
    dir: PathBuf,
    /// Name the ISO gets on the stick, and the suggested other name.
    name: String,
    new_name: String,
}

enum Confirm {
    AutoLargest,
    Aur,
    Overwrite(PathBuf),
}

struct NewUser {
    name: String,
    pw: String,
    pw2: String,
    groups: String,
    shell: String,
}

impl Default for NewUser {
    fn default() -> Self {
        NewUser { name: String::new(), pw: String::new(), pw2: String::new(), groups: "wheel".into(), shell: "/bin/bash".into() }
    }
}

#[derive(Default)]
struct BuildState {
    running: Option<Build>,
    stages: Vec<(String, State)>,
    log: Vec<String>,
    error: Option<String>,
    /// The finished ISO, once it is in place and checked.
    iso: Option<(PathBuf, u64, String)>,
    out: Option<PathBuf>,
    summary: Option<String>,
    /// Build straight onto this Ventoy volume under this file name, with no copy kept elsewhere.
    stick: Option<(Volume, String)>,
    /// The `done` event, kept until the process exit arrives (they can come in different frames).
    done: Option<(String, u64, String)>,
}

#[derive(Default)]
struct MediaState {
    volumes: Vec<Volume>,
    /// Save the built ISO as a file even when a Ventoy drive is present.
    to_file: bool,
    /// Ventoy volume to build onto (index into `volumes`); the first one when unset or stale.
    stick: Option<usize>,
    last_scan: Option<std::time::Instant>,
    /// Show internal disks too, not only removable ones.
    show_internal: bool,
    /// A mount or unmount in progress (device) and its answer.
    drive_rx: Option<Receiver<Result<String, String>>>,
    drive_busy: Option<String>,
    drive_msg: Option<Result<String, String>>,
    /// Scratch build directory to remove once the ISO is safely on the stick.
    cleanup: Option<PathBuf>,
    selected: Option<usize>,
    dir: Option<PathBuf>,
    iso_override: Option<PathBuf>,
    progress: Option<(Phase, u64, u64)>,
    rx: Option<Receiver<MediaMsg>>,
    cancel: Arc<AtomicBool>,
    result: Option<Result<String, String>>,
}

enum MediaMsg {
    Progress(Phase, u64, u64),
    Done(Result<media::Report, String>),
}

pub struct App {
    model: Model,
    tab: Tab,
    status: String,
    bufs: HashMap<&'static str, String>,
    new_user: NewUser,
    new_script: (String, String, String, String), // id, file, url, sha256
    root_script_ack: bool,
    confirm: Option<Confirm>,
    older: Option<OlderPrompt>,
    lists: Lists,
    aur: AurState,
    resolve: Option<Result<Resolution, String>>,
    resolve_rx: Option<Receiver<Result<Resolution, String>>>,
    build: BuildState,
    media: MediaState,
    next_build: u32,
    presets: Vec<Preset>,
}

impl App {
    pub fn new() -> App {
        let mut a = App {
            model: Model::starter(),
            tab: Tab::System,
            status: "New config".into(),
            bufs: HashMap::new(),
            new_user: NewUser::default(),
            new_script: Default::default(),
            root_script_ack: false,
            confirm: None,
            older: None,
            lists: Lists::load(),
            aur: AurState::default(),
            resolve: None,
            resolve_rx: None,
            build: BuildState::default(),
            media: MediaState::default(),
            next_build: 0,
            presets: build::find_root().map(|r| model::presets(&r)).unwrap_or_default(),
        };
        a.media.volumes = media::volumes();
        a
    }

    fn load(&mut self, m: Model) {
        self.model = m;
        self.bufs.clear();
        self.resolve = None;
        self.root_script_ack = false;
    }

    /// A multi-line list editor (one item per line).
    fn lines(&mut self, ui: &mut egui::Ui, key: &'static str, get: impl Fn(&Model) -> Vec<String>, set: impl Fn(&mut Model, Vec<String>), height: f32) {
        let buf = self.bufs.entry(key).or_insert_with(|| get(&self.model).join("\n"));
        let r = egui::ScrollArea::vertical().id_salt(key).max_height(height).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(buf).desired_width(f32::INFINITY).desired_rows((height / 20.0) as usize).margin(egui::Margin::symmetric(8, 6)).font(egui::TextStyle::Monospace))
        });
        if r.inner.changed() {
            let v: Vec<String> = buf.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
            set(&mut self.model, v);
            self.model.dirty = true;
        }
    }

    fn field(&mut self, ui: &mut egui::Ui, label: &str, get: impl Fn(&mut Model) -> &mut String) {
        style::row(ui, label, |ui| {
            if style::text(ui, get(&mut self.model)).changed() {
                self.model.dirty = true;
            }
        });
    }

    fn save_as(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("Lua config", &["lua"]).set_file_name("archstaler.lua").save_file() {
            self.status = match self.model.save(&p) {
                Ok(()) => format!("Saved {}", p.display()),
                Err(e) => e,
            };
        }
    }

    fn save(&mut self) {
        match self.model.path.clone() {
            Some(p) => {
                self.status = match self.model.save(&p) {
                    Ok(()) => format!("Saved {}", p.display()),
                    Err(e) => e,
                };
            }
            None => self.save_as(),
        }
    }

    // ---------------------------------------------------------------- tabs

    fn tab_system(&mut self, ui: &mut egui::Ui) {
        style::page(ui, "System", "Name and localization of the installed system.", |ui| {
            style::card(ui, Some("Identity"), |ui| {
                self.field(ui, "Hostname", |m| &mut m.cfg.hostname);
                style::row(ui, "Timezone", |ui| {
                    if style::suggest(ui, "tz", &mut self.model.cfg.timezone, &self.lists.timezones, false) {
                        self.model.dirty = true;
                    }
                });
            });
            style::card(ui, Some("Language"), |ui| {
                style::row(ui, "Locale", |ui| {
                    if style::suggest(ui, "locale", &mut self.model.cfg.locale, &self.lists.locales, false) {
                        self.model.dirty = true;
                    }
                });
                style::row(ui, "Keymap", |ui| {
                    if style::suggest(ui, "keymap", &mut self.model.cfg.keymap, &self.lists.keymaps, false) {
                        self.model.dirty = true;
                    }
                });
            });
        });
    }

    fn tab_disk(&mut self, ui: &mut egui::Ui) {
        style::page(ui, "Target disk", "The installer erases the disk it selects. Name it by serial number, or let it take the largest one.", |ui| {
            style::card(ui, Some("Which disk"), |ui| {
                let mut auto = self.model.cfg.disk.auto_largest;
                if ui.checkbox(&mut auto, "Erase and install onto the largest disk, without asking").changed() {
                    if auto {
                        self.confirm = Some(Confirm::AutoLargest);
                    } else {
                        self.model.cfg.disk.auto_largest = false;
                        self.model.dirty = true;
                    }
                }
                if self.model.cfg.disk.auto_largest {
                    style::note(ui, WARN, "Every machine that boots this ISO loses its largest disk.");
                }
                ui.add_space(4.0);
                ui.add_enabled_ui(!self.model.cfg.disk.auto_largest, |ui| {
                    self.field(ui, "Serial", |m| &mut m.cfg.disk.confirm_serial);
                    let mut has = self.model.cfg.disk.model.is_some();
                    style::row(ui, "Model", |ui| {
                        if ui.checkbox(&mut has, "must also contain").changed() {
                            self.model.cfg.disk.model = has.then(String::new);
                            self.model.dirty = true;
                        }
                        if let Some(m) = &mut self.model.cfg.disk.model {
                            if style::text(ui, m).changed() {
                                self.model.dirty = true;
                            }
                        }
                    });
                });
            });
            style::card(ui, Some("Layout"), |ui| {
                style::row(ui, "ESP size", |ui| {
                    if ui.add(egui::DragValue::new(&mut self.model.cfg.disk.esp_mib).range(64..=8192).suffix(" MiB")).changed() {
                        self.model.dirty = true;
                    }
                });
                style::hint(ui, "The FAT32 boot partition, mounted at /boot. The rest of the disk is the root file system.");
            });
        });
    }

    fn tab_packages(&mut self, ui: &mut egui::Ui) {
        style::page(ui, "Mirrors & packages", "Where packages come from and which ones to install (official core and extra, plus pinned AUR packages).", |ui| {
            style::card(ui, Some("Mirrors"), |ui| {
                self.lines(ui, "mirrors", |m| m.cfg.mirrors.clone(), |m, v| m.cfg.mirrors = v, 80.0);
                style::hint(ui, "One URL per line; $repo and $arch are substituted. The first mirror is tried first.");
            });
            style::card(ui, Some("Packages"), |ui| {
                self.lines(ui, "packages", |m| m.cfg.packages.clone(), |m, v| m.cfg.packages = v, 230.0);
                style::hint(ui, "One package or group per line.");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let busy = self.resolve_rx.is_some();
                    if ui.add_enabled(!busy, egui::Button::new("Resolve dependencies")).clicked() {
                        let cfg = self.model.cfg.clone();
                        let mirror = cfg.mirrors.first().cloned().unwrap_or_default();
                        let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("archstaler-gui");
                        let (tx, rx) = channel();
                        self.resolve_rx = Some(rx);
                        std::thread::spawn(move || {
                            let r = hostcfg::resolve::fetch_dbs(&mirror, &cache).and_then(|dbs| hostcfg::resolve::resolve(&cfg, &dbs)).map_err(|e| e.to_string());
                            let _ = tx.send(r);
                        });
                    }
                    if busy {
                        ui.spinner();
                        ui.label(egui::RichText::new("fetching the package databases...").weak());
                    }
                });
            });
            self.aur_group(ui);
            match &self.resolve {
                Some(Err(e)) => {
                    style::card(ui, Some("Resolution"), |ui| style::note(ui, BAD, e.clone()));
                }
                Some(Ok(r)) => {
                    style::card(ui, Some("Resolution"), |ui| {
                        ui.horizontal(|ui| {
                            style::chip(ui, &format!("{} packages", r.packages.len()), ACCENT);
                            style::chip(ui, &format!("{} MiB to download", r.download_bytes() >> 20), ACCENT);
                        });
                        for a in &r.ambiguities {
                            style::note(ui, WARN, format!("{} is provided by {}; chosen {} (set it in `providers` to pick another)", a.dep, a.candidates.join(", "), a.chosen));
                        }
                        egui::ScrollArea::vertical().id_salt("resolved").max_height(200.0).show(ui, |ui| {
                            egui::Grid::new("resolved_grid").num_columns(4).spacing([18.0, 3.0]).striped(true).show(ui, |ui| {
                                for p in &r.packages {
                                    ui.label(if p.explicit { egui::RichText::new(&p.name).strong() } else { egui::RichText::new(&p.name) });
                                    ui.label(egui::RichText::new(&p.version).weak());
                                    ui.label(egui::RichText::new(&p.repo).weak());
                                    ui.label(egui::RichText::new(format!("{} KiB", p.csize >> 10)).weak());
                                    ui.end_row();
                                }
                            });
                        });
                        style::hint(ui, "Bold: named in the list. The rest are dependencies.");
                    });
                }
                None => {}
            }
        });
    }

    /// The AUR packages group of the Mirrors & packages tab (docs/aur.md).
    fn aur_group(&mut self, ui: &mut egui::Ui) {
        style::card(ui, Some("AUR packages"), |ui| {
            let used = !self.model.cfg.aur.is_empty();
            if !used && !self.aur.trust_ack {
                style::hint(ui, "Packages from the Arch User Repository are not part of the official repositories.");
                if ui.button("Use AUR packages...").clicked() {
                    self.aur.trust_ack = false;
                    self.aur.message = None;
                    self.confirm = Some(Confirm::Aur);
                }
                return;
            }
            style::note(ui, WARN, "AUR recipes are not reviewed by Arch. The installed system builds each one at its first boot, as an unprivileged user, from the exact recipe you review here; the build still runs code from that recipe on that machine, and the result is not signed by Arch.");
            if !aur::has_network_service(&self.model.cfg.services) {
                style::note(ui, BAD, "The build needs a network on the installed system: enable NetworkManager.service (or another network service).");
                if ui.button("Add NetworkManager").clicked() {
                    if !self.model.cfg.packages.iter().any(|p| p == "networkmanager") {
                        self.model.cfg.packages.push("networkmanager".into());
                    }
                    self.model.cfg.services.push("NetworkManager.service".into());
                    self.bufs.remove("packages");
                    self.bufs.remove("services");
                    self.model.dirty = true;
                }
            }
            // Pinned packages.
            let mut remove = None;
            for (i, a) in self.model.cfg.aur.iter_mut().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(&a.name).strong());
                    ui.label(egui::RichText::new(format!("@ {}", &a.commit[..a.commit.len().min(8)])).weak());
                    if a.as_dep {
                        style::chip(ui, "dependency", ACCENT);
                    }
                    if a.vcs {
                        style::chip(ui, "VCS, not pinned", WARN);
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
                let mut units = a.services.join(", ");
                style::row(ui, "Enable units", |ui| {
                    if style::suggest(ui, &format!("aur_units_{i}"), &mut units, &self.lists.services, true) {
                        a.services = units.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                        self.model.dirty = true;
                    }
                });
            }
            if let Some(i) = remove {
                let gone = self.model.cfg.aur.remove(i);
                self.model.dirty = true;
                self.aur.message = Some(Ok(format!("Removed {}. Its AUR dependencies stay pinned; remove them too if nothing else needs them.", gone.name)));
            }
            ui.add_space(4.0);
            // Search.
            style::row(ui, "Search the AUR", |ui| {
                let w = ui.available_width() - 90.0;
                let r = ui.add(egui::TextEdit::singleline(&mut self.aur.search).desired_width(w).margin(egui::Margin::symmetric(8, 5)));
                let go = ui.add_enabled(!self.aur.busy() && self.aur.search.trim().len() >= 2, egui::Button::new("Search")).clicked();
                if go || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.aur.busy() && self.aur.search.trim().len() >= 2) {
                    self.aur.start_search();
                }
            });
            if self.aur.busy() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    let what = if self.aur.pin_rx.is_some() { "pinning (re-checking pins, resolving dependencies)..." } else if let Some(n) = &self.aur.fetching { &format!("fetching the recipe of {n}...") } else { "searching..." };
                    ui.label(egui::RichText::new(what).weak());
                });
            }
            match self.aur.results.clone() {
                Some(Err(e)) => style::note(ui, BAD, e),
                Some(Ok(list)) if list.is_empty() => style::hint(ui, "Nothing found."),
                Some(Ok(list)) => {
                    for info in list {
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(!self.aur.busy(), egui::Button::new("Review")).clicked() {
                                self.aur.start_review(&info);
                            }
                            ui.label(egui::RichText::new(&info.name).strong());
                            ui.label(egui::RichText::new(&info.version).weak());
                            ui.label(egui::RichText::new(format!("{} votes", info.votes)).weak());
                            match &info.maintainer {
                                Some(m) => ui.label(egui::RichText::new(format!("by {m}")).weak()),
                                None => ui.label(egui::RichText::new("orphaned").color(WARN)),
                            };
                            if info.out_of_date.is_some() {
                                style::chip(ui, "out of date", WARN);
                            }
                        });
                        if let Some(d) = &info.description {
                            style::hint(ui, &format!("    {d}"));
                        }
                    }
                }
                None => {}
            }
            self.aur_review(ui);
            match &self.aur.message {
                Some(Ok(m)) => style::note(ui, OK, m.clone()),
                Some(Err(e)) => style::note(ui, BAD, e.clone()),
                None => {}
            }
        });
    }

    /// The recipe viewer: what was fetched, what looks risky, and the acknowledgements before the pin.
    fn aur_review(&mut self, ui: &mut egui::Ui) {
        let Some(rev) = self.aur.review.clone() else { return };
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new(format!("Review: {} {} at commit {}", rev.name, rev.srcinfo.version(), &rev.commit[..12])).strong());
        style::hint(ui, &format!("pkgbase {}   tree digest {}", rev.pkgbase, &rev.tree_sha256[..16]));
        for w in &rev.warnings {
            style::note(ui, WARN, w.clone());
        }
        let deps = rev.srcinfo.depends(&rev.name);
        if !deps.is_empty() {
            style::hint(ui, &format!("depends: {}", deps.join(", ")));
        }
        if !rev.srcinfo.makedepends().is_empty() {
            style::hint(ui, &format!("makedepends: {}", rev.srcinfo.makedepends().join(", ")));
        }
        let risky = aurbuild::plan::risky_lines(&rev.files);
        ui.horizontal_wrapped(|ui| {
            for f in rev.files.keys() {
                if ui.selectable_label(self.aur.file == *f, f).clicked() {
                    self.aur.file = f.clone();
                }
            }
        });
        if let Some(data) = rev.files.get(&self.aur.file) {
            let text = String::from_utf8_lossy(data).into_owned();
            egui::ScrollArea::both().id_salt("aur_file").min_scrolled_height(260.0).max_height(300.0).auto_shrink([false, false]).show(ui, |ui| {
                for (n, line) in text.lines().enumerate() {
                    let bad = risky.iter().any(|(f, l, _)| *f == self.aur.file && *l == n + 1);
                    let t = egui::RichText::new(format!("{:>4}  {line}", n + 1)).monospace();
                    ui.label(if bad { t.color(WARN) } else { t });
                }
            });
        }
        ui.checkbox(&mut self.aur.ack_reviewed, "I reviewed this recipe at this commit and accept that it will be built and installed");
        if rev.vcs {
            ui.checkbox(&mut self.aur.ack_vcs, "I understand its VCS sources are not pinned by the commit: the build fetches whatever they point to then");
        }
        ui.horizontal(|ui| {
            let ok = self.aur.ack_reviewed && (!rev.vcs || self.aur.ack_vcs) && !self.aur.busy();
            if ui.add_enabled(ok, style::primary("Pin and add")).clicked() {
                let mirror = self.model.cfg.mirrors.first().cloned().unwrap_or_default();
                let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("archstaler-gui");
                self.aur.start_pin(rev.clone(), self.model.cfg.aur.clone(), mirror, cache);
            }
            if ui.button("Close").clicked() {
                self.aur.review = None;
            }
        });
    }

    fn tab_users(&mut self, ui: &mut egui::Ui) {
        style::page(ui, "Users", "Passwords are stored as SHA-512 crypt hashes only; the plaintext is never saved.", |ui| {
            style::card(ui, Some("Accounts"), |ui| {
                if self.model.cfg.users.is_empty() {
                    style::hint(ui, "No users yet: the system would have only a locked root account.");
                }
                let mut remove = None;
                egui::Grid::new("users_grid").num_columns(4).spacing([24.0, 6.0]).show(ui, |ui| {
                    for (i, u) in self.model.cfg.users.iter().enumerate() {
                        ui.label(egui::RichText::new(&u.name).strong());
                        ui.label(egui::RichText::new(format!("groups: {}", u.groups.join(", "))).weak());
                        ui.label(egui::RichText::new(&u.shell).weak());
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
                if let Some(i) = remove {
                    self.model.cfg.users.remove(i);
                    self.model.dirty = true;
                }
            });
            style::card(ui, Some("Add a user"), |ui| {
                let n = &mut self.new_user;
                style::row(ui, "Name", |ui| {
                    style::text(ui, &mut n.name);
                });
                style::row(ui, "Password", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut n.pw).password(true).desired_width(ui.available_width()).margin(egui::Margin::symmetric(8, 5)));
                });
                style::row(ui, "Again", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut n.pw2).password(true).desired_width(ui.available_width()).margin(egui::Margin::symmetric(8, 5)));
                });
                style::row(ui, "Groups", |ui| {
                    style::suggest(ui, "groups", &mut n.groups, &self.lists.groups, true);
                });
                style::row(ui, "Shell", |ui| {
                    style::suggest(ui, "shell", &mut n.shell, &self.lists.shells, false);
                });
                let ok = !n.name.is_empty() && !n.pw.is_empty() && n.pw == n.pw2;
                if !n.pw.is_empty() && n.pw != n.pw2 {
                    style::note(ui, BAD, "The passwords differ");
                }
                ui.add_space(2.0);
                if ui.add_enabled(ok, style::primary("Add user")).clicked() {
                    match hostcfg::password::hash(&n.pw) {
                        Ok(h) => {
                            let groups = n.groups.split(',').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
                            self.model.cfg.users.push(config::User { name: n.name.clone(), password_hash: h, groups, shell: n.shell.clone() });
                            self.model.dirty = true;
                            self.new_user = NewUser::default();
                        }
                        Err(e) => self.status = format!("cannot hash the password: {e}"),
                    }
                }
            });
            style::card(ui, Some("root account"), |ui| {
                let mut locked = self.model.cfg.root_password_hash.is_none();
                if ui.checkbox(&mut locked, "Leave root locked (use sudo)").changed() {
                    self.model.cfg.root_password_hash = if locked { None } else { Some(String::new()) };
                    self.model.dirty = true;
                }
                if let Some(h) = &mut self.model.cfg.root_password_hash {
                    style::row(ui, "Password hash", |ui| {
                        if style::text(ui, h).changed() {
                            self.model.dirty = true;
                        }
                    });
                    style::hint(ui, "A $6$ SHA-512 crypt hash, for example from `openssl passwd -6`.");
                }
            });
            style::card(ui, Some("Home directory config (zip)"), |ui| {
                style::note(ui, WARN, "Only for the Hyprland preset. The zip is laid out like a Hyprland setup (.config/hypr/hyprland.lua, ...) and is unpacked into every user's home. Do not use it for other desktops.");
                style::note(ui, WARN, "A Hyprland config can run any command when the session starts. Only use a server you trust: the archive is checked by HTTPS alone.");
                let mut url = self.model.cfg.user_archives.first().map(|a| a.url.clone()).unwrap_or_default();
                style::row(ui, "Zip URL", |ui| {
                    if style::text(ui, &mut url).changed() {
                        self.model.cfg.user_archives = if url.trim().is_empty() { vec![] } else { vec![config::UserArchive { url: url.trim().to_string() }] };
                        self.model.dirty = true;
                    }
                });
                style::hint(ui, "https:// only, at most 16 MiB. Leave empty to skip. If the download fails at install time it is skipped with a warning.");
            });
        });
    }

    /// A suggestion field that appends its value as a new line of the list editor `key` below it.
    fn add_line(&mut self, ui: &mut egui::Ui, key: &'static str, label: &str, options: Vec<String>, get: impl Fn(&Model) -> Vec<String>, set: impl Fn(&mut Model, Vec<String>)) {
        let slot = if key == "services" { "services_add" } else { "kernel_params_add" };
        let mut v = self.bufs.remove(slot).unwrap_or_default();
        style::row(ui, label, |ui| {
            style::suggest(ui, slot, &mut v, &options, false);
        });
        if ui.add_enabled(!v.trim().is_empty(), egui::Button::new("Add to the list")).clicked() {
            let mut list = get(&self.model);
            if !list.iter().any(|x| x == v.trim()) {
                list.push(v.trim().to_string());
            }
            set(&mut self.model, list);
            self.bufs.remove(key); // the list editor rebuilds its text from the model
            self.model.dirty = true;
            v.clear();
        }
        self.bufs.insert(slot, v);
        ui.add_space(4.0);
    }

    fn tab_services(&mut self, ui: &mut egui::Ui) {
        style::page(ui, "Services & kernel", "What starts on the first boot and how the kernel is started.", |ui| {
            style::card(ui, Some("Services enabled on first boot"), |ui| {
                let opts = self.lists.services.clone();
                self.add_line(ui, "services", "Unit", opts, |m| m.cfg.services.clone(), |m, v| m.cfg.services = v);
                self.lines(ui, "services", |m| m.cfg.services.clone(), |m, v| m.cfg.services = v, 130.0);
                style::hint(ui, "One systemd unit per line, for example sshd.service.");
            });
            style::card(ui, Some("Extra kernel parameters"), |ui| {
                let opts = self.lists.kernel_params.clone();
                self.add_line(ui, "kernel_params", "Parameter", opts, |m| m.cfg.kernel_params.clone(), |m, v| m.cfg.kernel_params = v);
                self.lines(ui, "kernel_params", |m| m.cfg.kernel_params.clone(), |m, v| m.cfg.kernel_params = v, 110.0);
                style::hint(ui, "One word per line, appended to the installed system's kernel command line.");
            });
        });
    }

    fn tab_build(&mut self, ui: &mut egui::Ui) {
        style::page(ui, "Build & drivers", "How the installer ISO itself is built. These settings are not part of the installed system.", |ui| {
            style::card(ui, Some("Build profile"), |ui| {
                let current = self.model.host.build.profile.clone().unwrap_or_else(|| "super-small".into());
                let mut chosen = current.clone();
                ui.radio_value(&mut chosen, "super-small".into(), "super-small");
                style::hint(ui, "Size-optimized installer (the default), about 0.7 MiB. A crash shows no panic message.");
                ui.add_space(2.0);
                ui.radio_value(&mut chosen, "large".into(), "large");
                style::hint(ui, "The regular release build, about 0.8 MiB, with panic messages kept for debugging.");
                ui.add_space(2.0);
                ui.add_enabled(false, egui::RadioButton::new(false, "extra-large"));
                style::hint(ui, "Reserved, not available yet.");
                if chosen != current {
                    self.model.set_profile(&chosen);
                }
            });
            style::card(ui, Some("USB tethering"), |ui| {
                let mut teth = self.model.host.build.tethering;
                if ui.checkbox(&mut teth, "Include USB tethering (iPhone, Android, USB Ethernet)").changed() {
                    self.model.host.build.tethering = teth;
                    self.model.dirty = true;
                }
                style::hint(ui, "Adds about 100 KiB. Lets the installer use a phone's hotspot when no wired network has a link.");
            });
            style::card(ui, Some("Installer drivers"), |ui| {
                style::hint(ui, "Drivers of the installer itself, not packages for the installed system. Leave them all on unless you know the hardware.");
                ui.add_space(2.0);
                let mut all = self.model.host.installer_drivers.is_none();
                if ui.checkbox(&mut all, "All drivers").changed() {
                    self.model.host.installer_drivers = if all { None } else { Some(DRIVERS.iter().map(|d| d.id.to_string()).collect()) };
                    self.model.dirty = true;
                }
                if let Some(list) = self.model.host.installer_drivers.clone() {
                    let mut next = list.clone();
                    for (class, title) in [(DriverClass::Storage, "Storage"), (DriverClass::Network, "Network")] {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(title).strong());
                        egui::Grid::new(title).num_columns(3).spacing([16.0, 4.0]).show(ui, |ui| {
                            for d in DRIVERS.iter().filter(|d| d.class == class) {
                                let mut on = list.iter().any(|i| i == d.id);
                                if ui.checkbox(&mut on, d.id).changed() {
                                    if on {
                                        next.push(d.id.to_string());
                                    } else {
                                        next.retain(|i| i != d.id);
                                    }
                                }
                                ui.label(d.description);
                                ui.label(egui::RichText::new(d.status).small().weak());
                                ui.end_row();
                            }
                        });
                    }
                    if next != list {
                        self.model.host.installer_drivers = Some(next);
                        self.model.dirty = true;
                    }
                }
            });
            style::card(ui, Some("AUR packages"), |ui| style::hint(ui, "Not available yet."));
        });
    }

    fn tab_scripts(&mut self, ui: &mut egui::Ui) {
        style::page(ui, "First-boot scripts", "Run as root on the installed system, in this order, at the end of the first boot.", |ui| {
            style::note(ui, WARN, "Scripts are code. A failing script only logs a warning.");
            ui.add_space(4.0);
            style::card(ui, Some("Scripts in this config"), |ui| {
                if self.model.cfg.scripts.is_empty() {
                    style::hint(ui, "None yet.");
                }
                let mut remove = None;
                egui::Grid::new("scripts_grid").num_columns(4).spacing([20.0, 6.0]).show(ui, |ui| {
                    for (i, s) in self.model.cfg.scripts.iter().enumerate() {
                        let src = match hostcfg::scripts::source(s) {
                            hostcfg::scripts::Source::Builtin => "built-in".to_string(),
                            hostcfg::scripts::Source::File(f) => format!("file {f}"),
                            hostcfg::scripts::Source::Remote { url, .. } => format!("{url} (pinned)"),
                        };
                        ui.label(egui::RichText::new(format!("{}", i + 1)).weak());
                        ui.label(egui::RichText::new(&s.id).strong());
                        ui.label(egui::RichText::new(format!("{src}  {}", s.args.join(" "))).weak());
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
                if let Some(i) = remove {
                    self.model.cfg.scripts.remove(i);
                    self.model.dirty = true;
                }
            });
            style::card(ui, Some("Built-in scripts"), |ui| {
                let last = hostcfg::scripts::BUILTINS.len().saturating_sub(1);
                for (n, b) in hostcfg::scripts::BUILTINS.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(b.id).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Add").clicked() && !self.model.cfg.scripts.iter().any(|s| s.id == b.id) {
                                self.model.cfg.scripts.push(config::Script { id: b.id.into(), ..Default::default() });
                                self.model.dirty = true;
                            }
                        });
                    });
                    ui.label(b.description);
                    style::hint(ui, &format!("runs as {}, {}; needs: {}", b.runs_as, b.phase, if b.requires.is_empty() { "nothing" } else { b.requires }));
                    if n != last {
                        ui.separator();
                    }
                }
            });
            style::card(ui, Some("Custom script"), |ui| {
                style::hint(ui, "A local file, or an https URL pinned by its SHA-256 digest.");
                let n = &mut self.new_script;
                style::row(ui, "Id", |ui| {
                    style::text(ui, &mut n.0);
                });
                style::row(ui, "Local file", |ui| {
                    let w = ui.available_width() - 110.0;
                    ui.add(egui::TextEdit::singleline(&mut n.1).desired_width(w).margin(egui::Margin::symmetric(8, 5)));
                    if ui.button("Browse...").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                            n.1 = p.display().to_string();
                        }
                    }
                });
                style::row(ui, "URL", |ui| {
                    style::text(ui, &mut n.2);
                });
                style::row(ui, "SHA-256", |ui| {
                    style::text(ui, &mut n.3);
                });
                if !n.1.is_empty() {
                    if let Ok(text) = std::fs::read_to_string(&n.1) {
                        let digest = {
                            use sha2::Digest;
                            sha2::Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()
                        };
                        style::hint(ui, &format!("sha256 {digest}"));
                        egui::ScrollArea::vertical().id_salt("scriptsrc").max_height(120.0).show(ui, |ui| ui.monospace(&text));
                    }
                }
                ui.checkbox(&mut self.root_script_ack, "I understand a custom script runs as root on the installed system");
                let valid = !n.0.is_empty() && (!n.1.is_empty() || !n.2.is_empty());
                if ui.add_enabled(valid && self.root_script_ack, style::primary("Add custom script")).clicked() {
                    let s = config::Script {
                        id: n.0.clone(),
                        file: (!n.1.is_empty()).then(|| n.1.clone()),
                        url: (n.1.is_empty() && !n.2.is_empty()).then(|| n.2.clone()),
                        sha256: (n.1.is_empty() && !n.3.is_empty()).then(|| n.3.clone()),
                        ..Default::default()
                    };
                    self.model.cfg.scripts.push(s);
                    self.model.dirty = true;
                    self.new_script = Default::default();
                }
            });
        });
    }

    fn tab_preview(&mut self, ui: &mut egui::Ui) {
        let raw = self.model.raw.is_some();
        let sub = if raw { "This file was not written by the GUI, so it is edited as text and checked as it is." } else { "The file this config is saved as. It is plain Lua; you can also edit it by hand." };
        style::page(ui, "Lua preview", sub, |ui| {
            style::card(ui, None, |ui| match &mut self.model.raw {
                Some(text) => {
                    if ui.add(egui::TextEdit::multiline(text).code_editor().desired_width(f32::INFINITY).desired_rows(28)).changed() {
                        self.model.dirty = true;
                    }
                    ui.add_space(4.0);
                    if ui.button("Replace with a form-based starter").clicked() {
                        let p = self.model.path.clone();
                        let mut m = Model::starter();
                        m.path = p;
                        m.dirty = true;
                        self.load(m);
                    }
                }
                None => {
                    let mut text = self.model.lua();
                    egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| ui.add(egui::TextEdit::multiline(&mut text).code_editor().desired_width(f32::INFINITY).interactive(false)));
                }
            });
        });
    }

    fn tab_iso(&mut self, ui: &mut egui::Ui) {
        style::page(ui, "Build ISO", "Build the installer image from this config, then put it on a stick.", |ui| {
            let problems = self.model.problems();
            style::card(ui, Some("Summary"), |ui| {
                style::row(ui, "Config", |ui| match &self.model.path {
                    Some(p) => {
                        ui.label(p.display().to_string());
                    }
                    None => {
                        ui.label(egui::RichText::new("not saved yet; a build uses the saved file").weak());
                    }
                });
                style::row(ui, "Profile", |ui| {
                    style::chip(ui, &self.model.effective_profile(), ACCENT);
                    if self.model.host.build.tethering {
                        style::chip(ui, "USB tethering", ACCENT);
                    }
                });
                style::row(ui, "Drivers", |ui| {
                    ui.label(self.model.host.drivers().join(", "));
                });
                for l in hostcfg::scripts::summary(&self.model.cfg) {
                    style::row(ui, "Script", |ui| {
                        ui.label(l);
                    });
                }
                if self.model.cfg.disk.auto_largest {
                    style::note(ui, WARN, "This ISO erases the largest disk of the machine it boots on, without asking.");
                }
                if let Some(p) = problems.first() {
                    style::note(ui, BAD, format!("The config is invalid: {}", p.message));
                }
            });

            style::card(ui, Some("Build"), |ui| {
                let can = problems.is_empty() && self.build.running.is_none();
                let vv: Vec<usize> = self.media.volumes.iter().enumerate().filter(|(_, v)| media::is_ventoy(v, &self.media.volumes)).map(|(i, _)| i).collect();
                let stick_idx = self.media.stick.filter(|i| vv.contains(i)).or(vv.first().copied());
                let to_stick = !self.media.to_file && stick_idx.is_some();
                let can = can && (!to_stick || self.media.volumes[stick_idx.unwrap()].mounted);
                if vv.is_empty() {
                    style::hint(ui, "No Ventoy drive found: the ISO is saved as a file. Plug one in and press Refresh.");
                } else {
                    if ui.radio(to_stick, "Build straight onto the Ventoy drive (nothing is kept on this computer)").clicked() {
                        self.media.to_file = false;
                    }
                    if to_stick {
                        for &i in &vv {
                            let v = self.media.volumes[i].clone();
                            ui.horizontal(|ui| {
                                let place = if v.mounted { v.mount.display().to_string() } else { "not mounted".to_string() };
                                let label = format!("    {}  {}  {}", if v.label.is_empty() { &v.name } else { &v.label }, place, if v.mounted { format!("{} GiB free", v.available >> 30) } else { String::new() });
                                if ui.radio(stick_idx == Some(i), label).clicked() {
                                    self.media.stick = Some(i);
                                }
                                if !v.mounted && ui.add_enabled(self.media.drive_busy.is_none(), egui::Button::new("Mount")).clicked() {
                                    self.run_drive(v.name.clone(), true);
                                }
                            });
                        }
                    }
                    if ui.radio(!to_stick, "Save the ISO as a file instead").clicked() {
                        self.media.to_file = true;
                    }
                }
                if ui.small_button("Refresh drives").clicked() {
                    self.media.volumes = media::volumes();
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let label = if to_stick { "Build onto Ventoy" } else { "Save and build..." };
                    if ui.add_enabled(can, style::primary(label)).clicked() {
                        if to_stick {
                            // Straight to the stick: an unsaved config is built from a temporary copy, no dialog.
                            let name = self.model.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "archstaler".into());
                            let cfg = match self.model.path.clone() {
                                Some(p) => {
                                    self.save();
                                    Ok(p)
                                }
                                None => {
                                    let p = std::env::temp_dir().join(format!("archstaler-{}.lua", std::process::id()));
                                    std::fs::write(&p, self.model.lua()).map(|_| p).map_err(|e| format!("cannot write the temporary config: {e}"))
                                }
                            };
                            match cfg {
                                Ok(cfg) => {
                                    let v = self.media.volumes[stick_idx.unwrap()].clone();
                                    let out = std::env::temp_dir().join(format!("{name}.iso"));
                                    self.start_build(cfg, out);
                                    self.build.stick = Some((v, format!("{name}.iso")));
                                }
                                Err(e) => self.build.error = Some(e),
                            }
                        } else {
                            self.save();
                            if let Some(cfg) = self.model.path.clone() {
                                let name = cfg.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "archstaler".into());
                                if let Some(out) = rfd::FileDialog::new().add_filter("ISO image", &["iso"]).set_file_name(format!("{name}.iso")).save_file() {
                                    if out.exists() {
                                        self.confirm = Some(Confirm::Overwrite(out));
                                    } else {
                                        self.start_build(cfg, out);
                                    }
                                }
                            }
                        }
                    }
                    if self.build.running.is_some() {
                        if ui.button("Cancel").clicked() {
                            if let Some(b) = self.build.running.take() {
                                b.cancel();
                                self.build.error = Some(format!("Cancelled. Log kept at {}", b.log_path.display()));
                                self.build.iso = None;
                            }
                        }
                        ui.spinner();
                    }
                });
                if !self.build.stages.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for (name, st) in &self.build.stages {
                            let (mark, color) = match st {
                                State::Start => ("...", WARN),
                                State::Ok => ("ok", OK),
                                State::Fail => ("failed", BAD),
                            };
                            ui.label(egui::RichText::new(format!("[{name} {mark}]")).small().color(color));
                        }
                    });
                }
                if let Some(e) = &self.build.error {
                    style::note(ui, BAD, e.clone());
                }
                if let Some((path, size, sha)) = &self.build.iso {
                    style::note(ui, OK, format!("Built {} ({} KiB)", path.display(), size >> 10));
                    style::hint(ui, &format!("sha256 {sha}"));
                    if let Some(s) = &self.build.summary {
                        style::hint(ui, s);
                    }
                }
                ui.add_space(4.0);
                ui.collapsing("Build log", |ui| {
                    egui::ScrollArea::vertical().id_salt("log").stick_to_bottom(true).max_height(200.0).show(ui, |ui| {
                        for l in &self.build.log {
                            ui.monospace(l);
                        }
                    });
                });
            });
            self.media_section(ui);
        });
    }

    fn start_build(&mut self, cfg: PathBuf, out: PathBuf) {
        let root = match build::find_root() {
            Ok(r) => r,
            Err(e) => {
                self.build.error = Some(e);
                return;
            }
        };
        self.next_build += 1;
        let id = format!("{}-{}", std::process::id(), self.next_build);
        self.build = BuildState::default();
        match Build::start(&root, &cfg, &id) {
            Ok(b) => {
                self.build.out = Some(out);
                self.build.summary = Some(format!("config {}, profile {}", cfg.display(), self.model.effective_profile()));
                self.build.running = Some(b);
            }
            Err(e) => self.build.error = Some(e),
        }
    }

    fn pump_build(&mut self) {
        let Some(b) = &self.build.running else { return };
        let mut exit = None;
        while let Ok(m) = b.rx.try_recv() {
            match m {
                Msg::Log(l) => {
                    if self.build.log.len() < 20_000 {
                        self.build.log.push(l);
                    }
                }
                Msg::Event(Event::Stage { name, state }) => {
                    match self.build.stages.iter_mut().find(|(n, _)| *n == name) {
                        Some(s) => s.1 = state,
                        None => self.build.stages.push((name, state)),
                    }
                }
                Msg::Event(Event::Done { path, size, sha256, .. }) => self.build.done = Some((path, size, sha256)),
                Msg::Event(Event::Failed { message }) => self.build.error = Some(message),
                Msg::Exit(ok) => exit = Some(ok),
            }
        }
        if let Some(ok) = exit {
            let b = self.build.running.take().unwrap();
            let _ = std::fs::write(&b.log_path, self.build.log.join("\n"));
            match (ok, self.build.done.take()) {
                (true, Some((path, size, sha))) if self.build.stick.is_some() && build::usable(std::path::Path::new(&path), size) => {
                    let (v, name) = self.build.stick.clone().unwrap();
                    self.media.cleanup = Some(b.workdir.clone());
                    self.build.iso = Some((PathBuf::from(&path), size, sha));
                    self.media.result = None;
                    let dir = v.mount.clone();
                    self.begin_copy(PathBuf::from(path), v, dir, name);
                }
                (true, Some((path, size, sha))) if build::usable(std::path::Path::new(&path), size) => {
                    let out = self.build.out.clone().unwrap();
                    let moved = std::fs::rename(&path, &out).or_else(|_| std::fs::copy(&path, &out).map(|_| ()));
                    match moved {
                        Ok(()) if build::usable(&out, size) => {
                            let _ = std::fs::remove_dir_all(&b.workdir);
                            self.build.iso = Some((out, size, sha));
                        }
                        Ok(()) => self.build.error = Some("the ISO was moved but is not readable at the expected size".into()),
                        Err(e) => self.build.error = Some(format!("cannot place the ISO at its destination: {e}")),
                    }
                }
                _ => {
                    if self.build.error.is_none() {
                        self.build.error = Some(format!("The build failed; see the log (kept at {})", b.log_path.display()));
                    }
                }
            }
        }
    }

    fn media_section(&mut self, ui: &mut egui::Ui) {
        let iso = self.media.iso_override.clone().or_else(|| self.build.iso.as_ref().map(|i| i.0.clone()));
        style::card(ui, Some("Put it on a stick"), |ui| {
            style::row(ui, "ISO", |ui| {
                if ui.button("Pick an ISO...").clicked() {
                    self.media.iso_override = rfd::FileDialog::new().add_filter("ISO image", &["iso"]).pick_file();
                }
                match &iso {
                    Some(p) => ui.label(p.display().to_string()),
                    None => ui.label(egui::RichText::new("build one first, or pick one").weak()),
                };
            });
            style::hint(ui, "Copy to Ventoy adds the ISO as a file; nothing on the stick is formatted or repartitioned. Raw flashing is not available yet.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Volumes").strong());
                if ui.small_button("Refresh").clicked() {
                    self.media.volumes = media::volumes();
                    self.media.selected = None;
                }
            });
            let all = self.media.volumes.clone();
            ui.checkbox(&mut self.media.show_internal, "Show internal disks");
            for (i, v) in all.iter().enumerate() {
                if !v.removable && !self.media.show_internal {
                    continue;
                }
                let ventoy = media::is_ventoy(v, &all);
                ui.horizontal_wrapped(|ui| {
                    let name = if v.label.is_empty() { v.name.clone() } else { v.label.clone() };
                    let text = if v.mounted { format!("{name}  {}", v.mount.display()) } else { format!("{name}  (not mounted)") };
                    if ui.add_enabled(v.mounted, egui::RadioButton::new(self.media.selected == Some(i), text)).clicked() {
                        self.media.selected = Some(i);
                        self.media.dir = Some(v.mount.clone());
                    }
                    if ventoy {
                        style::chip(ui, "Ventoy", OK);
                    }
                    let idle = self.media.drive_busy.is_none() && self.media.rx.is_none();
                    if !v.mounted {
                        if ui.add_enabled(idle, egui::Button::new("Mount")).clicked() {
                            self.run_drive(v.name.clone(), true);
                        }
                    } else if v.removable && ui.add_enabled(idle, egui::Button::new("Sync & unmount")).clicked() {
                        self.run_drive(v.name.clone(), false);
                    }
                });
                style::hint(ui, &format!("    {} {}  {}", v.name, v.fs, if v.mounted { format!("{} GiB free of {}", v.available >> 30, v.total >> 30) } else { format!("{} GiB", v.total >> 30) }));
            }
            if let Some(d) = &self.media.drive_busy {
                style::hint(ui, &format!("Working on {d} ..."));
            }
            match &self.media.drive_msg {
                Some(Ok(m)) => style::note(ui, OK, m.clone()),
                Some(Err(e)) => style::note(ui, BAD, e.clone()),
                None => {}
            }
            if let Some(v) = self.media.selected.and_then(|i| all.get(i)) {
                ui.add_space(4.0);
                if !media::is_ventoy(v, &all) {
                    style::note(ui, WARN, "Ventoy was not detected here (no ventoy folder or ventoy.json). The ISO is only copied into the folder you choose.");
                }
                style::row(ui, "Folder", |ui| {
                    if ui.button("Choose...").clicked() {
                        if let Some(d) = rfd::FileDialog::new().set_directory(&v.mount).pick_folder() {
                            self.media.dir = Some(d);
                        }
                    }
                    ui.label(self.media.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default());
                });
            }
            ui.add_space(4.0);
            let busy = self.media.rx.is_some();
            let ready = iso.is_some() && self.media.selected.is_some() && !busy;
            ui.horizontal(|ui| {
                if ui.add_enabled(ready, style::primary("Copy to the selected volume")).clicked() {
                    let (iso, v) = (iso.clone().unwrap(), all[self.media.selected.unwrap()].clone());
                    let dir = self.media.dir.clone().unwrap_or(v.mount.clone());
                    let name = iso.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.begin_copy(iso, v, dir, name);
                }
                if busy && ui.button("Cancel").clicked() {
                    self.media.cancel.store(true, Ordering::Relaxed);
                }
            });
            if let Some((phase, done, total)) = self.media.progress {
                if busy {
                    let f = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                    ui.add(egui::ProgressBar::new(f).desired_height(18.0).fill(ACCENT).text(match phase {
                        Phase::Copying => "copying",
                        Phase::Verifying => "verifying",
                    }));
                }
            }
            match &self.media.result {
                Some(Ok(m)) => style::note(ui, OK, m.clone()),
                Some(Err(e)) => style::note(ui, BAD, format!("Not copied: {e}")),
                None => {}
            }
        });
    }

    /// Copies `iso` to `dir` on `v` as `name`; asks first when earlier builds are already there.
    fn begin_copy(&mut self, iso: PathBuf, v: Volume, dir: PathBuf, name: String) {
        let files = media::older_isos(&dir, &name);
        if files.is_empty() {
            self.start_copy(iso, v, dir, vec![], Some(name));
        } else {
            let new_name = media::free_name(&dir, &name);
            self.older = Some(OlderPrompt { files, iso, volume: v, dir, name, new_name });
        }
    }

    fn start_copy(&mut self, iso: PathBuf, v: Volume, dir: PathBuf, delete: Vec<PathBuf>, rename_to: Option<String>) {
        let target = FolderCopy { dir, available: Some(v.available), overwrite: false, delete, trash_root: Some(v.mount.clone()), rename_to };
        let (tx, rx) = channel();
        self.media.rx = Some(rx);
        self.media.result = None;
        self.media.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.media.cancel.clone();
        std::thread::spawn(move || {
            let t2 = tx.clone();
            let r = target.write(&iso, &mut |p, d, t| {
                let _ = t2.send(MediaMsg::Progress(p, d, t));
            }, &cancel);
            let _ = tx.send(MediaMsg::Done(r));
        });
    }

    /// Mounts or unmounts (after a sync) a device in the background; the answer is shown under the list.
    fn run_drive(&mut self, dev: String, mount: bool) {
        let (tx, rx) = channel();
        self.media.drive_rx = Some(rx);
        self.media.drive_busy = Some(dev.clone());
        self.media.drive_msg = None;
        std::thread::spawn(move || {
            let r = if mount {
                media::mount(&dev).map(|p| format!("Mounted {dev} at {}", p.display()))
            } else {
                media::sync_and_unmount(&dev).map(|_| format!("{dev} is synced and unmounted: it is safe to remove"))
            };
            let _ = tx.send(r);
        });
    }

    fn pump_media(&mut self) {
        if let Some(rx) = &self.media.drive_rx {
            if let Ok(r) = rx.try_recv() {
                self.media.drive_rx = None;
                self.media.drive_busy = None;
                self.media.drive_msg = Some(r);
                self.media.volumes = media::volumes();
                self.media.selected = None;
            }
        }
        let mut finished = false;
        if let Some(rx) = &self.media.rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    MediaMsg::Progress(p, d, t) => self.media.progress = Some((p, d, t)),
                    MediaMsg::Done(r) => {
                        if r.is_ok() {
                            if let Some(d) = self.media.cleanup.take() {
                                let _ = std::fs::remove_dir_all(d);
                                self.build.iso = None;
                            }
                        }
                        self.media.result = Some(r.map(|r| format!("Copied and verified {} ({} KiB, sha256 {})", r.dest.display(), r.size >> 10, r.sha256)));
                        finished = true;
                    }
                }
            }
        }
        if finished {
            self.media.rx = None;
            self.media.progress = None;
            self.media.volumes = media::volumes();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.resolve_rx {
            if let Ok(r) = rx.try_recv() {
                self.resolve = Some(r);
                self.resolve_rx = None;
            }
        }
        if let Some(p) = self.aur.poll() {
            self.model.cfg.aur = p.entries;
            self.model.dirty = true;
        }
        if self.aur.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.pump_build();
        self.pump_media();
        // Pick up sticks plugged in or removed while the Build ISO tab is open.
        if self.tab == Tab::Iso && self.media.rx.is_none() && self.media.drive_rx.is_none() {
            if self.media.last_scan.is_none_or(|t| t.elapsed().as_secs() >= 3) {
                let fresh = media::volumes();
                if fresh.len() != self.media.volumes.len() || fresh.iter().zip(&self.media.volumes).any(|(a, b)| a.name != b.name || a.mounted != b.mounted) {
                    self.media.selected = None;
                    self.media.volumes = fresh;
                }
                self.media.last_scan = Some(std::time::Instant::now());
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(3));
        }
        if self.build.running.is_some() || self.media.rx.is_some() || self.media.drive_rx.is_some() || self.resolve_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        let bar = style::PANEL;
        egui::TopBottomPanel::top("top").frame(egui::Frame::new().fill(bar).inner_margin(egui::Margin::symmetric(10, 6))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Flat, VS Code style: bare text items, a highlight only on hover or while open.
                ui.scope(|ui| {
                    let w = &mut ui.style_mut().visuals.widgets;
                    for s in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                        s.bg_stroke = egui::Stroke::NONE;
                    }
                    w.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                    ui.style_mut().spacing.button_padding = egui::vec2(8.0, 3.0);
                    ui.menu_button("File", |ui| {
                        if ui.button("New").clicked() {
                            self.load(Model::starter());
                            self.status = "New config".into();
                            ui.close();
                        }
                        ui.menu_button("New from preset", |ui| {
                            if self.presets.is_empty() {
                                ui.label("No presets found (run from the checkout or set ARCHSTALER_ROOT).");
                            }
                            let mut pick = None;
                            for p in &self.presets {
                                if ui.button(&p.name).on_hover_text(&p.description).clicked() {
                                    pick = Some(p.path.clone());
                                    ui.close();
                                }
                            }
                            if let Some(path) = pick {
                                match Model::from_preset(&path) {
                                    Ok(m) => {
                                        self.status = format!("New config from preset {} (unsaved; presets erase the largest disk, see the Disk tab)", path.file_stem().unwrap_or_default().to_string_lossy());
                                        self.load(m);
                                    }
                                    Err(e) => self.status = e,
                                }
                            }
                        });
                        if ui.button("Open...").clicked() {
                            ui.close();
                            if let Some(p) = rfd::FileDialog::new().add_filter("Lua config", &["lua"]).pick_file() {
                                match Model::open(&p) {
                                    Ok(m) => {
                                        self.status = format!("Opened {}", p.display());
                                        self.load(m);
                                    }
                                    Err(e) => self.status = e,
                                }
                            }
                        }
                        ui.separator();
                        if ui.button("Save").clicked() {
                            ui.close();
                            self.save();
                        }
                        if ui.button("Save as...").clicked() {
                            ui.close();
                            self.save_as();
                        }
                    });
                    ui.menu_button("Tools", |ui| {
                        if ui.button("Copy config as Lua").clicked() {
                            ui.ctx().copy_text(self.model.lua());
                            self.status = "Lua copied to the clipboard".into();
                            ui.close();
                        }
                        let sha = self.build.iso.as_ref().map(|i| i.2.clone());
                        if ui.add_enabled(sha.is_some(), egui::Button::new("Copy ISO SHA-256")).clicked() {
                            ui.ctx().copy_text(sha.unwrap_or_default());
                            self.status = "SHA-256 copied to the clipboard".into();
                            ui.close();
                        }
                    });
                    if ui.button("Help").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab("https://github.com/Neekdo12/archstaller"));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let name = self.model.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into());
                    ui.label(format!("{name}{}", if self.model.dirty { " *" } else { "" }));
                    ui.add(egui::Label::new(egui::RichText::new(&self.status).small().weak()).truncate());
                });
            });
        });

        let problems = self.model.problems();
        egui::TopBottomPanel::bottom("problems").frame(egui::Frame::new().fill(bar).inner_margin(egui::Margin::symmetric(10, 4))).show(ctx, |ui| match problems.first() {
            Some(p) => style::note(ui, BAD, format!("Invalid: {}", p.message)),
            None => style::note(ui, OK, "The config is valid (the same checks as the command line)."),
        });

        egui::SidePanel::left("tabs").exact_width(176.0).resizable(false).frame(egui::Frame::new().fill(bar).inner_margin(egui::Margin::symmetric(8, 8))).show(ctx, |ui| {
            for (tab, name, area) in TABS {
                let bad = area.is_some_and(|a| problems.iter().any(|p| p.area == a));
                let selected = self.tab == *tab;
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::click());
                if selected || resp.hovered() {
                    let fill = if selected { style::ACCENT_BG } else { egui::Color32::from_rgb(0x24, 0x26, 0x2a) };
                    ui.painter().rect_filled(rect, 0.0, fill);
                }
                let color = if selected { egui::Color32::WHITE } else { egui::Color32::from_rgb(0xe0, 0xe2, 0xe4) };
                ui.painter().text(rect.min + egui::vec2(10.0, rect.height() / 2.0), egui::Align2::LEFT_CENTER, name, egui::TextStyle::Body.resolve(ui.style()), color);
                if bad {
                    ui.painter().circle_filled(rect.right_center() - egui::vec2(12.0, 0.0), 4.0, if selected { egui::Color32::WHITE } else { BAD });
                }
                if resp.clicked() {
                    self.tab = *tab;
                }
            }
        });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(style::BG).inner_margin(egui::Margin::symmetric(24, 10))).show(ctx, |ui| {
            if self.model.raw.is_some() && !matches!(self.tab, Tab::Preview | Tab::Iso) {
                style::page(ui, "Edited as text", "This config was not written by the GUI.", |ui| {
                    style::card(ui, None, |ui| {
                        ui.label("Open the \"Lua preview\" tab to edit it. The form tabs are for files the GUI generated: use New, or replace this file with a starter there.");
                    });
                });
                return;
            }
            egui::ScrollArea::vertical().id_salt("page").auto_shrink([false, false]).show(ui, |ui| match self.tab {
                Tab::System => self.tab_system(ui),
                Tab::Disk => self.tab_disk(ui),
                Tab::Packages => self.tab_packages(ui),
                Tab::Users => self.tab_users(ui),
                Tab::Services => self.tab_services(ui),
                Tab::Build => self.tab_build(ui),
                Tab::Scripts => self.tab_scripts(ui),
                Tab::Preview => self.tab_preview(ui),
                Tab::Iso => self.tab_iso(ui),
            });
        });

        // Earlier builds on the stick: delete them or rename the new one.
        let mut answer = None;
        if let Some(p) = &mut self.older {
            egui::Window::new("Older ISO on the stick").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label("The stick already holds an earlier build of this ISO:");
                for f in &p.files {
                    let size = std::fs::metadata(f).map(|m| m.len() >> 20).unwrap_or(0);
                    ui.label(format!("  {} ({} MiB)", f.file_name().unwrap_or_default().to_string_lossy(), size));
                }
                ui.add_space(6.0);
                ui.label("Delete the old one(s) for good, or keep them and store the new ISO under another name.");
                style::row(ui, "New name", |ui| {
                    style::text(ui, &mut p.new_name);
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete old, copy").clicked() {
                        answer = Some(Some(true));
                    }
                    if ui.add_enabled(!p.new_name.trim().is_empty() && !p.dir.join(p.new_name.trim()).exists(), egui::Button::new("Keep, rename new")).clicked() {
                        answer = Some(Some(false));
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(None);
                    }
                });
                if p.dir.join(p.new_name.trim()).exists() {
                    style::hint(ui, "That name is taken; choose another.");
                }
            });
        }
        if let Some(a) = answer {
            if let Some(p) = self.older.take() {
                match a {
                    Some(true) => self.start_copy(p.iso, p.volume, p.dir, p.files, Some(p.name)),
                    Some(false) => {
                        let n = p.new_name.trim().to_string();
                        let n = if n.to_ascii_lowercase().ends_with(".iso") { n } else { format!("{n}.iso") };
                        self.start_copy(p.iso, p.volume, p.dir, vec![], Some(n));
                    }
                    None => {
                        if self.media.cleanup.take().is_some() {
                            self.build.error = Some(format!("Not copied. The built ISO is kept at {}", p.iso.display()));
                        }
                    }
                }
            }
        }

        // Confirmations for irreversible choices.
        let mut resolved = None;
        if let Some(c) = &self.confirm {
            egui::Window::new("Please confirm").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                match c {
                    Confirm::AutoLargest => {
                        ui.label("With this option the installer wipes and installs onto the largest disk of whatever machine boots this ISO, with no confirmation. Use it only for machines you intend to erase.");
                    }
                    Confirm::Overwrite(p) => {
                        ui.label(format!("{} exists. Replace it with the new ISO?", p.display()));
                    }
                    Confirm::Aur => {
                        ui.label("AUR packages are built on the installed machine during its first boot, from recipes nobody at Arch reviewed. The build runs as an unprivileged user but it still executes the recipe's code there, and the result is not signed by Arch. You review and pin every recipe yourself. Continue?");
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("Yes, continue").clicked() {
                        resolved = Some(true);
                    }
                    if ui.button("No").clicked() {
                        resolved = Some(false);
                    }
                });
            });
        }
        if let Some(yes) = resolved {
            match (self.confirm.take(), yes) {
                (Some(Confirm::Aur), true) => {
                    self.aur.trust_ack = true;
                }
                (Some(Confirm::AutoLargest), true) => {
                    self.model.cfg.disk.auto_largest = true;
                    self.model.dirty = true;
                }
                (Some(Confirm::Overwrite(out)), true) => {
                    if let Some(cfg) = self.model.path.clone() {
                        self.start_build(cfg, out);
                    }
                }
                _ => {}
            }
        }
    }
}
