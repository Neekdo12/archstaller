use crate::app::{App, Page};
use crate::aur;
use crate::official::{self, Match};
use crate::ui::{self, Kind};
use gtk::prelude::*;
use std::rc::Rc;
use std::sync::mpsc::channel;

/// The parts of the official-package UI that change while the page is open: redrawn by `render_official`
/// on every refresh and, for the results, on every keystroke. `list` is the package list editor, so that
/// adding a package edits its text the way typing would (one undo step, the caret and focus stay).
#[derive(Clone)]
struct OfficialParts {
    status: gtk::Box,
    results: gtk::Box,
    total: gtk::Box,
    import: gtk::Box,
    list: gtk::TextView,
}

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let cfg = app.st.borrow().model.cfg.clone();
    ui::heading(content, "Mirrors & packages", "Where packages come from and which ones to install (official core and extra, plus pinned AUR packages).");

    let a = app.clone();
    let editor = ui::lines_editor(&cfg.packages, 230, move |v| a.edit(move |m| m.cfg.packages = v));
    let Some(list) = editor.child().and_downcast::<gtk::TextView>() else { return };
    let parts = OfficialParts { status: ui::vbox(6), results: ui::vbox(4), total: ui::hbox(8), import: ui::vbox(6), list };

    let c = ui::card(content, Some("Mirrors"));
    let (a, p) = (app.clone(), parts.clone());
    c.append(&ui::lines_editor(&cfg.mirrors, 80, move |v| {
        a.edit(move |m| m.cfg.mirrors = v);
        // The search says when its databases came from another mirror than the first one now.
        render_status(&a, &p);
    }));
    ui::hint(&c, "One URL per line; $repo and $arch are substituted. The first mirror is tried first.");

    search_card(app, content, &parts);

    let c = ui::card(content, Some("Packages"));
    c.append(&editor);
    c.append(&parts.total);
    ui::hint(&c, "One package or group per line. Names are suggestions: the resolver decides.");
    c.append(&parts.import);

    // Everything below changes while background work runs, so it is rebuilt on its own.
    let dynamic = ui::vbox(12);
    content.append(&dynamic);
    let refresh: Rc<dyn Fn()> = {
        let (a, d, p) = (app.clone(), dynamic.clone(), parts.clone());
        Rc::new(move || {
            render_official(&a, &p);
            fill(&a, &d);
        })
    };
    app.register_refresher(Page::Packages, refresh.clone());
    render_official(app, &parts);
    fill(app, &dynamic);
}

/// Starts loading core and extra from the first mirror unless that is done or under way.
fn ensure_loaded(app: &Rc<App>) {
    let mirror = {
        let st = app.st.borrow();
        if st.official.loading() || st.official.loaded().is_some() {
            return;
        }
        st.model.cfg.mirrors.first().cloned().unwrap_or_default()
    };
    reload(app, mirror);
}

fn reload(app: &Rc<App>, mirror: String) {
    let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("archstaller-gui");
    app.st.borrow_mut().official.start_load(mirror, cache);
    app.refresh_page(Page::Packages);
}

fn search_card(app: &Rc<App>, content: &gtk::Box, parts: &OfficialParts) {
    let c = ui::card(content, Some("Find official packages"));
    ui::hint(&c, "Searches core and extra on this computer: name, group and description, with a typo or two forgiven. The databases are downloaded once from the first mirror.");
    let query = app.st.borrow().official.query.clone();
    let entry = ui::entry(&query, {
        let (a, p) = (app.clone(), parts.clone());
        move |t| {
            a.st.borrow_mut().official.query = t.to_string();
            if !t.trim().is_empty() {
                ensure_loaded(&a);
            }
            render_results(&a, &p);
        }
    });
    entry.set_placeholder_text(Some("Package name, group, or words from the description"));
    c.append(&entry);
    c.append(&parts.status);
    c.append(&parts.results);
}

fn render_official(app: &Rc<App>, p: &OfficialParts) {
    render_status(app, p);
    render_results(app, p);
    render_total(app, p);
    render_import(app, p);
}

