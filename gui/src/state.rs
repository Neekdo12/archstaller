//! The data the GTK front end keeps besides the document: running work and what it reported.
//! No widgets here.
use crate::aur::AurState;
use crate::build::Build;
use crate::data;
use crate::media::{Phase, Volume};
use crate::model::{Model, Preset};
use hostcfg::progress::State as StageState;
use hostcfg::resolve::Resolution;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::rc::Rc;
use std::sync::Arc;

/// Suggestion lists, read once at start.
pub struct Lists {
    pub timezones: Rc<Vec<String>>,
    pub locales: Rc<Vec<String>>,
    pub keymaps: Rc<Vec<String>>,
    pub shells: Rc<Vec<String>>,
    pub groups: Rc<Vec<String>>,
    pub services: Rc<Vec<String>>,
    pub kernel_params: Rc<Vec<String>>,
}

impl Lists {
    pub fn load() -> Lists {
        Lists {
            timezones: Rc::new(data::timezones()),
            locales: Rc::new(data::locales()),
            keymaps: Rc::new(data::keymaps()),
            shells: Rc::new(data::shells()),
            groups: Rc::new(data::groups()),
            services: Rc::new(data::services()),
            kernel_params: Rc::new(data::kernel_params()),
        }
    }
}

#[derive(Default)]
pub struct BuildState {
    pub running: Option<Build>,
    pub stages: Vec<(String, StageState)>,
    pub log: Vec<String>,
    /// Lines of `log` already shown by the live log widget.
    pub log_shown: usize,
    pub error: Option<String>,
    /// The finished ISO, once it is in place and checked.
    pub iso: Option<(PathBuf, u64, String)>,
    pub out: Option<PathBuf>,
    pub summary: Option<String>,
    /// Build straight onto this Ventoy volume under this file name, with no copy kept elsewhere.
    pub stick: Option<(Volume, String)>,
    /// The `done` event, kept until the process exit arrives (they can come in different reads).
    pub done: Option<(String, u64, String)>,
}

pub enum MediaMsg {
    Progress(Phase, u64, u64),
    Done(Result<crate::media::Report, String>),
}

#[derive(Default)]
pub struct MediaState {
    pub volumes: Vec<Volume>,
    /// Save the built ISO as a file even when a Ventoy drive is present.
    pub to_file: bool,
    /// Ventoy volume to build onto (index into `volumes`); the first one when unset or stale.
    pub stick: Option<usize>,
    pub last_scan: Option<std::time::Instant>,
    /// Show internal disks too, not only removable ones.
    pub show_internal: bool,
    /// A mount or unmount in progress (device) and its answer.
    pub drive_rx: Option<Receiver<Result<String, String>>>,
    pub drive_busy: Option<String>,
    pub drive_msg: Option<Result<String, String>>,
    /// Scratch build directory to remove once the ISO is safely on the stick.
    pub cleanup: Option<PathBuf>,
    pub selected: Option<usize>,
    pub dir: Option<PathBuf>,
    pub iso_override: Option<PathBuf>,
    pub progress: Option<(Phase, u64, u64)>,
    pub rx: Option<Receiver<MediaMsg>>,
    pub cancel: Arc<AtomicBool>,
    pub result: Option<Result<String, String>>,
    /// The running or last media operation is a raw flash, not a file copy.
    pub flashing: bool,
}

pub struct State {
    pub model: Model,
    pub status: String,
    pub lists: Lists,
    pub aur: AurState,
    pub resolve: Option<Result<Resolution, String>>,
    pub resolve_rx: Option<Receiver<Result<Resolution, String>>>,
    pub build: BuildState,
    pub media: MediaState,
    pub next_build: u32,
    pub presets: Vec<Preset>,
    /// Set by background work when the visible page has something new to show.
    pub ui_dirty: bool,
    /// A search was started by the AUR group: when its results arrive, an exact name match is reviewed at once.
    pub aur_auto_review: bool,
    /// Commit of the review whose button already got the keyboard focus (so a refresh does not take it again).
    pub aur_focused_commit: Option<String>,
    /// A package was just pinned: put the keyboard focus back on the AUR search box for the next one.
    pub aur_focus_search: bool,
}

impl State {
    pub fn new() -> State {
        let mut s = State {
            model: Model::starter(),
            status: "New config".into(),
            lists: Lists::load(),
            aur: AurState::default(),
            resolve: None,
            resolve_rx: None,
            build: BuildState::default(),
            media: MediaState::default(),
            next_build: 0,
            presets: crate::build::find_root().map(|r| crate::model::presets(&r)).unwrap_or_default(),
            ui_dirty: false,
            aur_auto_review: false,
            aur_focused_commit: None,
            aur_focus_search: false,
        };
        s.media.volumes = crate::media::volumes();
        s
    }

    /// Replaces the document and forgets what belonged to the old one.
    pub fn load(&mut self, m: Model) {
        self.model = m;
        self.resolve = None;
    }
}
