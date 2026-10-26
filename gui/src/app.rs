//! The window: header bar, navigation sidebar, the pages, the problem bar, the actions that menus,
//! shortcuts and the command launcher share, and the timer that moves background work into the UI.
//! All decisions live in `model`, `build`, `media` and `hostcfg`; this only shows them.
use crate::dialogs;
use crate::model::{Area, Model};
use crate::pages;
use crate::state::State;
use crate::ui;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

pub use crate::model::{page_of, Page};

pub const PAGES: &[(Page, &str, &str, Option<Area>)] = &[
    (Page::System, "system", "System", Some(Area::System)),
    (Page::Disk, "disk", "Disk", Some(Area::Disk)),
    (
        Page::Packages,
        "packages",
        "Packages",
        Some(Area::Packages),
    ),
    (Page::Users, "users", "Users", Some(Area::Users)),
    (Page::Build, "build", "Build & drivers", Some(Area::Build)),
];

pub const ALL_PAGES: &[(Page, &str, &str)] = &[
    (Page::System, "system", "System"),
    (Page::Disk, "disk", "Disk"),
    (Page::Packages, "packages", "Packages"),
    (Page::Users, "users", "Users"),
    (Page::Build, "build", "Build & drivers"),
    (Page::Toml, "toml", "TOML source"),
    (Page::Iso, "iso", "Build ISO"),
    (Page::Help, "help", "Help & Reference"),
];

#[must_use]
pub fn icon_of(page: Page) -> &'static str {
    match page {
        Page::System => "computer-symbolic",
        Page::Disk => "drive-harddisk-symbolic",
        Page::Packages => "system-software-install-symbolic",
        Page::Users => "avatar-default-symbolic",
        Page::Build => "applications-engineering-symbolic",
        Page::Toml => "text-x-generic-symbolic",
        Page::Iso => "media-optical-symbolic",
        Page::Help => "help-browser-symbolic",
    }
}

pub struct App {
    pub st: RefCell<State>,
    pub win: gtk::ApplicationWindow,
    stack: gtk::Stack,
    nav: gtk::ListBox,
    nav_marks: Vec<gtk::Label>,
    title: gtk::Label,
    status: gtk::Label,
    problem: gtk::Label,
    problem_btn: gtk::Button,
    toml_btn: gtk::ToggleButton,
    boxes: HashMap<Page, gtk::Box>,
    refreshers: RefCell<HashMap<Page, Rc<dyn Fn()>>>,
    current: Cell<Page>,
    previous: Cell<Page>,
    /// The live widgets of the Build ISO page (progress, log), while that page exists.
    pub iso_live: RefCell<Option<Rc<pages::iso::Live>>>,
}