/// Where the databases stand: never an empty list without a reason.
fn render_status(app: &Rc<App>, p: &OfficialParts) {
    ui::clear(&p.status);
    let (loading, index, mirror) = {
        let st = app.st.borrow();
        (st.official.loading(), st.official.index.clone(), st.model.cfg.mirrors.first().cloned().unwrap_or_default())
    };
    let r = ui::hbox(8);
    if loading {
        r.append(&ui::spinner_row(&format!("loading core and extra from {mirror}…")).0);
    } else {
        match &index {
            None => {
                r.append(&ui::dim("The package databases are not loaded yet."));
                let a = app.clone();
                r.append(&ui::button("Load now", move || ensure_loaded(&a)));
            }
            Some(Err(e)) => {
                let l = ui::dim(&format!("Could not load the package databases: {e}"));
                l.add_css_class("note-bad");
                l.set_wrap(true);
                r.append(&l);
                let a = app.clone();
                r.append(&ui::button("Retry", move || {
                    let m = a.st.borrow().model.cfg.mirrors.first().cloned().unwrap_or_default();
                    reload(&a, m);
                }));
            }
            Some(Ok(idx)) => {
                r.append(&ui::dim(&format!("{} packages in core and extra", idx.items.len())));
                if idx.mirror != mirror {
                    r.append(&ui::chip("loaded from another mirror", "chip-warn"));
                    let (a, m) = (app.clone(), mirror.clone());
                    r.append(&ui::button("Reload", move || reload(&a, m.clone())));
                }
            }
        }
    }
    p.status.append(&r);
}

