//! The document being edited: the installer's config plus the host build settings. No UI here, so it can be tested.
use config::{Config, Disk};
use hostcfg::host::HostConfig;
use hostcfg::writer::{self, HEADER};
use std::path::{Path, PathBuf};

pub struct Model {
    pub cfg: Config,
    pub host: HostConfig,
    pub path: Option<PathBuf>,
    /// A file not written by this app: shown and edited as text, never regenerated from the form.
    pub raw: Option<String>,
    /// `raw` came from a file the GUI did not write: leaving text mode replaces it with form output.
    pub foreign: bool,
    pub dirty: bool,
}

/// What a validation message is about, so the UI can show it next to the right tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    System,
    Disk,
    Packages,
    Users,
    Services,
    Build,
    Scripts,
    Other,
}

pub struct Problem {
    pub area: Area,
    pub message: String,
}

pub fn area_of(message: &str) -> Area {
    let m = message;
    if m.starts_with("scripts[") {
        Area::Scripts
    } else if m.starts_with("aur_packages") {
        Area::Packages
    } else if m.starts_with("installer_drivers") || m.starts_with("build.") {
        Area::Build
    } else if m.contains("hostname") || m.contains("timezone") || m.contains("locale") || m.contains("keymap") {
        Area::System
    } else if m.contains("disk.") {
        Area::Disk
    } else if m.contains("mirror") || m.contains("package") {
        Area::Packages
    } else if m.contains("user") || m.contains("password") || m.contains("shell") || m.contains("group") {
        Area::Users
    } else if m.contains("service") || m.contains("kernel parameter") {
        Area::Services
    } else {
        Area::Other
    }
}

impl Model {
    /// A starting point: the defaults of the example config, with no disk chosen yet.
    pub fn starter() -> Model {
        Model {
            cfg: Config {
                hostname: "archbox".into(),
                timezone: "Europe/Prague".into(),
                locale: "en_US.UTF-8".into(),
                keymap: "us".into(),
                disk: Disk { model: None, confirm_serial: String::new(), auto_largest: false, esp_mib: 1024 },
                mirrors: vec!["https://geo.mirror.pkgbuild.com/$repo/os/$arch".into()],
                packages: ["base", "linux", "linux-firmware-intel", "linux-firmware-realtek", "mkinitcpio", "grub", "efibootmgr", "sudo", "nano"].map(String::from).to_vec(),
                providers: vec![("initramfs".into(), "mkinitcpio".into())],
                root_password_hash: None,
                users: vec![],
                services: vec![],
                kernel_params: vec![],
                user_files: vec![],
                user_archives: vec![],
                dry_run: false,
                aur: vec![],
                scripts: vec![],
            },
            host: HostConfig::default(),
            path: None,
            raw: None,
            foreign: false,
            dirty: false,
        }
    }