fn open_autoload_dialog(app: &Rc<App>) {
    let dialog = gtk::Window::builder()
        .title("Autoload System Configuration")
        .transient_for(&app.win)
        .modal(true)
        .default_width(460)
        .default_height(380)
        .resizable(false)
        .build();

    let root = ui::vbox(12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    root.append(&ui::strong("Selective Hardware & System Import"));
    ui::hint(
        &root,
        "Select components from this running computer to import into the configuration:",
    );

    let chk_id = ui::check("Identity & Localization (hostname, timezone, locale, keymap)", true, |_| {});
    let chk_users = ui::check("Host Users & Groups (from /etc/passwd and /etc/group)", true, |_| {});
    let chk_hw = ui::check("Network & Hardware Credentials (Wi-Fi, Bluetooth, SSH host keys, zram)", true, |_| {});
    let chk_services = ui::check("Enabled Systemd Services (package and custom units)", true, |_| {});
    let chk_packages = ui::check("Installed Pacman Packages (core and extra)", false, |_| {});
    let chk_aur = ui::check("Installed AUR Packages (autoload and review)", false, |_| {});

    root.append(&chk_id);
    root.append(&chk_users);
    root.append(&chk_hw);
    root.append(&chk_services);
    root.append(&chk_packages);
    root.append(&chk_aur);

    let btn_row = ui::hbox(8);
    btn_row.set_halign(gtk::Align::End);
    let cancel = ui::button("Cancel", {
        let d = dialog.clone();
        move || d.close()
    });
    btn_row.append(&cancel);

    let a = app.clone();
    let d = dialog.clone();
    let apply = ui::primary("Apply Selected", move || {
        let do_id = chk_id.is_active();
        let do_users = chk_users.is_active();
        let do_hw = chk_hw.is_active();
        let do_services = chk_services.is_active();
        let do_packages = chk_packages.is_active();
        let do_aur = chk_aur.is_active();

        if do_id {
            let sys = hostcfg::system::discover_host_system();
            a.edit(|m| {
                m.cfg.hostname = sys.hostname;
                m.cfg.timezone = sys.timezone;
                m.cfg.locale = sys.locale;
                m.cfg.keymap = sys.keymap;
            });
        }
        if do_users {
            let host_users = hostcfg::users::discover_host_users();
            a.edit(|m| {
                for u in host_users {
                    if let Some(existing) = m.cfg.users.iter_mut().find(|e| e.name == u.name) {
                        existing.groups = u.groups;
                        existing.shell = u.shell;
                        if let Some(hash) = u.password_hash {
                            existing.password_hash = hash;
                        }
                    } else if let Some(hash) = u.password_hash {
                        m.cfg.users.push(config::User {
                            name: u.name,
                            password_hash: hash,
                            groups: u.groups,
                            shell: u.shell,
                            dotfiles: None,
                            ..Default::default()
                        });
                    }
                }
            });
        }
        if do_services {
            let discovered = hostcfg::services::discover_local_services();
            a.edit(|m| {
                for s in discovered.system_package_services {
                    if !m.cfg.services.contains(&s) {
                        m.cfg.services.push(s);
                    }
                }
                for cs in discovered.custom_system_services {
                    if !m.cfg.custom_services.iter().any(|e| e.name == cs.name && e.user == cs.user) {
                        m.cfg.custom_services.push(cs);
                    }
                }
            });
        }
        if do_packages {
            a.st.borrow_mut().official.start_import();
        }
        if do_aur {
            let (mirror, existing) = {
                let st = a.st.borrow();
                (
                    st.model.cfg.mirrors.first().cloned().unwrap_or_else(|| "https://geo.mirror.pkgbuild.com/$repo/os/$arch".into()),
                    st.model.cfg.aur.clone(),
                )
            };
            let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("archstaller-gui");
            a.st.borrow_mut().aur.start_autoload(existing, mirror, cache);
        }
        if do_hw {
            crate::pages::users::open_shadow_auth_dialog(&a);
        }
        a.set_status("Autoload configuration applied.");
        a.rebuild_all();
        d.close();
    });
    btn_row.append(&apply);
    root.append(&btn_row);

    dialog.set_child(Some(&root));
    dialog.present();
}

impl App {
    pub fn new(gapp: &gtk::Application) -> Rc<App> {
        // A default size that fits the screen: laptops are smaller than 1180x780.
        let (mut w, mut h) = (880, 680);
        if let Some(m) = gtk::gdk::Display::default()
            .and_then(|d| d.monitors().item(0))
            .and_then(|m| m.downcast::<gtk::gdk::Monitor>().ok())
        {
            let g = m.geometry();
            w = w.min(g.width() * 85 / 100);
            h = h.min(g.height() * 85 / 100);
        }
        let win = gtk::ApplicationWindow::builder()
            .application(gapp)
            .title("Archstaller")
            .default_width(w)
            .default_height(h)
            .build();
        win.set_size_request(380, 400);

        // Header bar: file and tools menus, the sidebar toggle, the document name.
        let header = gtk::HeaderBar::new();
        let title = gtk::Label::builder()
            .label("untitled")
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .width_chars(8)
            .selectable(true)
            .build();
        header.set_title_widget(Some(&title));
        let sidebar_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .active(true)
            .tooltip_text("Show or hide the sidebar")
            .build();
        sidebar_toggle
            .update_property(&[gtk::accessible::Property::Label("Show or hide the sidebar")]);
        header.pack_start(&sidebar_toggle);

        let autoload_btn = gtk::Button::builder()
            .label("Autoload")
            .icon_name("system-search-symbolic")
            .tooltip_text("Selective hardware, user, network, and config import from this system")
            .build();
        header.pack_start(&autoload_btn);

        let menu = gio::Menu::new();
        let file = gio::Menu::new();
        file.append(Some("New"), Some("win.new"));
        file.append(Some("Open…"), Some("win.open"));
        file.append(Some("Save"), Some("win.save"));
        file.append(Some("Save as…"), Some("win.save-as"));
        menu.append_section(None, &file);
        let tools = gio::Menu::new();
        tools.append(Some("Command launcher…"), Some("win.launcher"));
        tools.append(Some("Copy config as TOML"), Some("win.copy-toml"));
        tools.append(Some("Copy ISO SHA-256"), Some("win.copy-sha"));
        menu.append_section(None, &tools);
        let presets = gio::Menu::new();
        let preset_list = crate::build::find_root()
            .map(|r| crate::model::presets(&r))
            .unwrap_or_default();
        for p in &preset_list {
            let item = gio::MenuItem::new(Some(&p.name), None);
            item.set_action_and_target_value(Some("win.preset"), Some(&p.name.to_variant()));
            presets.append_item(&item);
        }
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Menu")
            .build();
        header.pack_end(&menu_btn);

        let help_btn = gtk::Button::builder()
            .icon_name("help-browser-symbolic")
            .tooltip_text("Help & Reference")
            .build();
        header.pack_end(&help_btn);

        let build_iso_btn = gtk::Button::builder()
            .label("Build ISO")
            .icon_name("media-optical-symbolic")
            .css_classes(["suggested-action"])
            .tooltip_text("Build bootable ISO and write to media")
            .build();
        header.pack_end(&build_iso_btn);

        let toml_btn = gtk::ToggleButton::builder()
            .label("Edit TOML")
            .icon_name("document-edit-symbolic")
            .tooltip_text("Toggle TOML source editor mode")
            .build();
        header.pack_end(&toml_btn);

        let preset_btn = gtk::MenuButton::builder().label("From preset").menu_model(&presets).sensitive(!preset_list.is_empty()).tooltip_text(if preset_list.is_empty() { "No presets found (install the package, run from a checkout or set ARCHSTALLER_ROOT)" } else { "Start a new, unsaved config from one of the repository's presets" }).build();
        header.pack_end(&preset_btn);

        win.set_titlebar(Some(&header));

        // Sidebar.
        let nav = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar", "sidebar-list"])
            .build();
        let mut nav_marks = Vec::new();
        for (page, _, name, _) in PAGES {
            let r = ui::hbox(8);
            let icon = gtk::Image::from_icon_name(icon_of(*page));
            icon.set_pixel_size(16);
            icon.add_css_class("dim-label");
            let l = gtk::Label::builder()
                .label(*name)
                .xalign(0.0)
                .hexpand(true)
                .build();
            let mark = gtk::Label::builder().label("").build();
            mark.add_css_class("nav-bad");
            r.append(&icon);
            r.append(&l);
            r.append(&mark);
            nav.append(&r);
            nav_marks.push(mark);
        }
        nav.set_header_func(|row, before| {
            if before.is_none() && row.index() == 0 {
                let header = gtk::Label::builder()
                    .label("CONFIGURATION")
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .margin_top(8)
                    .margin_bottom(4)
                    .margin_start(10)
                    .build();
                let f = header.attributes().unwrap_or_default();
                f.insert(gtk::pango::AttrFontDesc::new(&gtk::pango::FontDescription::from_string("bold 8")));
                header.set_attributes(Some(&f));
                row.set_header(Some(&header));
            } else {
                row.set_header(None::<&gtk::Widget>);
            }
        });
        let side = gtk::ScrolledWindow::builder()
            .child(&nav)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(190)
            .vexpand(true)
            .build();

        // Pages.
        // Not homogeneous: the window may be as narrow as the visible page, not the widest page.
        let stack = gtk::Stack::builder()
            .hexpand(true)
            .vexpand(true)
            .hhomogeneous(false)
            .vhomogeneous(false)
            .transition_type(gtk::StackTransitionType::None)
            .build();
        let mut boxes = HashMap::new();
        for (page, id, _) in ALL_PAGES {
            let (scroll, content) = ui::page_shell();
            stack.add_named(&scroll, Some(id));
            boxes.insert(*page, content);
        }

        // Bottom bar: validation summary and status.
        let bar = ui::hbox(10);
        bar.add_css_class("problem-bar");
        let problem = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(25)
            .width_chars(8)
            .selectable(true)
            .build();
        let problem_btn = gtk::Button::with_label("Fix issue");
        let status = gtk::Label::builder()
            .xalign(1.0)
            .css_classes(["hint"])
            .ellipsize(gtk::pango::EllipsizeMode::Start)
            .max_width_chars(25)
            .width_chars(8)
            .selectable(true)
            .build();
        bar.append(&problem);
        bar.append(&problem_btn);
        bar.append(&status);

        let body = ui::hbox(0);
        let sep = gtk::Separator::new(gtk::Orientation::Vertical);
        body.append(&side);
        body.append(&sep);
        body.append(&stack);
        let root = ui::vbox(0);
        root.append(&body);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&bar);
        win.set_child(Some(&root));

        let app = Rc::new(App {
            st: RefCell::new(State::new()),
            win,
            stack,
            nav,
            nav_marks,
            title,
            status,
            problem,
            problem_btn,
            toml_btn: toml_btn.clone(),
            boxes,
            refreshers: RefCell::new(HashMap::new()),
            current: Cell::new(Page::System),
            previous: Cell::new(Page::System),
            iso_live: RefCell::new(None),
        });
        {
            let side = side.clone();
            let sep = sep.clone();
            sidebar_toggle.connect_toggled(move |t| {
                side.set_visible(t.is_active());
                sep.set_visible(t.is_active());
            });
        }
        // A narrow window hides the sidebar (the toggle brings it back); a wide one shows it again.
        {
            let (toggle, last) = (sidebar_toggle.clone(), Cell::new(None::<bool>));
            app.win.add_tick_callback(move |w, _| {
                let narrow = w.width() < 760;
                if last.get() != Some(narrow) {
                    if last.get().is_some() || narrow {
                        toggle.set_active(!narrow);
                    }
                    last.set(Some(narrow));
                }
                glib::ControlFlow::Continue
            });
        }
        {
            let a = app.clone();
            app.nav.connect_row_selected(move |_, row| {
                if let Some(r) = row {
                    if let Ok(idx) = usize::try_from(r.index()) {
                        if let Some((page, ..)) = PAGES.get(idx) {
                            a.show_no_select(*page);
                        }
                    }
                }
            });
        }
        {
            let a = app.clone();
            app.problem_btn.connect_clicked(move |_| {
                let first = a.st.borrow().model.problems().into_iter().next();
                if let Some(p) = first {
                    a.show(page_of(p.area));
                }
            });
        }
        {
            let a = app.clone();
            autoload_btn.connect_clicked(move |_| {
                open_autoload_dialog(&a);
            });
        }
        {
            let a = app.clone();
            help_btn.connect_clicked(move |_| {
                a.show(Page::Help);
            });
        }
        {
            let a = app.clone();
            build_iso_btn.connect_clicked(move |_| {
                a.show(Page::Iso);
            });
        }
        {
            let a = app.clone();
            toml_btn.connect_toggled(move |btn| {
                let active = btn.is_active();
                if active && a.current() != Page::Toml {
                    a.show(Page::Toml);
                } else if !active && a.current() == Page::Toml {
                    if a.st.borrow().model.raw.is_some() {
                        match a.st.borrow_mut().model.leave_source() {
                            Ok(()) => {
                                a.rebuild_all();
                                a.show(a.previous.get());
                            }
                            Err(e) => {
                                a.set_status(format!("Cannot return to forms: {e}"));
                                btn.set_active(true);
                            }
                        }
                    } else {
                        a.show(a.previous.get());
                    }
                }
            });
        }
        app.install_actions(gapp);
        app.rebuild_all();
        app.show(Page::System);
        {
            let a = app.clone();
            glib::timeout_add_local(Duration::from_millis(100), move || {
                a.tick();
                glib::ControlFlow::Continue
            });
        }
        {
            let a = app.clone();
            app.win.connect_close_request(move |_| {
                if a.st.borrow().model.dirty {
                    let a2 = a.clone();
                    glib::spawn_future_local(async move {
                        if dialogs::confirm(
                            a2.win.upcast_ref(),
                            "Discard the unsaved changes?",
                            "This config has changes that were not saved.",
                            "Discard and close",
                        )
                        .await
                        {
                            a2.st.borrow_mut().model.dirty = false;
                            a2.win.close();
                        }
                    });
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        app
    }

    // ------------------------------------------------------------------ navigation and pages

    pub fn show(self: &Rc<Self>, page: Page) {
        if let Some(i) = PAGES.iter().position(|p| p.0 == page) {
            if let Some(row) = self.nav.row_at_index(i as i32) {
                self.nav.select_row(Some(&row));
                return;
            }
        } else {
            self.nav.select_row(None::<&gtk::ListBoxRow>);
        }
        self.show_no_select(page);
    }

    fn show_no_select(self: &Rc<Self>, page: Page) {
        if PAGES.iter().any(|p| p.0 == page) {
            self.previous.set(page);
        }
        self.current.set(page);
        self.toml_btn.set_active(page == Page::Toml);
        // The read-only source view shows what the forms say now.
        if page == Page::Toml && self.st.borrow().model.raw.is_none() {
            self.rebuild(Page::Toml);
        }
        if let Some((_, id, ..)) = ALL_PAGES.iter().find(|p| p.0 == page) {
            self.stack.set_visible_child_name(id);
        }
        if page == Page::Iso {
            self.st.borrow_mut().media.last_scan = None;
            // The summary shows the document as it is now; the live parts are rebuilt from the state.
            self.rebuild(Page::Iso);
        }
    }

    pub fn current(&self) -> Page {
        self.current.get()
    }

    pub fn register_refresher(&self, page: Page, f: Rc<dyn Fn()>) {
        self.refreshers.borrow_mut().insert(page, f);
    }

    /// Builds (or rebuilds) one page from the current document.
    /// [`App::rebuild`] for a change the user made on the page itself: the view stays where it was instead of
    /// jumping to the top. The position is restored once the new widgets have been laid out.
    pub fn rebuild_keep_scroll(self: &Rc<Self>, page: Page) {
        let adj = self
            .boxes
            .get(&page)
            .and_then(|b| b.ancestor(gtk::ScrolledWindow::static_type()))
            .and_downcast::<gtk::ScrolledWindow>()
            .map(|w| w.vadjustment());
        let pos = adj.as_ref().map(|a| a.value());
        self.rebuild(page);
        if let (Some(adj), Some(pos)) = (adj, pos) {
            glib::idle_add_local_once(move || adj.set_value(pos));
        }
    }

    pub fn rebuild(self: &Rc<Self>, page: Page) {
        let Some(content) = self.boxes.get(&page) else {
            return;
        };
        ui::clear(content);
        self.refreshers.borrow_mut().remove(&page);
        let raw = self.st.borrow().model.raw.is_some();
        if raw && !matches!(page, Page::Toml | Page::Iso | Page::Help) {
            ui::heading(
                content,
                ALL_PAGES
                    .iter()
                    .find(|p| p.0 == page)
                    .map(|p| p.2)
                    .unwrap_or(""),
                "",
            );
            let c = ui::card(content, None);
            ui::note(
                &c,
                ui::Kind::Warn,
                "This config is being edited as source text.",
            );
            ui::hint(&c, "The form pages work on a config the GUI writes. Open the TOML source page: leave source mode there to come back to the forms (a file the GUI did not write is replaced by form output only after you confirm).");
            let a = self.clone();
            c.append(&ui::button("Open the TOML source page", move || {
                a.show(Page::Toml)
            }));
            return;
        }
        match page {
            Page::System => pages::system::build(self, content),
            Page::Disk => pages::disk::build(self, content),
            Page::Packages => pages::packages::build(self, content),
            Page::Users => pages::users::build(self, content),
            Page::Build => pages::build_page::build(self, content),
            Page::Toml => pages::toml::build(self, content),
            Page::Iso => pages::iso::build(self, content),
            Page::Help => pages::help::build(self, content),
        }
    }

    /// Runs the page's refresher (the part of the page that shows background work), if it has one.
    pub fn refresh_page(&self, page: Page) {
        let f = self.refreshers.borrow().get(&page).cloned();
        if let Some(f) = f {
            f();
        }
    }

    pub fn rebuild_all(self: &Rc<Self>) {
        for (page, ..) in ALL_PAGES {
            self.rebuild(*page);
        }
        self.refresh_chrome();
    }

    // ------------------------------------------------------------------ the document

    /// A change made by a form control: marks the document dirty and refreshes what depends on it.
    pub fn edit(&self, f: impl FnOnce(&mut Model)) {
        {
            let mut st = self.st.borrow_mut();
            f(&mut st.model);
            st.model.dirty = true;
        }
        self.refresh_chrome();
    }

    pub fn set_status(&self, msg: impl Into<String>) {
        let msg = msg.into();
        self.status.set_text(&msg);
        self.status.set_tooltip_text(Some(&msg));
        self.st.borrow_mut().status = msg;
    }

    /// Title, validation summary and the navigation marks.
    pub fn refresh_chrome(&self) {
        let (name, dirty, problems) = {
            let st = self.st.borrow();
            let name = st
                .model
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "untitled".into());
            (name, st.model.dirty, st.model.problems())
        };
        let shown = format!("{name}{}", if dirty { " •" } else { "" });
        self.title.set_text(&shown);
        self.win.set_title(Some(&format!("{shown} — Archstaller")));
        for (i, (_, _, _, area)) in PAGES.iter().enumerate() {
            let bad = area.is_some_and(|a| problems.iter().any(|p| p.area == a));
            if let Some(mark) = self.nav_marks.get(i) {
                mark.set_text(if bad { "●" } else { "" });
            }
        }
        self.problem.remove_css_class("note-bad");
        self.problem.remove_css_class("note-ok");
        match problems.first() {
            Some(p) => {
                self.problem.set_text(&format!("Issue: {}", p.message));
                self.problem.add_css_class("note-bad");
                self.problem_btn.set_visible(true);
            }
            None => {
                self.problem.set_text("Valid configuration");
                self.problem.add_css_class("note-ok");
                self.problem_btn.set_visible(false);
            }
        }
        self.status.set_text(&self.st.borrow().status);
    }

    /// Asks before a replacement of the document throws unsaved work away.
    pub async fn may_discard(self: &Rc<Self>) -> bool {
        if !self.st.borrow().model.dirty {
            return true;
        }
        dialogs::confirm(
            self.win.upcast_ref(),
            "Discard the unsaved changes?",
            "This config has changes that were not saved.",
            "Discard",
        )
        .await
    }

    pub fn load(self: &Rc<Self>, m: Model, status: String) {
        self.st.borrow_mut().load(m);
        self.set_status(status);
        self.rebuild_all();
    }

    pub fn new_doc(self: &Rc<Self>) {
        let a = self.clone();
        glib::spawn_future_local(async move {
            if a.may_discard().await {
                a.load(Model::starter(), "New config".into());
            }
        });
    }

    pub fn open_doc(self: &Rc<Self>) {
        let a = self.clone();
        glib::spawn_future_local(async move {
            if !a.may_discard().await {
                return;
            }
            let Some(p) = dialogs::open_file(
                a.win.upcast_ref(),
                "Open a TOML config",
                Some(("TOML config", "*.toml")),
            )
            .await
            else {
                return;
            };
            match Model::open(&p) {
                Ok(m) => a.load(m, format!("Opened {}", p.display())),
                Err(e) => a.set_status(e),
            }
        });
    }

    pub fn preset_doc(self: &Rc<Self>, name: String) {
        let a = self.clone();
        glib::spawn_future_local(async move {
            if !a.may_discard().await {
                return;
            }
            let path =
                a.st.borrow()
                    .presets
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.path.clone());
            let Some(path) = path else {
                a.set_status(format!("no preset named {name}"));
                return;
            };
            match Model::from_preset(&path) {
                Ok(m) => {
                    a.load(m, format!("New config from preset {name} (unsaved)"));
                    if name == "reinstall" {
                        crate::pages::users::open_shadow_auth_dialog(&a);
                    }
                    // Presets erase the largest disk: say so before the user builds anything from it.
                    a.show(Page::Disk);
                    if a.st.borrow().model.cfg.disk.auto_largest {
                        dialogs::notice(a.win.upcast_ref(), "This preset erases the largest disk", "A machine that boots an ISO built from it loses its largest disk without being asked. Change the disk choice on the Disk page if that is not what you want.");
                    }
                }
                Err(e) => a.set_status(e),
            }
        });
    }

    /// Saves to the known path, else asks for one. `true` when the file was written.
    pub async fn save_async(self: &Rc<Self>) -> bool {
        let path = self.st.borrow().model.path.clone();
        let path = match path {
            Some(p) => p,
            None => match dialogs::save_file(
                self.win.upcast_ref(),
                "Save the config",
                "archstaller.toml",
                Some(("TOML config", "*.toml")),
            )
            .await
            {
                Some(p) => p,
                None => return false,
            },
        };
        let r = self.st.borrow_mut().model.save(&path);
        match r {
            Ok(()) => {
                self.set_status(format!("Saved {}", path.display()));
                self.refresh_chrome();
                true
            }
            Err(e) => {
                self.set_status(e);
                false
            }
        }
    }

    pub fn save(self: &Rc<Self>) {
        let a = self.clone();
        glib::spawn_future_local(async move {
            a.save_async().await;
        });
    }

    pub fn save_as(self: &Rc<Self>) {
        let a = self.clone();
        glib::spawn_future_local(async move {
            let Some(p) = dialogs::save_file(
                a.win.upcast_ref(),
                "Save the config as",
                "archstaller.toml",
                Some(("TOML config", "*.toml")),
            )
            .await
            else {
                return;
            };
            let r = a.st.borrow_mut().model.save(&p);
            match r {
                Ok(()) => a.set_status(format!("Saved {}", p.display())),
                Err(e) => a.set_status(e),
            }
            a.refresh_chrome();
        });
    }

    pub fn copy_text(&self, text: &str, what: &str) {
        self.win.clipboard().set_text(text);
        self.set_status(format!("{what} copied to the clipboard"));
    }

    // ------------------------------------------------------------------ actions

    fn install_actions(self: &Rc<Self>, gapp: &gtk::Application) {
        let simple = |name: &str, a: &Rc<App>, f: fn(&Rc<App>)| {
            let act = gio::SimpleAction::new(name, None);
            let a = a.clone();
            act.connect_activate(move |_, _| f(&a));
            self.win.add_action(&act);
        };
        simple("new", self, |a| a.new_doc());
        simple("open", self, |a| a.open_doc());
        simple("save", self, |a| a.save());
        simple("save-as", self, |a| a.save_as());
        simple("copy-toml", self, |a| {
            let t = a.st.borrow().model.toml();
            a.copy_text(&t, "TOML");
        });
        simple("copy-sha", self, |a| {
            let sha = a.st.borrow().build.iso.as_ref().map(|i| i.2.clone());
            match sha {
                Some(s) => a.copy_text(&s, "SHA-256"),
                None => a.set_status("no ISO built yet"),
            }
        });
        simple("launcher", self, crate::launcher::open);
        simple("build-iso", self, |a| {
            a.show(Page::Iso);
            pages::iso::start_from_action(a);
        });
        let preset = gio::SimpleAction::new("preset", Some(glib::VariantTy::STRING));
        {
            let a = self.clone();
            preset.connect_activate(move |_, v| {
                if let Some(name) = v.and_then(|v| v.get::<String>()) {
                    a.preset_doc(name);
                }
            });
        }
        self.win.add_action(&preset);
        let goto = gio::SimpleAction::new("goto", Some(glib::VariantTy::STRING));
        {
            let a = self.clone();
            goto.connect_activate(move |_, v| {
                if let Some(id) = v.and_then(|v| v.get::<String>()) {
                    if let Some((page, ..)) = ALL_PAGES.iter().find(|p| p.1 == id) {
                        a.show(*page);
                    }
                }
            });
        }
        self.win.add_action(&goto);
        gapp.set_accels_for_action("win.new", &["<Primary>n"]);
        gapp.set_accels_for_action("win.open", &["<Primary>o"]);
        gapp.set_accels_for_action("win.save", &["<Primary>s"]);
        gapp.set_accels_for_action("win.save-as", &["<Primary><Shift>s"]);
        gapp.set_accels_for_action("win.launcher", &["<Primary>k"]);
        // "/" opens the launcher too, but only when no text field has the focus.
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let a = self.clone();
            key.connect_key_pressed(move |_, k, _, mods| {
                if k == gtk::gdk::Key::slash && mods.is_empty() {
                    let typing = gtk::prelude::GtkWindowExt::focus(&a.win).is_some_and(|f| {
                        f.is::<gtk::Editable>()
                            || f.is::<gtk::TextView>()
                            || f.ancestor(gtk::TextView::static_type()).is_some()
                            || f.ancestor(gtk::Text::static_type()).is_some()
                    });
                    if !typing {
                        crate::launcher::open(&a);
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            });
        }
        self.win.add_controller(key);
    }

    // ------------------------------------------------------------------ background work

    fn tick(self: &Rc<Self>) {
        let mut changed = false;
        // An import wrote `packages`: the list editor shows the old text until its page is rebuilt.
        let mut rebuild_packages = false;
        {
            let mut st = self.st.borrow_mut();
            if let Some(rx) = &st.resolve_rx {
                match rx.try_recv() {
                    Ok(r) => {
                        st.resolve = Some(r);
                        st.resolve_rx = None;
                        changed = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        st.resolve_rx = None;
                        st.resolve = Some(Err("Dependency resolver disconnected unexpectedly".into()));
                        changed = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
            let st = &mut *st;
            let polled = st.official.poll(&mut st.model.cfg);
            if polled.packages_changed {
                st.model.dirty = true;
                rebuild_packages = true;
            }
            changed |= polled.changed;
            let was_busy = st.aur.busy();
            if let Some(p) = st.aur.poll() {
                // Pinned: the search that led here is done with.
                st.aur.results = None;
                st.aur.search.clear();
                st.aur_focus_search = true;
                st.model.cfg.aur = p.entries;
                st.model.dirty = true;
                changed = true;
            }
            if was_busy && !st.aur.busy() {
                changed = true;
            }
            // A search for an exact package name goes straight to that package's review.
            if st.aur_auto_review && st.aur.search_rx.is_none() {
                st.aur_auto_review = false;
                let term = st.aur.search.trim().to_lowercase();
                let hit = match &st.aur.results {
                    Some(Ok(list)) => list.iter().find(|i| i.name.to_lowercase() == term).cloned(),
                    _ => None,
                };
                if let Some(info) = hit {
                    if st.aur.review.is_none() && !st.aur.busy() {
                        st.aur.start_review(&info);
                        changed = true;
                    }
                }
            }
        }
        if rebuild_packages {
            self.rebuild_keep_scroll(Page::Packages);
        }
        if pages::iso::pump(self) {
            changed = true;
        }
        if changed {
            self.st.borrow_mut().ui_dirty = true;
        }
        let dirty = std::mem::take(&mut self.st.borrow_mut().ui_dirty);
        if dirty {
            self.refresh_chrome();
            let f = self.refreshers.borrow().get(&self.current()).cloned();
            if let Some(f) = f {
                f();
            }
        }
        // Keep the live ISO page current while it is visible (progress bars, volume list).
        if self.current() == Page::Iso {
            pages::iso::live_refresh(self);
        }
    }

    pub fn window(&self) -> gtk::Window {
        self.win.clone().upcast()
    }
}
