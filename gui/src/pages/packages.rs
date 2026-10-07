use crate::app::{App, Page};
use crate::aur;
use crate::ui::{self, Kind};
use gtk::prelude::*;
use std::rc::Rc;
use std::sync::mpsc::channel;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let cfg = app.st.borrow().model.cfg.clone();
    ui::heading(content, "Mirrors & packages", "Where packages come from and which ones to install (official core and extra, plus pinned AUR packages).");

    let c = ui::card(content, Some("Mirrors"));
    let a = app.clone();
    c.append(&ui::lines_editor(&cfg.mirrors, 80, move |v| a.edit(move |m| m.cfg.mirrors = v)));
    ui::hint(&c, "One URL per line; $repo and $arch are substituted. The first mirror is tried first.");

    let c = ui::card(content, Some("Packages"));
    let a = app.clone();
    c.append(&ui::lines_editor(&cfg.packages, 230, move |v| a.edit(move |m| m.cfg.packages = v)));
    ui::hint(&c, "One package or group per line. Names are suggestions: the resolver decides.");

    // Everything below changes while background work runs, so it is rebuilt on its own.
    let dynamic = ui::vbox(12);
    content.append(&dynamic);
    let refresh: Rc<dyn Fn()> = {
        let (a, d) = (app.clone(), dynamic.clone());
        Rc::new(move || fill(&a, &d))
    };
    app.register_refresher(Page::Packages, refresh.clone());
    fill(app, &dynamic);
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
            let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("archstaler-gui");
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
            let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("archstaler-gui");
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