fn render_results(app: &Rc<App>, p: &OfficialParts) {
    ui::clear(&p.results);
    let (query, idx, packages) = {
        let st = app.st.borrow();
        (st.official.query.clone(), st.official.loaded(), st.model.cfg.packages.clone())
    };
    let q = query.trim();
    if q.is_empty() {
        return;
    }
    if q.chars().count() < official::MIN_QUERY {
        ui::hint(&p.results, "Type at least two characters.");
        return;
    }
    // Not loaded: the status line above says why.
    let Some(idx) = idx else { return };
    let hits = official::search(&idx.items, q, official::LIMIT);
    if hits.is_empty() {
        ui::hint(&p.results, "Nothing in core or extra matches. It may be an AUR package: search for it in the AUR card below.");
        return;
    }
    for h in hits {
        let c = &idx.items[h.item];
        let row = ui::vbox(2);
        row.add_css_class("card-box");
        let top = ui::hbox(8);
        let name = ui::strong(&c.name);
        name.set_hexpand(false);
        top.append(&name);
        top.append(&ui::dim(&c.version));
        top.append(&ui::chip(&c.repo, "chip-accent"));
        if h.why == Match::Typo {
            top.append(&ui::chip("similar name", "chip-warn"));
        }
        if h.why == Match::Group {
            top.append(&ui::chip(&format!("group {}", c.groups.join(", ")), "chip-accent"));
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        top.append(&spacer);
        top.append(&ui::dim(&format!("{} download · {} installed", official::size(c.csize), official::size(c.isize))));
        if packages.contains(&c.name) {
            top.append(&ui::dim("already added"));
        } else {
            let (a, parts, name) = (app.clone(), p.clone(), c.name.clone());
            top.append(&ui::button("Add", move || {
                append_line(&parts.list, &name);
                a.set_status(format!("Added {name}"));
                render_results(&a, &parts);
            }));
        }
        row.append(&top);
        if !c.description.is_empty() {
            let d = ui::dim(&c.description);
            d.set_ellipsize(gtk::pango::EllipsizeMode::End);
            d.set_tooltip_text(Some(&c.description));
            row.append(&d);
        }
        p.results.append(&row);
    }
}

/// Adds `name` as a new line of the list editor; its change handler writes `packages`.
fn append_line(view: &gtk::TextView, name: &str) {
    let buf = view.buffer();
    let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
    let line = if text.is_empty() || text.ends_with('\n') { name.to_string() } else { format!("\n{name}") };
    buf.insert(&mut buf.end_iter(), &line);
}

/// What the list installs, by the installer's own resolver against the loaded databases.
fn render_total(app: &Rc<App>, p: &OfficialParts) {
    ui::clear(&p.total);
    let (loaded, loading, resolving, total) = {
        let st = app.st.borrow();
        (st.official.loaded().is_some(), st.official.loading(), st.official.resolving(), st.official.total.clone())
    };
    if !loaded {
        p.total.append(&ui::dim(if loading { "The total appears when the package databases are loaded." } else { "Load the package databases (search above) to see what this list installs." }));
        return;
    }
    match total {
        Some(Ok((n, bytes))) => {
            p.total.append(&ui::chip(&format!("{n} packages"), "chip-accent"));
            p.total.append(&ui::chip(&format!("{} to download", official::size(bytes)), "chip-accent"));
        }
        Some(Err(e)) => {
            let l = ui::dim(&format!("The resolver refuses this list: {e}"));
            l.add_css_class("note-bad");
            l.set_wrap(true);
            p.total.append(&l);
        }
        None => {}
    }
    if resolving {
        p.total.append(&ui::spinner_row("resolving…").0);
    }
}

fn render_import(app: &Rc<App>, p: &OfficialParts) {
    ui::clear(&p.import);
    let (importing, report) = {
        let st = app.st.borrow();
        (st.official.importing(), st.official.import.clone())
    };
    let r = ui::hbox(8);
    let a = app.clone();
    let b = ui::button("Import from this system", move || {
        a.st.borrow_mut().official.start_import();
        ensure_loaded(&a);
        a.refresh_page(Page::Packages);
    });
    b.set_tooltip_text(Some("Adds the packages this computer has explicitly installed from core and extra (pacman -Qqen). Only package names are read."));
    b.set_sensitive(!importing);
    r.append(&b);
    if importing {
        r.append(&ui::spinner_row("reading pacman's package list…").0);
    }
    p.import.append(&r);
    match report {
        Some(Ok(rep)) => {
            let msg = if rep.added.is_empty() { format!("Nothing to add: {} installed package(s) are already in the list.", rep.already) } else { format!("Added {} package(s); {} were already in the list.", rep.added.len(), rep.already) };
            ui::note(&p.import, Kind::Ok, &msg);
            if !rep.not_official.is_empty() {
                ui::note(&p.import, Kind::Warn, &format!("Not in core/extra, skipped: {}", rep.not_official.join(", ")));
            }
            if !rep.foreign.is_empty() {
                ui::hint(&p.import, &format!("Installed from outside the repositories (AUR or local builds), not added: {}. An AUR package needs its recipe reviewed and pinned in the AUR card below.", rep.foreign.join(", ")));
            }
        }
        Some(Err(e)) => {
            ui::note(&p.import, Kind::Bad, &format!("Import failed: {e}"));
        }
        None => {}
    }
}

fn fill(app: &Rc<App>, d: &gtk::Box) {
    ui::clear(d);
    resolution_card(app, d);
    aur_card(app, d);
}

fn resolution_card(app: &Rc<App>, d: &gtk::Box) {
    let c = ui::card(d, Some("Resolution"));
    let busy = app.st.borrow().resolve_rx.is_some();
    let r = ui::hbox(10);
    let go = ui::button("Resolve dependencies", {
        let a = app.clone();
        move || {
            let cfg = a.st.borrow().model.cfg.clone();
            let mirror = cfg.mirrors.first().cloned().unwrap_or_default();
            let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("archstaller-gui");
            let (tx, rx) = channel();
            a.st.borrow_mut().resolve_rx = Some(rx);
            std::thread::spawn(move || {
                let r = hostcfg::resolve::fetch_dbs(&mirror, &cache).and_then(|dbs| hostcfg::resolve::resolve(&cfg, &dbs)).map_err(|e| e.to_string());
                let _ = tx.send(r);
            });
            a.refresh_page(Page::Packages);
        }
    });
    go.set_sensitive(!busy);
    r.append(&go);
    if busy {
        r.append(&ui::spinner_row("fetching the package databases…").0);
    }
    c.append(&r);
    let st = app.st.borrow();
    match &st.resolve {
        Some(Err(e)) => {
            ui::note(&c, Kind::Bad, e);
        }
        Some(Ok(res)) => {
            let chips = ui::hbox(8);
            chips.append(&ui::chip(&format!("{} packages", res.packages.len()), "chip-accent"));
            chips.append(&ui::chip(&format!("{} MiB to download", res.download_bytes() >> 20), "chip-accent"));
            c.append(&chips);
            for a in &res.ambiguities {
                ui::note(&c, Kind::Warn, &format!("{} is provided by {}; chosen {} (set it in `providers` to pick another)", a.dep, a.candidates.join(", "), a.chosen));
            }
            let view = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).left_margin(8).top_margin(6).bottom_margin(6).build();
            let buf = view.buffer();
            let bold = buf.create_tag(Some("bold"), &[("weight", &700i32)]).unwrap();
            let width = res.packages.iter().map(|p| p.name.len()).max().unwrap_or(10).min(48);
            for p in &res.packages {
                let mut end = buf.end_iter();
                let start = end.offset();
                buf.insert(&mut end, &format!("{:<w$}  {:<24} {:<8} {} KiB\n", p.name, p.version, p.repo, p.csize >> 10, w = width));
                if p.explicit {
                    let (s, e) = (buf.iter_at_offset(start), buf.iter_at_offset(start + width as i32));
                    buf.apply_tag(&bold, &s, &e);
                }
            }
            c.append(&gtk::ScrolledWindow::builder().min_content_height(220).max_content_height(220).has_frame(true).child(&view).build());
            ui::hint(&c, "Bold: named in the list. The rest are dependencies.");
        }
        None => {
            ui::hint(&c, "Fetches the package databases from the first mirror and shows what would be installed.");
        }
    }
}

