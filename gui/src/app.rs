//! The egui front end. All decisions live in `model`, `build`, `media` and `hostcfg`; this only draws them.
use crate::build::{self, Build, Msg};
use crate::media::{self, FolderCopy, MediaTarget, Phase, Volume};
use crate::model::{self, Area, Model, Preset};
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

enum Confirm {
    AutoLargest,
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
}

#[derive(Default)]
struct MediaState {
    volumes: Vec<Volume>,
    selected: Option<usize>,
    dir: Option<PathBuf>,
    overwrite: bool,
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

    fn lines(&mut self, ui: &mut egui::Ui, key: &'static str, get: impl Fn(&Model) -> Vec<String>, set: impl Fn(&mut Model, Vec<String>), height: f32) {
        let buf = self.bufs.entry(key).or_insert_with(|| get(&self.model).join("\n"));
        let r = egui::ScrollArea::vertical().id_salt(key).max_height(height).show(ui, |ui| ui.add(egui::TextEdit::multiline(buf).desired_width(f32::INFINITY).desired_rows(4)));
        if r.inner.changed() {
            let v: Vec<String> = buf.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
            set(&mut self.model, v);
            self.model.dirty = true;
        }
    }

    fn field(&mut self, ui: &mut egui::Ui, label: &str, get: impl Fn(&mut Model) -> &mut String) {
        ui.horizontal(|ui| {
            ui.label(format!("{label:<12}"));
            if ui.text_edit_singleline(get(&mut self.model)).changed() {
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
        ui.heading("System");
        self.field(ui, "Hostname", |m| &mut m.cfg.hostname);
        self.field(ui, "Timezone", |m| &mut m.cfg.timezone);
        self.field(ui, "Locale", |m| &mut m.cfg.locale);
        self.field(ui, "Keymap", |m| &mut m.cfg.keymap);
    }

    fn tab_disk(&mut self, ui: &mut egui::Ui) {
        ui.heading("Target disk");
        ui.label("The installer erases the disk it selects. Name it by serial number, or let it take the largest disk.");
        let mut auto = self.model.cfg.disk.auto_largest;
        if ui.checkbox(&mut auto, "Erase and install onto the LARGEST disk without asking").changed() {
            if auto {
                self.confirm = Some(Confirm::AutoLargest);
            } else {
                self.model.cfg.disk.auto_largest = false;
                self.model.dirty = true;
            }
        }
        ui.add_enabled_ui(!self.model.cfg.disk.auto_largest, |ui| {
            self.field(ui, "Serial", |m| &mut m.cfg.disk.confirm_serial);
            let mut has = self.model.cfg.disk.model.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut has, "Also match the model").changed() {
                    self.model.cfg.disk.model = has.then(String::new);
                    self.model.dirty = true;
                }
                if let Some(m) = &mut self.model.cfg.disk.model {
                    if ui.text_edit_singleline(m).changed() {
                        self.model.dirty = true;
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("ESP size (MiB)");
            if ui.add(egui::DragValue::new(&mut self.model.cfg.disk.esp_mib).range(64..=8192)).changed() {
                self.model.dirty = true;
            }
        });
    }

    fn tab_packages(&mut self, ui: &mut egui::Ui) {
        ui.heading("Mirrors");
        ui.label("One URL per line; $repo and $arch are substituted.");
        self.lines(ui, "mirrors", |m| m.cfg.mirrors.clone(), |m, v| m.cfg.mirrors = v, 90.0);
        ui.separator();
        ui.heading("Packages");
        ui.label("Official packages and groups (core and extra), one per line.");
        self.lines(ui, "packages", |m| m.cfg.packages.clone(), |m, v| m.cfg.packages = v, 220.0);
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
                ui.label("fetching the package databases...");
            }
        });
        match &self.resolve {
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            Some(Ok(r)) => {
                ui.label(format!("{} packages, {} MiB to download", r.packages.len(), r.download_bytes() >> 20));
                for a in &r.ambiguities {
                    ui.colored_label(egui::Color32::YELLOW, format!("{} is provided by {}; chosen {} (set it in `providers` to pick another)", a.dep, a.candidates.join(", "), a.chosen));
                }
                egui::ScrollArea::vertical().id_salt("resolved").max_height(180.0).show(ui, |ui| {
                    for p in &r.packages {
                        ui.label(format!("{}{} {} ({}, {} KiB)", if p.explicit { "* " } else { "  " }, p.name, p.version, p.repo, p.csize >> 10));
                    }
                });
            }
            None => {}
        }
    }

    fn tab_users(&mut self, ui: &mut egui::Ui) {
        ui.heading("Users");
        ui.label("Passwords are stored as SHA-512 crypt hashes only; the plaintext is never saved.");
        let mut remove = None;
        for (i, u) in self.model.cfg.users.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}  groups: {}  shell: {}", u.name, u.groups.join(","), u.shell));
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.model.cfg.users.remove(i);
            self.model.dirty = true;
        }
        ui.separator();
        ui.label("Add a user");
        let n = &mut self.new_user;
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut n.name);
        });
        ui.horizontal(|ui| {
            ui.label("Password");
            ui.add(egui::TextEdit::singleline(&mut n.pw).password(true));
            ui.label("Again");
            ui.add(egui::TextEdit::singleline(&mut n.pw2).password(true));
        });
        ui.horizontal(|ui| {
            ui.label("Groups");
            ui.text_edit_singleline(&mut n.groups);
            ui.label("Shell");
            ui.text_edit_singleline(&mut n.shell);
        });
        let ok = !n.name.is_empty() && !n.pw.is_empty() && n.pw == n.pw2;
        if !n.pw.is_empty() && n.pw != n.pw2 {
            ui.colored_label(egui::Color32::LIGHT_RED, "The passwords differ");
        }
        if ui.add_enabled(ok, egui::Button::new("Add user")).clicked() {
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
        ui.separator();
        ui.label("root account");
        let mut locked = self.model.cfg.root_password_hash.is_none();
        if ui.checkbox(&mut locked, "Leave root locked (use sudo)").changed() {
            if locked {
                self.model.cfg.root_password_hash = None;
            } else {
                self.model.cfg.root_password_hash = Some(String::new());
            }
            self.model.dirty = true;
        }
        if self.model.cfg.root_password_hash.is_some() {
            ui.label("Set a root password in the user form above and paste its hash here, or edit the Lua file: root_password_hash.");
            if let Some(h) = &mut self.model.cfg.root_password_hash {
                if ui.text_edit_singleline(h).changed() {
                    self.model.dirty = true;
                }
            }
        }
    }

    fn tab_services(&mut self, ui: &mut egui::Ui) {
        ui.heading("Services enabled on first boot");
        self.lines(ui, "services", |m| m.cfg.services.clone(), |m, v| m.cfg.services = v, 140.0);
        ui.heading("Extra kernel parameters");
        ui.label("One word per line, for the installed system's kernel command line.");
        self.lines(ui, "kernel_params", |m| m.cfg.kernel_params.clone(), |m, v| m.cfg.kernel_params = v, 120.0);
    }

    fn tab_build(&mut self, ui: &mut egui::Ui) {
        ui.heading("Build profile");
        let current = self.model.host.build.profile.clone().unwrap_or_else(|| "super-small".into());
        let mut chosen = current.clone();
        ui.radio_value(&mut chosen, "super-small".into(), "super-small: size-optimized installer (the default). About 0.7 MiB; a crash shows no panic message.");
        ui.radio_value(&mut chosen, "large".into(), "large: regular release build, panic messages kept. About 0.8 MiB.");
        ui.add_enabled(false, egui::RadioButton::new(false, "extra-large: reserved, not available yet"));
        if chosen != current {
            self.model.set_profile(&chosen);
        }
        ui.separator();
        let mut teth = self.model.host.build.tethering;
        if ui.checkbox(&mut teth, "Include USB tethering (iPhone, Android, USB Ethernet), about +100 KiB").changed() {
            self.model.host.build.tethering = teth;
            self.model.dirty = true;
        }
        ui.separator();
        ui.heading("Installer drivers");
        ui.label("Drivers of the installer itself, not packages for the installed system. Leave all selected unless you know the hardware.");
        let mut all = self.model.host.installer_drivers.is_none();
        if ui.checkbox(&mut all, "All drivers").changed() {
            self.model.host.installer_drivers = if all { None } else { Some(DRIVERS.iter().map(|d| d.id.to_string()).collect()) };
            self.model.dirty = true;
        }
        if let Some(list) = self.model.host.installer_drivers.clone() {
            let mut next = list.clone();
            for (class, title) in [(DriverClass::Storage, "Storage"), (DriverClass::Network, "Network")] {
                ui.label(title);
                for d in DRIVERS.iter().filter(|d| d.class == class) {
                    let mut on = list.iter().any(|i| i == d.id);
                    if ui.checkbox(&mut on, format!("{}: {} ({})", d.id, d.description, d.status)).changed() {
                        if on {
                            next.push(d.id.to_string());
                        } else {
                            next.retain(|i| i != d.id);
                        }
                    }
                }
            }
            if next != list {
                self.model.host.installer_drivers = Some(next);
                self.model.dirty = true;
            }
        }
        ui.separator();
        ui.label("AUR packages: not available yet.");
    }

    fn tab_scripts(&mut self, ui: &mut egui::Ui) {
        ui.heading("First-boot scripts");
        ui.colored_label(egui::Color32::YELLOW, "Scripts are code. They run as root on the installed system, in this order, at the end of the first boot. A failing script only logs a warning.");
        let mut remove = None;
        for (i, s) in self.model.cfg.scripts.iter().enumerate() {
            ui.horizontal(|ui| {
                let src = match hostcfg::scripts::source(s) {
                    hostcfg::scripts::Source::Builtin => "built-in".to_string(),
                    hostcfg::scripts::Source::File(f) => format!("local file {f}"),
                    hostcfg::scripts::Source::Remote { url, sha256 } => format!("{url}, sha256 {sha256}"),
                };
                ui.label(format!("{}. {} ({src}) args: [{}]", i + 1, s.id, s.args.join(" ")));
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.model.cfg.scripts.remove(i);
            self.model.dirty = true;
        }
        ui.separator();
        ui.label("Built-in scripts");
        for b in hostcfg::scripts::BUILTINS {
            ui.horizontal(|ui| {
                ui.label(format!("{}: {} (runs as {}, {}; needs: {})", b.id, b.description, b.runs_as, b.phase, if b.requires.is_empty() { "nothing" } else { b.requires }));
                if ui.button("Add").clicked() && !self.model.cfg.scripts.iter().any(|s| s.id == b.id) {
                    self.model.cfg.scripts.push(config::Script { id: b.id.into(), ..Default::default() });
                    self.model.dirty = true;
                }
            });
        }
        ui.separator();
        ui.label("Custom script: a local file, or an https URL pinned by its SHA-256 digest");
        let n = &mut self.new_script;
        ui.horizontal(|ui| {
            ui.label("Id");
            ui.text_edit_singleline(&mut n.0);
        });
        ui.horizontal(|ui| {
            ui.label("Local file");
            ui.text_edit_singleline(&mut n.1);
            if ui.button("Browse...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_file() {
                    n.1 = p.display().to_string();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("URL");
            ui.text_edit_singleline(&mut n.2);
            ui.label("SHA-256");
            ui.text_edit_singleline(&mut n.3);
        });
        if !n.1.is_empty() {
            if let Ok(text) = std::fs::read_to_string(&n.1) {
                let digest = {
                    use sha2::Digest;
                    sha2::Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()
                };
                ui.label(format!("sha256 {digest}"));
                egui::ScrollArea::vertical().id_salt("scriptsrc").max_height(120.0).show(ui, |ui| ui.monospace(&text));
            }
        }
        ui.checkbox(&mut self.root_script_ack, "I understand a custom script runs as root on the installed system");
        let valid = !n.0.is_empty() && (!n.1.is_empty() || !n.2.is_empty());
        if ui.add_enabled(valid && self.root_script_ack, egui::Button::new("Add custom script")).clicked() {
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
    }

    fn tab_preview(&mut self, ui: &mut egui::Ui) {
        ui.heading("Lua");
        match &mut self.model.raw {
            Some(text) => {
                ui.label("This file was not written by the GUI, so it is edited as text and checked as it is.");
                if ui.add(egui::TextEdit::multiline(text).code_editor().desired_width(f32::INFINITY).desired_rows(30)).changed() {
                    self.model.dirty = true;
                }
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
                egui::ScrollArea::vertical().show(ui, |ui| ui.add(egui::TextEdit::multiline(&mut text).code_editor().desired_width(f32::INFINITY).interactive(false)));
            }
        }
    }

    fn tab_iso(&mut self, ui: &mut egui::Ui) {
        ui.heading("Build the ISO");
        let problems = self.model.problems();
        if let Some(p) = problems.first() {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("The config is invalid: {}", p.message));
        }
        match self.model.path.clone() {
            Some(p) => {
                ui.label(format!("Config: {}", p.display()));
            }
            None => {
                ui.label("The config has not been saved yet; a build uses the saved file.");
            }
        }
        ui.label(format!("Profile: {}   Tethering: {}   Drivers: {}", self.model.effective_profile(), self.model.host.build.tethering, self.model.host.drivers().join(", ")));
        if self.model.cfg.disk.auto_largest {
            ui.colored_label(egui::Color32::YELLOW, "This ISO erases the largest disk of the machine it boots on, without asking.");
        }
        for l in hostcfg::scripts::summary(&self.model.cfg) {
            ui.label(l);
        }
        let can = problems.is_empty() && self.build.running.is_none();
        ui.horizontal(|ui| {
            if ui.add_enabled(can, egui::Button::new("Save and build...")).clicked() {
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
            if self.build.running.is_some() && ui.button("Cancel").clicked() {
                if let Some(b) = self.build.running.take() {
                    b.cancel();
                    self.build.error = Some(format!("Cancelled. Log kept at {}", b.log_path.display()));
                    self.build.iso = None;
                }
            }
        });
        for (name, st) in &self.build.stages {
            let (mark, color) = match st {
                State::Start => ("...", egui::Color32::YELLOW),
                State::Ok => ("ok ", egui::Color32::LIGHT_GREEN),
                State::Fail => ("FAILED", egui::Color32::LIGHT_RED),
            };
            ui.colored_label(color, format!("{mark} {name}"));
        }
        if let Some(e) = &self.build.error {
            ui.colored_label(egui::Color32::LIGHT_RED, e);
        }
        if let Some((path, size, sha)) = &self.build.iso {
            ui.colored_label(egui::Color32::LIGHT_GREEN, format!("Built {} ({} KiB)", path.display(), size >> 10));
            ui.label(format!("sha256 {sha}"));
            if let Some(s) = &self.build.summary {
                ui.label(s);
            }
        }
        ui.separator();
        ui.label("Build log");
        egui::ScrollArea::vertical().id_salt("log").stick_to_bottom(true).max_height(160.0).show(ui, |ui| {
            for l in &self.build.log {
                ui.monospace(l);
            }
        });
        ui.separator();
        self.media_section(ui);
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
        let mut done: Option<(String, u64, String)> = None;
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
                Msg::Event(Event::Done { path, size, sha256, .. }) => done = Some((path, size, sha256)),
                Msg::Event(Event::Failed { message }) => self.build.error = Some(message),
                Msg::Exit(ok) => exit = Some(ok),
            }
        }
        if let Some(ok) = exit {
            let b = self.build.running.take().unwrap();
            let _ = std::fs::write(&b.log_path, self.build.log.join("\n"));
            match (ok, done) {
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
        ui.heading("Put it on a stick");
        let iso = self.media.iso_override.clone().or_else(|| self.build.iso.as_ref().map(|i| i.0.clone()));
        ui.horizontal(|ui| {
            ui.label(match &iso {
                Some(p) => format!("ISO: {}", p.display()),
                None => "Build an ISO first, or pick one.".into(),
            });
            if ui.button("Pick an ISO...").clicked() {
                self.media.iso_override = rfd::FileDialog::new().add_filter("ISO image", &["iso"]).pick_file();
            }
        });
        ui.label("Copy to Ventoy adds the ISO as a file; nothing on the stick is formatted or repartitioned. Raw flashing is not available yet.");
        if ui.button("Refresh volumes").clicked() {
            self.media.volumes = media::volumes();
            self.media.selected = None;
        }
        let all = self.media.volumes.clone();
        for (i, v) in all.iter().enumerate() {
            let ventoy = media::is_ventoy(v, &all);
            let tag = if ventoy { "  [Ventoy]" } else if v.removable { "  [removable]" } else { "" };
            let label = format!("{}  {}  {}  {} GiB free of {} GiB{tag}", v.name, v.mount.display(), v.fs, v.available >> 30, v.total >> 30);
            if ui.radio(self.media.selected == Some(i), label).clicked() {
                self.media.selected = Some(i);
                self.media.dir = Some(v.mount.clone());
            }
        }
        if let Some(i) = self.media.selected {
            let v = &all[i];
            if !media::is_ventoy(v, &all) {
                ui.colored_label(egui::Color32::YELLOW, "Ventoy was not detected on this volume (no ventoy folder or ventoy.json). The ISO will only be copied into the folder you choose.");
            }
            ui.horizontal(|ui| {
                ui.label(format!("Destination folder: {}", self.media.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default()));
                if ui.button("Choose...").clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_directory(&v.mount).pick_folder() {
                        self.media.dir = Some(d);
                    }
                }
            });
            ui.checkbox(&mut self.media.overwrite, "Overwrite a file with the same name");
        }
        let busy = self.media.rx.is_some();
        let ready = iso.is_some() && self.media.selected.is_some() && !busy;
        ui.horizontal(|ui| {
            if ui.add_enabled(ready, egui::Button::new("Copy to the selected volume")).clicked() {
                let (iso, v) = (iso.clone().unwrap(), all[self.media.selected.unwrap()].clone());
                let target = FolderCopy { dir: self.media.dir.clone().unwrap_or(v.mount.clone()), available: Some(v.available), overwrite: self.media.overwrite };
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
            if busy && ui.button("Cancel").clicked() {
                self.media.cancel.store(true, Ordering::Relaxed);
            }
        });
        if let Some((phase, done, total)) = self.media.progress {
            if busy {
                let f = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                ui.add(egui::ProgressBar::new(f).text(match phase {
                    Phase::Copying => "copying",
                    Phase::Verifying => "verifying",
                }));
            }
        }
        match &self.media.result {
            Some(Ok(m)) => {
                ui.colored_label(egui::Color32::LIGHT_GREEN, m);
            }
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("Not copied: {e}"));
            }
            None => {}
        }
    }

    fn pump_media(&mut self) {
        let mut finished = false;
        if let Some(rx) = &self.media.rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    MediaMsg::Progress(p, d, t) => self.media.progress = Some((p, d, t)),
                    MediaMsg::Done(r) => {
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
        self.pump_build();
        self.pump_media();
        if self.build.running.is_some() || self.media.rx.is_some() || self.resolve_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("New").clicked() {
                    self.load(Model::starter());
                    self.status = "New config".into();
                }
                ui.menu_button("From preset", |ui| {
                    if self.presets.is_empty() {
                        ui.label("No presets found (run from the checkout or set ARCHSTALER_ROOT).");
                    }
                    let mut pick = None;
                    for p in &self.presets {
                        if ui.button(&p.name).on_hover_text(&p.description).clicked() {
                            pick = Some(p.path.clone());
                            ui.close_menu();
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
                if ui.button("Save").clicked() {
                    self.save();
                }
                if ui.button("Save as...").clicked() {
                    self.save_as();
                }
                ui.separator();
                let name = self.model.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(unsaved)".into());
                ui.label(format!("{name}{}", if self.model.dirty { " *" } else { "" }));
                ui.separator();
                ui.label(&self.status);
            });
        });

        let problems = self.model.problems();
        egui::TopBottomPanel::bottom("problems").show(ctx, |ui| match problems.first() {
            Some(p) => {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("Invalid: {}", p.message));
            }
            None => {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "The config is valid (the same checks as the command line).");
            }
        });

        egui::SidePanel::left("tabs").resizable(false).show(ctx, |ui| {
            for (tab, name, area) in TABS {
                let bad = area.is_some_and(|a| problems.iter().any(|p| p.area == a));
                let text = if bad { egui::RichText::new(format!("{name} !")).color(egui::Color32::LIGHT_RED) } else { egui::RichText::new(*name) };
                if ui.selectable_label(self.tab == *tab, text).clicked() {
                    self.tab = *tab;
                }
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.model.raw.is_some() && !matches!(self.tab, Tab::Preview | Tab::Iso) {
                ui.label("This config was not written by the GUI: it is edited as text on the \"Lua preview\" tab. The form tabs are for GUI-generated files.");
                return;
            }
            egui::ScrollArea::vertical().id_salt("page").show(ui, |ui| match self.tab {
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
