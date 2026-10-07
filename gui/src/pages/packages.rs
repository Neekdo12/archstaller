use crate::app::{App, Page};
use crate::aur;
use crate::dialogs;
use crate::ui::{self, Kind};
use gtk::glib;
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
    let (used, trust, services, packages) = {
        let st = app.st.borrow();
        (!st.model.cfg.aur.is_empty(), st.aur.trust_ack, st.model.cfg.services.clone(), st.model.cfg.packages.clone())
    };
    if !used && !trust {
        ui::hint(&c, "Packages from the Arch User Repository are not part of the official repositories.");
        let a = app.clone();
        c.append(&ui::button("Use AUR packages…", move || {
            let a = a.clone();
            glib::spawn_future_local(async move {
                if dialogs::confirm(a.win.upcast_ref(), "Use AUR packages?", "AUR packages are built on the installed machine during its first boot, from recipes nobody at Arch reviewed. The build runs as an unprivileged user but it still executes the recipe's code there, and the result is not signed by Arch. You review and pin every recipe yourself. Continue?", "Yes, continue").await {
                    {
                        let mut st = a.st.borrow_mut();
                        st.aur.trust_ack = true;
                        st.aur.message = None;
                    }
                    a.refresh_page(Page::Packages);
                }
            });
        }));
        return;
    }
    ui::note(&c, Kind::Warn, "AUR recipes are not reviewed by Arch. The installed system builds each one at its first boot, as an unprivileged user, from the exact recipe you review here; the build still runs code from that recipe on that machine, and the result is not signed by Arch.");
    if !aur::has_network_service(&services) {
        ui::note(&c, Kind::Bad, "The build needs a network on the installed system: enable NetworkManager.service (or another network service).");
        let a = app.clone();
        c.append(&ui::button("Add NetworkManager", move || {
            a.edit(|m| {
                if !m.cfg.packages.iter().any(|p| p == "networkmanager") {
                    m.cfg.packages.push("networkmanager".into());
                }
                m.cfg.services.push("NetworkManager.service".into());
            });
            // The list editors above show the old lists; rebuild the whole page.
            a.rebuild(Page::Packages);
            a.rebuild(Page::Services);
        }));
    }
    let _ = packages;

    // Pinned packages.
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
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        r.append(&spacer);
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
        let a = app.clone();
        ui::row(&c, "Enable units", &ui::suggest(&p.services.join(", "), units.clone(), true, move |t| {
            let v: Vec<String> = t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            a.edit(move |m| {
                if let Some(x) = m.cfg.aur.get_mut(i) {
                    x.services = v;
                }
            });
        }));
    }

    // Search.
    let (search, busy, results, message) = {
        let st = app.st.borrow();
        let results = st.aur.results.clone();
        (st.aur.search.clone(), st.aur.busy(), results, st.aur.message.clone())
    };
    let sr = ui::hbox(8);
    let entry = ui::entry(&search, {
        let a = app.clone();
        move |t| a.st.borrow_mut().aur.search = t.to_string()
    });
    let go = ui::button("Search", || {});
    go.set_sensitive(!busy && search.trim().len() >= 2);
    {
        let (a, e) = (app.clone(), entry.clone());
        let start = Rc::new(move || {
            if e.text().trim().len() >= 2 && !a.st.borrow().aur.busy() {
                a.st.borrow_mut().aur.start_search();
                a.refresh_page(Page::Packages);
            }
        });
        let s = start.clone();
        go.connect_clicked(move |_| s());
        entry.connect_activate(move |_| start());
    }
    sr.append(&entry);
    sr.append(&go);
    ui::row(&c, "Search the AUR", &sr);
    if busy {
        let st = app.st.borrow();
        let what = if st.aur.pin_rx.is_some() { "pinning (re-checking pins, resolving dependencies)…".to_string() } else if let Some(n) = &st.aur.fetching { format!("fetching the recipe of {n}…") } else { "searching…".to_string() };
        c.append(&ui::spinner_row(&what).0);
    }
    match results {
        Some(Err(e)) => {
            ui::note(&c, Kind::Bad, &e);
        }
        Some(Ok(list)) if list.is_empty() => {
            ui::hint(&c, "Nothing found.");
        }
        Some(Ok(list)) => {
            for info in list {
                let r = ui::hbox(8);
                let rv = ui::button("Review", {
                    let (a, info) = (app.clone(), info.clone());
                    move || {
                        a.st.borrow_mut().aur.start_review(&info);
                        a.refresh_page(Page::Packages);
                    }
                });
                rv.set_sensitive(!busy);
                r.append(&rv);
                r.append(&ui::strong(&info.name));
                r.append(&ui::dim(&info.version));
                r.append(&ui::dim(&format!("{} votes", info.votes)));
                match &info.maintainer {
                    Some(m) => r.append(&ui::dim(&format!("by {m}"))),
                    None => r.append(&ui::chip("orphaned", "chip-warn")),
                }
                if info.out_of_date.is_some() {
                    r.append(&ui::chip("out of date", "chip-warn"));
                }
                c.append(&r);
                if let Some(desc) = &info.description {
                    ui::hint(&c, desc);
                }
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

/// The recipe viewer: what was fetched, what looks risky, and the acknowledgements before the pin.
fn review(app: &Rc<App>, c: &gtk::Box, busy: bool) {
    let Some((rev, file, ack_rev, ack_vcs)) = ({
        let st = app.st.borrow();
        st.aur.review.clone().map(|r| (r, st.aur.file.clone(), st.aur.ack_reviewed, st.aur.ack_vcs))
    }) else {
        return;
    };
    c.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    c.append(&ui::strong(&format!("Review: {} {} at commit {}", rev.name, rev.srcinfo.version(), &rev.commit[..12])));
    ui::hint(c, &format!("pkgbase {}   tree digest {}", rev.pkgbase, &rev.tree_sha256[..16]));
    for w in &rev.warnings {
        ui::note(c, Kind::Warn, w);
    }
    let deps = rev.srcinfo.depends(&rev.name);
    if !deps.is_empty() {
        ui::hint(c, &format!("depends: {}", deps.join(", ")));
    }
    if !rev.srcinfo.makedepends().is_empty() {
        ui::hint(c, &format!("makedepends: {}", rev.srcinfo.makedepends().join(", ")));
    }
    let risky = aurbuild::plan::risky_lines(&rev.files);
    let files = ui::hbox(6);
    let mut group: Option<gtk::ToggleButton> = None;
    for f in rev.files.keys() {
        let b = gtk::ToggleButton::with_label(f);
        b.set_active(*f == file);
        if let Some(g) = &group {
            b.set_group(Some(g));
        } else {
            group = Some(b.clone());
        }
        let (a, name) = (app.clone(), f.clone());
        b.connect_toggled(move |b| {
            if b.is_active() && a.st.borrow().aur.file != name {
                a.st.borrow_mut().aur.file = name.clone();
                a.refresh_page(Page::Packages);
            }
        });
        files.append(&b);
    }
    c.append(&gtk::ScrolledWindow::builder().child(&files).vscrollbar_policy(gtk::PolicyType::Never).build());
    if let Some(data) = rev.files.get(&file) {
        let view = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).left_margin(8).top_margin(6).bottom_margin(6).wrap_mode(gtk::WrapMode::None).build();
        let buf = view.buffer();
        let warn_tag = buf.create_tag(Some("risky"), &[("background", &"rgba(245,194,17,0.25)")]).unwrap();
        let text = String::from_utf8_lossy(data).into_owned();
        for (n, line) in text.lines().enumerate() {
            let mut end = buf.end_iter();
            let start = end.offset();
            buf.insert(&mut end, &format!("{:>4}  {line}\n", n + 1));
            if risky.iter().any(|(f, l, _)| *f == file && *l == n + 1) {
                let (s, e) = (buf.iter_at_offset(start), buf.end_iter());
                buf.apply_tag(&warn_tag, &s, &e);
            }
        }
        c.append(&gtk::ScrolledWindow::builder().min_content_height(280).max_content_height(320).has_frame(true).child(&view).build());
    }
    let a = app.clone();
    c.append(&ui::check("I reviewed this recipe at this commit and accept that it will be built and installed", ack_rev, move |on| {
        a.st.borrow_mut().aur.ack_reviewed = on;
        a.refresh_page(Page::Packages);
    }));
    if rev.vcs {
        let a = app.clone();
        c.append(&ui::check("I understand its VCS sources are not pinned by the commit: the build fetches whatever they point to then", ack_vcs, move |on| {
            a.st.borrow_mut().aur.ack_vcs = on;
            a.refresh_page(Page::Packages);
        }));
    }
    let r = ui::hbox(8);
    let ok = ack_rev && (!rev.vcs || ack_vcs) && !busy;
    let pin = ui::primary("Pin and add", {
        let (a, rev) = (app.clone(), rev.clone());
        move || {
            let (mirror, existing) = {
                let st = a.st.borrow();
                (st.model.cfg.mirrors.first().cloned().unwrap_or_default(), st.model.cfg.aur.clone())
            };
            let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("archstaler-gui");
            a.st.borrow_mut().aur.start_pin(rev.clone(), existing, mirror, cache);
            a.refresh_page(Page::Packages);
        }
    });
    pin.set_sensitive(ok);
    r.append(&pin);
    let a = app.clone();
    r.append(&ui::button("Close", move || {
        a.st.borrow_mut().aur.review = None;
        a.refresh_page(Page::Packages);
    }));
    c.append(&r);
}