fn aur_card(app: &Rc<App>, d: &gtk::Box) {
    let c = ui::card(d, Some("AUR packages"));
    ui::hint(&c, "Type a package name. It is built on the installed machine, at its second boot, from the recipe you review. Not signed by Arch.");

    // Pinned packages: one compact row each.
    let pinned = app.st.borrow().model.cfg.aur.clone();
    let units = app.st.borrow().lists.services.clone();
    for (i, p) in pinned.iter().enumerate() {
        let r = ui::hbox(8);
        r.append(&ui::strong(&p.name));
        r.append(&ui::dim(&format!("@ {}", &p.commit[..p.commit.len().min(8)])));
        if p.as_dep {
            r.append(&ui::chip("dependency", "chip-accent"));
        }
        if p.vcs {
            r.append(&ui::chip("VCS, not pinned", "chip-warn"));
        }
        let a = app.clone();
        let units_entry = ui::suggest(&p.services.join(", "), units.clone(), true, move |t| {
            let v: Vec<String> = t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            a.edit(move |m| {
                if let Some(x) = m.cfg.aur.get_mut(i) {
                    x.services = v;
                }
            });
        });
        units_entry.set_placeholder_text(Some("units to enable"));
        units_entry.set_tooltip_text(Some("systemd units to enable once this package is installed, comma separated"));
        r.append(&units_entry);
        let a = app.clone();
        r.append(&ui::button("Remove", move || {
            {
                let mut st = a.st.borrow_mut();
                let gone = st.model.cfg.aur.remove(i);
                st.model.dirty = true;
                st.aur.message = Some(Ok(format!("Removed {}. Its AUR dependencies stay pinned; remove them too if nothing else needs them.", gone.name)));
            }
            a.refresh_chrome();
            a.refresh_page(Page::Packages);
        }));
        c.append(&r);
    }

    // Search: one row, with the busy state beside it.
    let (search, busy, results, message, fetching, pinning) = {
        let st = app.st.borrow();
        (st.aur.search.clone(), st.aur.busy(), st.aur.results.clone(), st.aur.message.clone(), st.aur.fetching.clone(), st.aur.pin_rx.is_some())
    };
    let sr = ui::hbox(8);
    let entry = ui::entry(&search, {
        let a = app.clone();
        move |t| a.st.borrow_mut().aur.search = t.to_string()
    });
    entry.set_placeholder_text(Some("Search the AUR by name"));
    // After a pin the box takes the focus (and the page scrolls to it) for the next package.
    if std::mem::take(&mut app.st.borrow_mut().aur_focus_search) {
        let e = entry.clone();
        gtk::glib::idle_add_local_once(move || {
            e.grab_focus();
        });
    }
    let go = ui::primary("Search", || {});
    go.set_sensitive(!busy && search.trim().len() >= 2);
    {
        let (a, e) = (app.clone(), entry.clone());
        let start = Rc::new(move || {
            if e.text().trim().len() >= 2 && !a.st.borrow().aur.busy() {
                {
                    let mut st = a.st.borrow_mut();
                    st.aur.start_search();
                    // An exact name goes straight to its review when the results arrive.
                    st.aur_auto_review = true;
                }
                a.refresh_page(Page::Packages);
            }
        });
        let s = start.clone();
        go.connect_clicked(move |_| s());
        entry.connect_activate(move |_| start());
    }
    sr.append(&entry);
    sr.append(&go);
    if busy {
        let what = if pinning { "pinning…".to_string() } else if let Some(n) = fetching { format!("fetching {n}…") } else { "searching…".to_string() };
        sr.append(&ui::spinner_row(&what).0);
    }
    c.append(&sr);
    match results {
        Some(Err(e)) => {
            ui::note(&c, Kind::Bad, &e);
        }
        Some(Ok(list)) if list.is_empty() => {
            ui::hint(&c, "Nothing found.");
        }
        Some(Ok(list)) => {
            for info in list {
                let row = ui::hbox(10);
                let name = ui::strong(&info.name);
                name.set_hexpand(false);
                row.append(&name);
                row.append(&ui::dim(&format!("{}  ·  {} votes", info.version, info.votes)));
                if info.maintainer.is_none() {
                    row.append(&ui::chip("orphaned", "chip-warn"));
                }
                if info.out_of_date.is_some() {
                    row.append(&ui::chip("out of date", "chip-warn"));
                }
                let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                spacer.set_hexpand(true);
                row.append(&spacer);
                row.append(&ui::dim("Review ›"));
                let btn = gtk::Button::builder().child(&row).css_classes(["flat"]).sensitive(!busy).build();
                if let Some(desc) = &info.description {
                    btn.set_tooltip_text(Some(desc));
                }
                let (a, info) = (app.clone(), info.clone());
                btn.connect_clicked(move |_| {
                    a.st.borrow_mut().aur.start_review(&info);
                    a.refresh_page(Page::Packages);
                });
                c.append(&btn);
            }
        }
        None => {}
    }
    review(app, &c, busy);
    match message {
        Some(Ok(m)) => {
            ui::note(&c, Kind::Ok, &m);
        }
        Some(Err(e)) => {
            ui::note(&c, Kind::Bad, &e);
        }
        None => {}
    }
}