    /// Opens a Lua file. Files this app wrote are edited as a form; any other file as text.
    pub fn open(path: &Path) -> Result<Model, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let generated = text.starts_with(HEADER);
        match hostcfg::lua::load(path) {
            Ok(l) => Ok(Model { cfg: l.config, host: l.host, path: Some(path.to_path_buf()), raw: (!generated).then_some(text), foreign: !generated, dirty: false }),
            // An invalid file can still be opened as text to fix it.
            Err(e) if !generated => {
                let mut m = Model::starter();
                m.path = Some(path.to_path_buf());
                m.raw = Some(text);
                m.foreign = true;
                m.dirty = false;
                let _ = e;
                Ok(m)
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// A new, unsaved document from one of the repository's presets (`configs/`). The preset is
    /// evaluated like the CLI does and then edited as an ordinary form; saving writes plain Lua.
    pub fn from_preset(path: &Path) -> Result<Model, String> {
        let mut l = hostcfg::lua::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // The document is unsaved and will land in another directory (or a temp copy): script paths
        // that were relative to the preset must not depend on where the copy ends up.
        for s in &mut l.config.scripts {
            if let Some(f) = s.file.as_mut().filter(|f| std::path::Path::new(f.as_str()).is_relative()) {
                *f = l.dir.join(&*f).to_string_lossy().into_owned();
            }
        }
        Ok(Model { cfg: l.config, host: l.host, path: None, raw: None, foreign: false, dirty: true })
    }

    /// The Lua text this document stands for.
    pub fn lua(&self) -> String {
        match &self.raw {
            Some(t) => t.clone(),
            None => writer::to_lua(&self.cfg, &self.host),
        }
    }

    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.lua()).map_err(|e| format!("{}: {e}", path.display()))?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    /// Everything wrong with the document, by the same code the CLI runs. A raw file is checked by
    /// evaluating its text (through a temporary file next to it so relative paths resolve).
    pub fn problems(&self) -> Vec<Problem> {
        let result: Result<(), String> = match &self.raw {
            None => self.cfg.validate().and_then(|_| self.host.validate()).and_then(|_| hostcfg::scripts::validate(&self.cfg)),
            Some(text) => self.check_text(text),
        };
        match result {
            Ok(()) => vec![],
            Err(m) => vec![Problem { area: area_of(&m), message: m }],
        }
    }

    /// Starts source mode for a generated config: the form is serialized once and the text is what
    /// gets edited from here on.
    pub fn enter_source(&mut self) {
        if self.raw.is_none() {
            self.raw = Some(writer::to_lua(&self.cfg, &self.host));
            self.foreign = false;
        }
    }

    /// Evaluates the source text through the loader the CLI uses (the same syntax, shape and type checks)
    /// and goes back to the form with what it says. A config that is well formed but incomplete (no disk
    /// chosen yet, say) comes back too: the forms and the problem bar then point at what is missing. A
    /// syntax or type error keeps source mode, changes nothing and returns the message.
    pub fn leave_source(&mut self) -> Result<(), String> {
        let Some(text) = self.raw.clone() else { return Ok(()) };
        let l = self.evaluate(&text, false)?;
        self.cfg = l.config;
        self.host = l.host;
        self.raw = None;
        self.foreign = false;
        self.dirty = true;
        Ok(())
    }

    /// The text checked like a file: used for the diagnostics line while editing source.
    pub fn check_source(&self, text: &str) -> Result<(), String> {
        self.evaluate(text, true).map(|_| ())
    }

    fn evaluate(&self, text: &str, validate: bool) -> Result<hostcfg::lua::Loaded, String> {
        let dir = self.path.as_deref().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
        let tmp = dir.join(format!(".archstaller-check-{}.lua", std::process::id()));
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        // The messages name the temporary file; the user knows it as the text in the editor.
        let r = if validate { hostcfg::lua::load(&tmp) } else { hostcfg::lua::load_unvalidated(&tmp) };
        let r = r.map_err(|e| e.to_string().replace(tmp.to_string_lossy().as_ref(), "source.lua"));
        let _ = std::fs::remove_file(&tmp);
        r
    }

    fn check_text(&self, text: &str) -> Result<(), String> {
        self.check_source(text)
    }

    /// The config that will be built: the saved file, so CLI and GUI build the same thing.
    pub fn effective_profile(&self) -> String {
        self.host.profile().map(|p| p.name().to_string()).unwrap_or_else(|e| e)
    }

    pub fn set_profile(&mut self, name: &str) {
        self.host.build.profile = Some(name.to_string());
        self.dirty = true;
    }
}

pub use hostcfg::configs::Preset;

/// The 1-based line a Lua error message points at (`chunk.lua:12: ...`), if it names one. Messages
/// from the config validation carry no line and give `None`: none is invented.
pub fn located(msg: &str) -> Option<usize> {
    // Lua names the chunk after the file (`path.lua:12:`) or, for an unnamed chunk, `[string "..."]:12:`.
    for marker in [".lua:", "]:"] {
        let mut rest = msg;
        while let Some(i) = rest.find(marker) {
            let after = &rest[i + marker.len()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() && after[digits.len()..].starts_with(':') {
                return digits.parse().ok();
            }
            rest = after;
        }
    }
    None
}

/// The presets under `<root>/configs` (files marked `-- archstaller: kind=preset`), by name.
pub fn presets(root: &Path) -> Vec<Preset> {
    hostcfg::configs::presets(&root.join(hostcfg::configs::DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("archstaller-gui-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn starter_needs_a_disk_choice() {
        let m = Model::starter();
        let p = m.problems();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].area, Area::Disk);
    }

    #[test]
    fn save_and_reopen_preserves_everything() {
        let mut m = Model::starter();
        m.cfg.disk.confirm_serial = "SER123".into();
        m.cfg.users.push(config::User { name: "arch".into(), password_hash: hostcfg::password::hash("pw").unwrap(), groups: vec!["wheel".into()], shell: "/bin/bash".into() });
        m.cfg.scripts.push(config::Script { id: "enable-sshd".into(), ..Default::default() });
        m.host.build.profile = Some("large".into());
        m.host.installer_drivers = Some(vec!["virtio-blk".into(), "nvme".into(), "e1000".into()]);
        assert!(m.problems().is_empty());
        let dir = tmp("roundtrip");
        let path = dir.join("c.lua");
        m.save(&path).unwrap();
        let back = Model::open(&path).unwrap();
        assert!(back.raw.is_none(), "a generated file is edited as a form");
        assert_eq!(format!("{:?}", back.cfg), format!("{:?}", m.cfg));
        assert_eq!(back.host, m.host);
        assert_eq!(back.host.drivers(), vec!["virtio-blk", "nvme", "e1000"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn foreign_lua_opens_as_text_and_is_checked_by_the_cli_code() {
        let dir = tmp("raw");
        let path = dir.join("hand.lua");
        std::fs::write(&path, "return { hostname = \"bad host\" }").unwrap();
        let m = Model::open(&path).unwrap();
        assert!(m.raw.is_some());
        assert!(!m.problems().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gui_and_cli_reject_the_same_config() {
        let mut m = Model::starter();
        m.cfg.disk.confirm_serial = "S".into();
        m.host.build.profile = Some("extra-large".into());
        let gui = m.problems();
        let dir = tmp("same");
        let path = dir.join("c.lua");
        m.save(&path).unwrap();
        let cli = hostcfg::lua::load(&path).err().map(|e| e.to_string());
        assert_eq!(gui.first().map(|p| p.message.clone()), cli);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_repository_preset_opens_as_an_editable_form() {
        let root = crate::build::find_root().unwrap();
        let list = presets(&root);
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["hyprland", "i3", "minimal", "omarchy", "plasma", "server", "sway", "tester"]);
        for p in &list {
            assert!(!p.description.is_empty(), "{}", p.name);
            let m = Model::from_preset(&p.path).unwrap_or_else(|e| panic!("{}: {e}", p.name));
            assert!(m.raw.is_none() && m.path.is_none() && m.dirty);
            assert!(m.problems().is_empty(), "{}: {:?}", p.name, m.problems().first().map(|x| &x.message));
            // Saved as plain Lua, it loads back to the same thing.
            let dir = std::env::temp_dir().join(format!("archstaller-preset-{}-{}", std::process::id(), p.name));
            std::fs::create_dir_all(&dir).unwrap();
            let mut m = m;
            m.save(&dir.join("c.lua")).unwrap();
            let back = Model::open(&dir.join("c.lua")).unwrap();
            assert_eq!(format!("{:?}", back.cfg), format!("{:?}", m.cfg), "{}", p.name);
            assert_eq!(back.host, m.host, "{}", p.name);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn error_lines_are_found_only_when_the_message_has_one() {
        assert_eq!(located("syntax error: /tmp/c.lua:12: unexpected symbol near '}'"), Some(12));
        assert_eq!(located("runtime error: /home/x/.archstaller-check-1.lua:3: boom\nstack traceback:"), Some(3));
        assert_eq!(located("as.system.hostname: invalid type: integer `5`, expected a string"), None);
        assert_eq!(located("invalid hostname: \"x\""), None);
        assert_eq!(located("file.lua:abc: nope"), None);
    }

    #[test]
    fn source_mode_round_trips_and_refuses_bad_text() {
        let mut m = Model::starter();
        m.cfg.disk.confirm_serial = "SER1".into();
        m.enter_source();
        let text = m.raw.clone().unwrap();
        assert!(text.starts_with(HEADER) && !m.foreign);
        // Edit the text: change the hostname and leave source mode.
        m.raw = Some(text.replace("hostname = \"archbox\"", "hostname = \"edited\""));
        m.leave_source().unwrap();
        assert!(m.raw.is_none() && m.dirty);
        assert_eq!(m.cfg.hostname, "edited");
        // A syntax error keeps the text and the old values.
        m.enter_source();
        m.raw = Some("return { as = ".into());
        let e = m.leave_source().unwrap_err();
        assert!(m.raw.is_some() && m.cfg.hostname == "edited");
        assert!(located(&e).is_some(), "{e}");
        // A shape error (a missing section) has a path but no line, and keeps source mode too.
        m.raw = Some(m.raw.clone().unwrap().replace("return { as = ", "return { as = { schema = 1 } } --"));
        let e = m.leave_source().unwrap_err();
        assert!(m.raw.is_some() && e.contains("missing field"), "{e}");
        // A well formed but incomplete config (no disk chosen) returns to the forms, which then show the problem.
        let mut m = Model::starter();
        m.enter_source();
        m.leave_source().unwrap();
        assert!(m.raw.is_none() && m.problems().len() == 1);
    }

    #[test]
    fn messages_map_to_areas() {
        assert_eq!(area_of("scripts[2]: bad"), Area::Scripts);
        assert_eq!(area_of("installer_drivers[1]: x"), Area::Build);
        assert_eq!(area_of("invalid hostname: \"x\""), Area::System);
        assert_eq!(area_of("disk.confirm_serial must be set"), Area::Disk);
        assert_eq!(area_of("invalid mirror: x"), Area::Packages);
        assert_eq!(area_of("invalid user name: x"), Area::Users);
        assert_eq!(area_of("aur_packages[1]: commit must be a full 40-digit lower-case hex commit id"), Area::Packages);
        assert_eq!(area_of("invalid service: x"), Area::Services);
    }
}
