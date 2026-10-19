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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Page {
    System,
    Disk,
    Packages,
    Users,
    Build,
    // Action views (opened from header bar)
    Toml,
    Iso,
    Help,
}

pub const PAGES: &[(Page, &str, &str, Option<Area>)] = &[
    (Page::System, "system", "System", Some(Area::System)),
    (Page::Disk, "disk", "Disk", Some(Area::Disk)),
    (Page::Packages, "packages", "Packages", Some(Area::Packages)),
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
        Page::Toml => "document-edit-symbolic",
        Page::Iso => "media-optical-symbolic",
        Page::Help => "help-browser-symbolic",
    }
}

pub fn page_of(area: Area) -> Page {
    match area {
        Area::System | Area::Scripts | Area::Services | Area::Other => Page::System,
        Area::Disk => Page::Disk,
        Area::Packages => Page::Packages,
        Area::Users => Page::Users,
        Area::Build => Page::Build,
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
    boxes: HashMap<Page, gtk::Box>,
    refreshers: RefCell<HashMap<Page, Rc<dyn Fn()>>>,
    current: Cell<Page>,
    previous_page: Cell<Page>,
    toml_toggle: gtk::ToggleButton,
    /// The live widgets of the Build ISO page (progress, log), while that page exists.
    pub iso_live: RefCell<Option<Rc<pages::iso::Live>>>,
}

fn open_autoload_dialog(app: &Rc<App>) {
    let dialog = gtk::Window::builder()
        .title("Autoload System Configuration")
        .transient_for(&app.win)
        .modal(true)
        .default_width(460)
        .default_height(360)
        .build();

    let root = ui::vbox(12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    ui::heading(
        &root,
        "Autoload Hardware & Configuration",
        "Select the components you wish to import from this running system:",
    );

    let check_sys = ui::check_button("Identity & Localization (hostname, timezone, locale, keymap)");
    check_sys.set_active(true);
    root.append(&check_sys);

    let check_net = ui::check_button("Network & Drivers (wired/wireless NIC drivers)");
    check_net.set_active(true);
    root.append(&check_net);

    let check_users = ui::check_button("Users & Accounts (user accounts from /etc/passwd and groups)");
    check_users.set_active(true);
    root.append(&check_users);

    let check_services = ui::check_button("Enabled Services (systemd services and custom units)");
    check_services.set_active(true);
    root.append(&check_services);

    let check_pkgs = ui::check_button("Installed Packages (native packages explicitly installed by pacman)");
    check_pkgs.set_active(true);
    root.append(&check_pkgs);

    let btn_row = ui::hbox(8);
    btn_row.set_halign(gtk::Align::End);
    let cancel = ui::button("Cancel", {
        let d = dialog.clone();
        move || d.close()
    });
    btn_row.append(&cancel);

    let import = ui::primary("Import Selected", {
        let a = app.clone();
        let d = dialog.clone();
        move || {
            let do_sys = check_sys.is_active();
            let do_net = check_net.is_active();
            let do_users = check_users.is_active();
            let do_services = check_services.is_active();
            let do_pkgs = check_pkgs.is_active();

            a.edit(|m| {
                if do_sys {
                    let sys = hostcfg::system::discover_host_system();
                    m.cfg.hostname = sys.hostname;
                    m.cfg.timezone = sys.timezone;
                    m.cfg.locale = sys.locale;
                    m.cfg.keymap = sys.keymap;
                }
                if do_net {
                    let net = hostcfg::network::discover_host_network(None);
                    if let Some(pkg) = net.package {
                        if !m.cfg.packages.contains(&pkg) {
                            m.cfg.packages.push(pkg);
                        }
                    }
                    if let Some(srv) = net.service {
                        if !m.cfg.services.contains(&srv) {
                            m.cfg.services.push(srv);
                        }
                    }
                    for cf in net.connection_files {
                        if !m.cfg.system_files.iter().any(|f| f.path == cf.path) {
                            m.cfg.system_files.push(cf);
                        }
                    }
                }
                if do_users {
                    let host_users = hostcfg::users::discover_host_users();
                    for u in host_users {
                        if let Some(existing) = m.cfg.users.iter_mut().find(|e| e.name == u.name) {
                            existing.groups = u.groups;
                            existing.shell = u.shell;
                        } else {
                            m.cfg.users.push(config::User {
                                name: u.name,
                                password_hash: u.password_hash.unwrap_or_else(|| {
                                    "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.".into()
                                }),
                                groups: u.groups,
                                shell: u.shell,
                                dotfiles: None,
                            });
                        }
                    }
                }
                if do_services {
                    let disc = hostcfg::services::discover_local_services();
                    for s in disc.system_package_services {
                        if !m.cfg.services.contains(&s) {
                            m.cfg.services.push(s);
                        }
                    }
                }
                if do_pkgs {
                    if let Ok(out) = std::process::Command::new("pacman").arg("-Qqen").output() {
                        if out.status.success() {
                            let text = String::from_utf8_lossy(&out.stdout);
                            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                                if !m.cfg.packages.iter().any(|p| p == line) {
                                    m.cfg.packages.push(line.to_string());
                                }
                            }
                        }
                    }
                }
            });
            a.set_status("Autoloaded selected system components.");
            a.rebuild_all();
            d.close();
        }
    });
    btn_row.append(&import);
    root.append(&btn_row);

    dialog.set_child(Some(&root));
    dialog.present();
}

impl App {
    pub fn new(gapp: &gtk::Application) -> Rc<App> {
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
        let preset_btn = gtk::MenuButton::builder().label("From preset").menu_model(&presets).sensitive(!preset_list.is_empty()).tooltip_text(if preset_list.is_empty() { "No presets found (install the package, run from a checkout or set ARCHSTALLER_ROOT)" } else { "Start a new, unsaved config from one of the repository's presets" }).build();
        header.pack_end(&preset_btn);

        let toml_toggle = gtk::ToggleButton::builder()
            .label("Edit TOML")
            .icon_name("document-edit-symbolic")
            .tooltip_text("Switch between graphical form view and direct TOML source editor")
            .build();
        header.pack_end(&toml_toggle);

        let flash_btn = gtk::Button::builder()
            .label("Build & Flash")
            .icon_name("media-optical-symbolic")
            .css_classes(["suggested-action"])
            .tooltip_text("Build bootable ISO image or flash directly to USB / drive")
            .build();
        header.pack_end(&flash_btn);

        let help_btn = gtk::Button::builder()
            .icon_name("help-browser-symbolic")
            .tooltip_text("Help & reference documentation")
            .build();
        header.pack_end(&help_btn);

        let autoload_btn = gtk::Button::builder()
            .label("Autoload")
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Import hardware, localization, users, and network settings from this computer")
            .build();
        header.pack_start(&autoload_btn);

        win.set_titlebar(Some(&header));

        // Sidebar
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

        // Stack with all pages
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

        // Bottom bar
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
        let problem_btn = ui::button("Go to page", || {});
        problem_btn.set_visible(false);
        let status = gtk::Label::builder()
            .xalign(1.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(30)
            .width_chars(10)
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
            boxes,
            refreshers: RefCell::new(HashMap::new()),
            current: Cell::new(Page::System),
            previous_page: Cell::new(Page::System),
            toml_toggle,
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
            flash_btn.connect_clicked(move |_| {
                a.show(Page::Iso);
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
            app.toml_toggle.connect_toggled(move |btn| {
                if btn.is_active() {
                    a.show(Page::Toml);
                } else if a.current() == Page::Toml {
                    let r = a.st.borrow_mut().model.leave_source();
                    match r {
                        Ok(()) => {
                            let prev = a.previous_page.get();
                            a.show(prev);
                            a.rebuild_all();
                        }
                        Err(e) => {
                            btn.set_active(true);
                            a.set_status(format!("Cannot return to forms: {e}"));
                        }
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
        }
        self.nav.unselect_all();
        self.show_no_select(page);
    }

    fn show_no_select(self: &Rc<Self>, page: Page) {
        let prev = self.current.get();
        if prev != Page::Toml && PAGES.iter().any(|p| p.0 == prev) {
            self.previous_page.set(prev);
        }
        self.current.set(page);
        self.toml_toggle.set_active(page == Page::Toml);

        if page == Page::Toml && self.st.borrow().model.raw.is_none() {
            self.rebuild(Page::Toml);
        }
        if let Some((_, id, ..)) = ALL_PAGES.iter().find(|p| p.0 == page) {
            self.stack.set_visible_child_name(id);
        }
        if page == Page::Iso {
            self.st.borrow_mut().media.last_scan = None;
            self.rebuild(Page::Iso);
        }
        if page == Page::Help {
            self.rebuild(Page::Help);
        }
    }

    pub fn current(&self) -> Page {
        self.current.get()
    }

    pub fn register_refresher(&self, page: Page, f: Rc<dyn Fn()>) {
        self.refreshers.borrow_mut().insert(page, f);
    }

    pub fn rebuild_keep_scroll(self: &Rc<Self>, page: Page) {
        let adj = self
            .boxes
            .get(&page)
            .and_then(|b| b.parent())
            .and_then(|p| p.downcast::<gtk::ScrolledWindow>().ok())
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
        for (i, (page, ..)) in PAGES.iter().enumerate() {
            let bad = problems.iter().any(|p| page_of(p.area) == *page);
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
                    a.show(Page::Disk);
                    if a.st.borrow().model.cfg.disk.auto_largest {
                        dialogs::notice(a.win.upcast_ref(), "This preset erases the largest disk", "A machine that boots an ISO built from it loses its largest disk without being asked. Change the disk choice on the Disk page if that is not what you want.");
                    }
                }
                Err(e) => a.set_status(e),
            }
        });
    }

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
        match self.st.borrow_mut().model.save(&path) {
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
            let Some(path) = dialogs::save_file(
                a.win.upcast_ref(),
                "Save the config as",
                "archstaller.toml",
                Some(("TOML config", "*.toml")),
            )
            .await
            else {
                return;
            };
            match a.st.borrow_mut().model.save(&path) {
                Ok(()) => {
                    a.set_status(format!("Saved {}", path.display()));
                    a.refresh_chrome();
                }
                Err(e) => a.set_status(e),
            }
        });
    }

    pub fn copy_text(&self, text: &str, label: &str) {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(text);
            self.set_status(format!("Copied {label} to clipboard"));
        }
    }

    fn install_actions(self: &Rc<Self>, gapp: &gtk::Application) {
        let act = |name: &str, f: Rc<dyn Fn()>| {
            let a = gio::SimpleAction::new(name, None);
            a.connect_activate(move |_, _| f());
            a
        };
        let a = self.clone();
        self.win
            .add_action(&act("new", Rc::new(move || a.new_doc())));
        let a = self.clone();
        self.win
            .add_action(&act("open", Rc::new(move || a.open_doc())));
        let a = self.clone();
        self.win.add_action(&act("save", Rc::new(move || a.save())));
        let a = self.clone();
        self.win
            .add_action(&act("save-as", Rc::new(move || a.save_as())));
        let a = self.clone();
        self.win
            .add_action(&act("launcher", Rc::new(move || crate::launcher::open(&a))));
        let a = self.clone();
        self.win.add_action(&act(
            "copy-toml",
            Rc::new(move || {
                let t = a.st.borrow().model.toml();
                a.copy_text(&t, "TOML");
            }),
        ));
        let a = self.clone();
        self.win.add_action(&act(
            "copy-sha",
            Rc::new(move || {
                let sha = a.st.borrow().build.iso.as_ref().map(|i| i.2.clone());
                match sha {
                    Some(s) => a.copy_text(&s, "SHA-256"),
                    None => a.set_status("no ISO built yet"),
                }
            }),
        ));
        let a = self.clone();
        let preset_action =
            gio::SimpleAction::new("preset", Some(glib::VariantTy::STRING));
        preset_action.connect_activate(move |_, var| {
            if let Some(s) = var.and_then(|v| v.str()) {
                a.preset_doc(s.to_string());
            }
        });
        self.win.add_action(&preset_action);

        gapp.set_accels_for_action("win.new", &["<Control>n"]);
        gapp.set_accels_for_action("win.open", &["<Control>o"]);
        gapp.set_accels_for_action("win.save", &["<Control>s"]);
        gapp.set_accels_for_action("win.save-as", &["<Control><Shift>s"]);
        gapp.set_accels_for_action("win.launcher", &["<Control>k"]);
    }

    pub fn tick(self: &Rc<Self>) {
        let mut changed = false;
        let mut rebuild_packages = false;
        {
            let mut st = self.st.borrow_mut();
            if let Some(r) = st.resolve_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                st.resolve_rx = None;
                st.resolve = Some(r);
                changed = true;
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
        if self.current() == Page::Iso {
            pages::iso::live_refresh(self);
        }
    }

    pub fn window(&self) -> gtk::Window {
        self.win.clone().upcast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_maps_to_5_config_pages() {
        assert_eq!(page_of(Area::System), Page::System);
        assert_eq!(page_of(Area::Scripts), Page::System);
        assert_eq!(page_of(Area::Services), Page::System);
        assert_eq!(page_of(Area::Other), Page::System);
        assert_eq!(page_of(Area::Disk), Page::Disk);
        assert_eq!(page_of(Area::Packages), Page::Packages);
        assert_eq!(page_of(Area::Users), Page::Users);
        assert_eq!(page_of(Area::Build), Page::Build);
    }
}