/// The recipe viewer. The checks are folded away behind a count, so what needs attention is the recipe
/// itself (risky lines are highlighted) and one acknowledgement.
fn review(app: &Rc<App>, c: &gtk::Box, busy: bool) {
    let Some((rev, file, ack_rev, ack_vcs)) = ({
        let st = app.st.borrow();
        st.aur.review.clone().map(|r| (r, st.aur.file.clone(), st.aur.ack_reviewed, st.aur.ack_vcs))
    }) else {
        app.st.borrow_mut().aur_focused_commit = None;
        return;
    };
    let p = ui::vbox(8);
    p.add_css_class("card-box");
    c.append(&p);
    let head = ui::hbox(8);
    head.append(&ui::strong(&format!("Review {} {}", rev.name, rev.srcinfo.version())));
    head.append(&ui::dim(&format!("commit {}", &rev.commit[..12])));
    p.append(&head);

    // Checks, folded.
    let deps = rev.srcinfo.depends(&rev.name);
    let risky = aurbuild::plan::risky_lines(&rev.files);
    let n = rev.warnings.len();
    let label = if n == 0 { "Automatic checks: nothing to report".to_string() } else { format!("Automatic checks: {n} thing{} to look at", if n == 1 { "" } else { "s" }) };
    let exp = gtk::Expander::builder().label(label).build();
    let inner = ui::vbox(4);
    inner.set_margin_top(4);
    for w in &rev.warnings {
        let l = gtk::Label::builder().label(format!("• {w}")).xalign(0.0).wrap(true).css_classes(["note-warn"]).build();
        inner.append(&l);
    }
    if !deps.is_empty() {
        ui::hint(&inner, &format!("depends: {}", deps.join(", ")));
    }
    if !rev.srcinfo.makedepends().is_empty() {
        ui::hint(&inner, &format!("makedepends: {}", rev.srcinfo.makedepends().join(", ")));
    }
    ui::hint(&inner, &format!("pkgbase {}   tree digest {}", rev.pkgbase, &rev.tree_sha256[..16]));
    exp.set_child(Some(&inner));
    p.append(&exp);

    // Recipe: file picker and the text.
    let names: Vec<&str> = rev.files.keys().map(|s| s.as_str()).collect();
    if names.len() > 1 {
        let dd = gtk::DropDown::from_strings(&names);
        dd.set_selected(names.iter().position(|n| *n == file).unwrap_or(0) as u32);
        let a = app.clone();
        let list: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        dd.connect_selected_notify(move |d| {
            if let Some(name) = list.get(d.selected() as usize) {
                if a.st.borrow().aur.file != *name {
                    a.st.borrow_mut().aur.file = name.clone();
                    a.refresh_page(Page::Packages);
                }
            }
        });
        let r = ui::hbox(8);
        r.append(&ui::dim("File"));
        r.append(&dd);
        p.append(&r);
    }
    if let Some(data) = rev.files.get(&file) {
        let view = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).left_margin(8).top_margin(6).bottom_margin(6).wrap_mode(gtk::WrapMode::None).build();
        let buf = view.buffer();
        let warn_tag = buf.create_tag(Some("risky"), &[("background", &"rgba(245,194,17,0.25)")]).unwrap();
        let text = String::from_utf8_lossy(data).into_owned();
        let mut any_risky = false;
        for (n, line) in text.lines().enumerate() {
            let mut end = buf.end_iter();
            let start = end.offset();
            buf.insert(&mut end, &format!("{:>4}  {line}\n", n + 1));
            if risky.iter().any(|(f, l, _)| *f == file && *l == n + 1) {
                any_risky = true;
                let (s, e) = (buf.iter_at_offset(start), buf.end_iter());
                buf.apply_tag(&warn_tag, &s, &e);
            }
        }
        p.append(&gtk::ScrolledWindow::builder().min_content_height(240).max_content_height(300).has_frame(true).child(&view).build());
        if any_risky {
            ui::hint(&p, "Highlighted lines use commands worth a second look (curl, sudo, eval, ...).");
        }
    }

    // The click on the button is the acknowledgement. A recipe that builds from unpinned VCS sources needs a
    // second, explicit one.
    let mut vcs_box = None;
    if rev.vcs {
        let a = app.clone();
        let cb = ui::check("Its VCS sources are not pinned by the commit: the build fetches whatever they point to then", ack_vcs, move |on| {
            {
                let mut st = a.st.borrow_mut();
                st.aur.ack_vcs = on;
                // The page is rebuilt: let the focus move on to the button (or back to this box).
                st.aur_focused_commit = None;
            }
            a.refresh_page(Page::Packages);
        });
        p.append(&cb);
        vcs_box = Some(cb);
    }
    let _ = ack_rev;
    let r = ui::hbox(8);
    r.set_halign(gtk::Align::End);
    let a = app.clone();
    r.append(&ui::button("Cancel", move || {
        {
            let mut st = a.st.borrow_mut();
            st.aur.review = None;
            st.aur_focused_commit = None;
        }
        a.refresh_page(Page::Packages);
    }));
    let ok = (!rev.vcs || ack_vcs) && !busy;
    let pin = ui::primary(&format!("I reviewed it: add {}", rev.name), {
        let (a, rev) = (app.clone(), rev.clone());
        move || {
            // The build needs a network on the installed system: add NetworkManager when none is enabled.
            let services = a.st.borrow().model.cfg.services.clone();
            let mut note = None;
            if !aur::has_network_service(&services) {
                a.edit(|m| {
                    if !m.cfg.packages.iter().any(|p| p == "networkmanager") {
                        m.cfg.packages.push("networkmanager".into());
                    }
                    m.cfg.services.push("NetworkManager.service".into());
                });
                note = Some("NetworkManager was added: the AUR build needs a network on the installed system.");
            }
            let (mirror, existing) = {
                let st = a.st.borrow();
                (st.model.cfg.mirrors.first().cloned().unwrap_or_default(), st.model.cfg.aur.clone())
            };
            let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("archstaller-gui");
            {
                let mut st = a.st.borrow_mut();
                st.aur.ack_reviewed = true;
                st.aur.start_pin(rev.clone(), existing, mirror, cache);
            }
            if let Some(n) = note {
                a.set_status(n);
                // The list editors show the old lists: rebuild the pages that hold them.
                a.rebuild(Page::Services);
                a.rebuild(Page::Packages);
            } else {
                a.refresh_page(Page::Packages);
            }
        }
    });
    pin.set_sensitive(ok);
    r.append(&pin);
    p.append(&r);

    // A review that has just opened: scroll to its button and focus it, so that Enter adds the package. When
    // the recipe needs the VCS acknowledgement first, that checkbox gets the focus instead.
    let first_time = app.st.borrow().aur_focused_commit.as_deref() != Some(rev.commit.as_str());
    if first_time {
        app.st.borrow_mut().aur_focused_commit = Some(rev.commit.clone());
        let target: gtk::Widget = match (&vcs_box, ok) {
            (Some(cb), false) => cb.clone().upcast(),
            _ => pin.clone().upcast(),
        };
        // Focusing a widget inside the page's scrolled window scrolls it into view.
        gtk::glib::idle_add_local_once(move || {
            target.grab_focus();
        });
    }
}
