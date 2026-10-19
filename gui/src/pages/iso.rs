//! Build ISO: run `cargo xtask build` for the saved config, follow its progress, and put the result on a
//! mounted Ventoy volume as a file or, when no Ventoy drive is found and the user asks for it, write it over
//! a whole USB stick (`flash_dialog`). The work runs on threads; `pump` moves what they report into the
//! state and `live_refresh` shows it.
use crate::app::{App, Page};
use crate::build::{self, Build, Msg};
use crate::dialogs;
use crate::flash::{self, Candidate};
use crate::media::{self, FolderCopy, MediaTarget, Volume};
use crate::state::{BuildState, MediaMsg};
use crate::ui::{self, Kind};
use gtk::glib;
use gtk::prelude::*;
use hostcfg::progress::{Event, State as Stage};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;

/// Widgets that change while work runs.
pub struct Live {
    stages: gtk::Label,
    error: gtk::Label,
    done: gtk::Label,
    sha: gtk::Label,
    summary: gtk::Label,
    spinner: gtk::Spinner,
    cancel: gtk::Button,
    build_btn: gtk::Button,
    log: gtk::TextBuffer,
    log_view: gtk::TextView,
    media_progress: gtk::ProgressBar,
    media_cancel: gtk::Button,
    copy_btn: gtk::Button,
    /// Physical media action.
    write_btn: Option<gtk::Button>,
    media_result: gtk::Label,
    drive_note: gtk::Label,
    iso_label: gtk::Label,
}

fn file_stem(p: &std::path::Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archstaller".into())
}

fn label_of(v: &Volume) -> String {
    if v.label.is_empty() {
        v.name.clone()
    } else {
        v.label.clone()
    }
}

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    ui::heading(
        content,
        "Build ISO",
        "Build the installer image from this config, then put it on a stick.",
    );
    let (model_path, profile, tethering, drivers, scripts, auto, problems) = {
        let st = app.st.borrow();
        (
            st.model.path.clone(),
            st.model.effective_profile(),
            st.model.host.build.tethering,
            st.model.host.drivers().join(", "),
            hostcfg::scripts::summary(&st.model.cfg),
            st.model.cfg.disk.auto_largest,
            st.model.problems(),
        )
    };

    // Summary.
    let c = ui::card(content, Some("Summary"));
    let cfg_label = match &model_path {
        Some(p) => p.display().to_string(),
        None => "not saved yet; a build uses the saved file".to_string(),
    };
    ui::row(
        &c,
        "Config",
        &gtk::Label::builder()
            .label(cfg_label)
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .build(),
    );
    let chips = ui::hbox(8);
    chips.append(&ui::chip(&profile, "chip-accent"));
    if tethering {
        chips.append(&ui::chip("USB tethering", "chip-accent"));
    }
    ui::row(&c, "Profile", &chips);
    ui::row(
        &c,
        "Drivers",
        &gtk::Label::builder()
            .label(drivers)
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .build(),
    );
    for l in scripts {
        ui::row(
            &c,
            "Script",
            &gtk::Label::builder()
                .label(l)
                .xalign(0.0)
                .wrap(true)
                .selectable(true)
                .build(),
        );
    }
    if auto {
        ui::note(
            &c,
            Kind::Warn,
            "This ISO erases the largest disk of the machine it boots on, without asking.",
        );
    }
    if let Some(p) = problems.first() {
        let n = ui::note(
            &c,
            Kind::Bad,
            &format!("The config is invalid: {}", p.message),
        );
        n.set_selectable(true);
    }

    // Build card.
    let c = ui::card(content, Some("Build"));
    let (volumes, to_file, stick_pref) = {
        let st = app.st.borrow();
        (st.media.volumes.clone(), st.media.to_file, st.media.stick)
    };
    let vv: Vec<usize> = volumes
        .iter()
        .enumerate()
        .filter(|(_, v)| media::is_ventoy(v, &volumes))
        .map(|(i, _)| i)
        .collect();
    let stick_idx = stick_pref
        .filter(|i| vv.contains(i))
        .or(vv.first().copied());
    let to_stick = !to_file && stick_idx.is_some();
    if vv.is_empty() {
        ui::hint(&c, "No Ventoy drive found: the ISO is saved as a file. Plug one in and press Refresh drives, or flash a blank stick under Put it on a stick.");
    } else {
        let r_stick = ui::check_button(
            "Build straight onto the Ventoy drive (nothing is kept on this computer)",
        );
        let r_file = ui::check_button("Save the ISO as a file instead");
        r_file.set_group(Some(&r_stick));
        r_stick.set_active(to_stick);
        r_file.set_active(!to_stick);
        {
            let a = app.clone();
            r_stick.connect_toggled(move |b| {
                let want_file = !b.is_active();
                if a.st.borrow().media.to_file != want_file {
                    a.st.borrow_mut().media.to_file = want_file;
                    a.rebuild(Page::Iso);
                }
            });
        }
        c.append(&r_stick);
        if to_stick {
            let mut first: Option<gtk::CheckButton> = None;
            for &i in &vv {
                let Some(v) = volumes.get(i).cloned() else {
                    continue;
                };
                let r = ui::hbox(10);
                let place = if v.mounted {
                    v.mount.display().to_string()
                } else {
                    "not mounted".to_string()
                };
                let free = if v.mounted {
                    format!("{} GiB free", v.available >> 30)
                } else {
                    String::new()
                };
                let rb = ui::check_button(&format!("{}  {}  {}", label_of(&v), place, free));
                rb.set_margin_start(24);
                if let Some(f) = &first {
                    rb.set_group(Some(f));
                } else {
                    first = Some(rb.clone());
                }
                rb.set_active(stick_idx == Some(i));
                let a = app.clone();
                rb.connect_toggled(move |b| {
                    if b.is_active() {
                        a.st.borrow_mut().media.stick = Some(i);
                    }
                });
                r.append(&rb);
                if !v.mounted {
                    let busy = app.st.borrow().media.drive_busy.is_some();
                    let a = app.clone();
                    let name = v.name.clone();
                    let m = ui::button("Mount", move || run_drive(&a, name.clone(), true));
                    m.set_sensitive(!busy);
                    r.append(&m);
                }
                c.append(&r);
            }
        }
        c.append(&r_file);
    }
    let a = app.clone();
    c.append(&ui::button("Refresh drives", move || {
        {
            let mut st = a.st.borrow_mut();
            st.media.volumes = media::volumes();
            st.media.selected = None;
        }
        a.rebuild(Page::Iso);
    }));

    let run = ui::hbox(8);
    let can = problems.is_empty()
        && app.st.borrow().build.running.is_none()
        && (!to_stick || stick_idx.and_then(|i| volumes.get(i)).is_some_and(|v| v.mounted));
    let build_btn = ui::primary(
        if to_stick {
            "Build onto Ventoy"
        } else {
            "Save and build…"
        },
        {
            let a = app.clone();
            move || start_from_action(&a)
        },
    );
    build_btn.set_sensitive(can);
    run.append(&build_btn);
    let cancel = ui::button("Cancel", {
        let a = app.clone();
        move || {
            let mut st = a.st.borrow_mut();
            if let Some(b) = st.build.running.take() {
                b.cancel();
                // The log is otherwise only written when a build exits, which a cancelled one is no longer watched for.
                drop(std::fs::write(&b.log_path, st.build.log.join("\n")));
                st.build.error = Some(format!("Cancelled. Log kept at {}", b.log_path.display()));
                st.build.iso = None;
            }
        }
    });
    run.append(&cancel);
    let spinner = gtk::Spinner::new();
    run.append(&spinner);
    c.append(&run);
    let stages = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .use_markup(true)
        .selectable(true)
        .build();
    c.append(&stages);
    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(["note-bad"])
        .build();
    c.append(&error);
    let done = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(["note-ok"])
        .build();
    c.append(&done);
    let sha = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(["hint"])
        .build();
    c.append(&sha);
    let summary = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(["hint"])
        .build();
    c.append(&summary);
    let log_view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .left_margin(8)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    let log = log_view.buffer();
    let exp = gtk::Expander::builder().label("Build log").build();
    exp.set_child(Some(
        &gtk::ScrolledWindow::builder()
            .min_content_height(200)
            .max_content_height(240)
            .has_frame(true)
            .child(&log_view)
            .build(),
    ));
    c.append(&exp);

    // Put it on a stick.
    let c = ui::card(content, Some("Put it on a stick"));
    let iso_label = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .selectable(true)
        .build();
    let r = ui::hbox(8);
    r.append(&ui::button("Pick an ISO…", {
        let a = app.clone();
        move || {
            let a = a.clone();
            glib::spawn_future_local(async move {
                if let Some(p) = dialogs::open_file(
                    a.win.upcast_ref(),
                    "Pick an ISO",
                    Some(("ISO image", "*.iso")),
                )
                .await
                {
                    a.st.borrow_mut().media.iso_override = Some(p);
                }
            });
        }
    }));
    r.append(&iso_label);
    ui::row(&c, "ISO", &r);
    ui::hint(&c, "Copy to Ventoy adds the ISO as a file; nothing on the stick is formatted or repartitioned.");
    let head = ui::hbox(10);
    head.append(&ui::strong("Volumes"));
    head.append(&ui::button("Refresh", {
        let a = app.clone();
        move || {
            {
                let mut st = a.st.borrow_mut();
                st.media.volumes = media::volumes();
                st.media.selected = None;
            }
            a.rebuild(Page::Iso);
        }
    }));
    c.append(&head);
    let show_internal = app.st.borrow().media.show_internal;
    c.append(&ui::check("Show internal disks", show_internal, {
        let a = app.clone();
        move |on| {
            a.st.borrow_mut().media.show_internal = on;
            a.rebuild(Page::Iso);
        }
    }));
    let (selected, drive_busy, copying) = {
        let st = app.st.borrow();
        (
            st.media.selected,
            st.media.drive_busy.clone(),
            st.media.rx.is_some(),
        )
    };
    let mut group: Option<gtk::CheckButton> = None;
    for (i, v) in volumes.iter().enumerate() {
        if !v.removable && !show_internal {
            continue;
        }
        let ventoy = media::is_ventoy(v, &volumes);
        let r = ui::hbox(10);
        let name = label_of(v);
        let text = if v.mounted {
            format!("{name}  {}", v.mount.display())
        } else {
            format!("{name}  (not mounted)")
        };
        let rb = ui::check_button(&text);
        if let Some(g) = &group {
            rb.set_group(Some(g));
        } else {
            group = Some(rb.clone());
        }
        rb.set_sensitive(v.mounted);
        rb.set_active(selected == Some(i));
        {
            let (a, mount) = (app.clone(), v.mount.clone());
            rb.connect_toggled(move |b| {
                if b.is_active() {
                    let changed = a.st.borrow().media.selected != Some(i);
                    if changed {
                        {
                            let mut st = a.st.borrow_mut();
                            st.media.selected = Some(i);
                            st.media.dir = Some(mount.clone());
                        }
                        a.rebuild(Page::Iso);
                    }
                }
            });
        }
        r.append(&rb);
        if ventoy {
            r.append(&ui::chip("Ventoy", "chip-ok"));
        }
        let idle = drive_busy.is_none() && !copying;
        if !v.mounted {
            let (a, dev) = (app.clone(), v.name.clone());
            let b = ui::button("Mount", move || run_drive(&a, dev.clone(), true));
            b.set_sensitive(idle);
            r.append(&b);
        } else if v.removable {
            let (a, dev) = (app.clone(), v.name.clone());
            let b = ui::button("Sync & unmount", move || run_drive(&a, dev.clone(), false));
            b.set_sensitive(idle);
            r.append(&b);
        }
        c.append(&r);
        let sizes = if v.mounted {
            format!("{} GiB free of {}", v.available >> 30, v.total >> 30)
        } else {
            format!("{} GiB", v.total >> 30)
        };
        let h = ui::hint(&c, &format!("{} {}  {}", v.name, v.fs, sizes));
        h.set_margin_start(24);
    }
    let drive_note = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    c.append(&drive_note);
    if let Some(v) = selected.and_then(|i| volumes.get(i)) {
        if !media::is_ventoy(v, &volumes) {
            ui::note(&c, Kind::Warn, "Ventoy was not detected here (no ventoy folder or ventoy.json). The ISO is only copied into the folder you choose.");
        }
        let r = ui::hbox(8);
        let dir_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .selectable(true)
            .label(
                app.st
                    .borrow()
                    .media
                    .dir
                    .as_ref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default(),
            )
            .build();
        let (a, mount) = (app.clone(), v.mount.clone());
        r.append(&ui::button("Choose folder…", move || {
            let (a, mount) = (a.clone(), mount.clone());
            glib::spawn_future_local(async move {
                if let Some(d) =
                    dialogs::open_folder(a.win.upcast_ref(), "Folder on the stick", Some(&mount))
                        .await
                {
                    a.st.borrow_mut().media.dir = Some(d);
                    a.rebuild(Page::Iso);
                }
            });
        }));
        r.append(&dir_label);
        ui::row(&c, "Folder", &r);
    }
    // Always offered, never by default: an empty or unformatted stick has no volume above, so a raw
    // flash is the only way to use it.
    let write_btn = {
        c.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        c.append(&ui::strong("Write directly to physical media"));
        ui::hint(&c, "Write the installer directly to whole physical media: USB flash drives, optical discs (CD/DVD/Blu-ray), floppy diskettes, or Raspberry Pi SD cards.");
        let row = ui::hbox(8);
        row.set_margin_top(4);
        row.set_margin_bottom(4);

        let media_dropdown = gtk::DropDown::from_strings(&[
            "USB Flash Drive (x86_64 Hybrid ISO)",
            "Optical Disc (CD / DVD / Blu-ray)",
            "Floppy Diskette (1.44 MB)",
            "Raspberry Pi (AArch64 SD / USB)",
        ]);
        media_dropdown.set_selected(0);
        row.append(&media_dropdown);

        let btn = gtk::Button::with_label("Write to physical drive…");
        btn.add_css_class("destructive-action");
        btn.set_halign(gtk::Align::Start);
        let a = app.clone();
        let dd = media_dropdown.clone();
        btn.connect_clicked(move |_| {
            let kind = match dd.selected() {
                0 => flash::MediaKind::Usb,
                1 => flash::MediaKind::Optical,
                2 => flash::MediaKind::Floppy,
                _ => flash::MediaKind::RaspberryPi,
            };
            let iso = if matches!(kind, flash::MediaKind::Usb | flash::MediaKind::Optical) {
                a.st.borrow().media.iso_override.clone()
            } else {
                None
            };
            if iso.is_none() {
                if a.st.borrow().build.running.is_some() {
                    a.set_status("a build is already running");
                    return;
                }
                if !a.st.borrow().model.problems().is_empty() {
                    a.set_status("the config is invalid; see the problem bar");
                    return;
                }
            }
            flash_dialog(&a, iso, kind);
        });
        row.append(&btn);

        c.append(&row);
        Some(btn)
    };
    let rr = ui::hbox(8);
    let copy_btn = ui::primary("Copy to the selected volume", {
        let a = app.clone();
        move || {
            let (iso, vol, dir) = {
                let st = a.st.borrow();
                let iso = st
                    .media
                    .iso_override
                    .clone()
                    .or_else(|| st.build.iso.as_ref().map(|i| i.0.clone()));
                let vol = st
                    .media
                    .selected
                    .and_then(|i| st.media.volumes.get(i).cloned());
                let dir = st.media.dir.clone();
                (iso, vol, dir)
            };
            if let (Some(iso), Some(v)) = (iso, vol) {
                let dir = dir.unwrap_or_else(|| v.mount.clone());
                let name = iso
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                begin_copy(&a, iso, v, dir, name);
            }
        }
    });
    rr.append(&copy_btn);
    let media_cancel = ui::button("Cancel", {
        let a = app.clone();
        move || a.st.borrow().media.cancel.store(true, Ordering::Relaxed)
    });
    rr.append(&media_cancel);
    c.append(&rr);
    let media_progress = gtk::ProgressBar::builder().show_text(true).build();
    c.append(&media_progress);
    let media_result = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    c.append(&media_result);

    let live = Rc::new(Live {
        stages,
        error,
        done,
        sha,
        summary,
        spinner,
        cancel,
        build_btn,
        log,
        log_view,
        media_progress,
        media_cancel,
        copy_btn,
        write_btn,
        media_result,
        drive_note,
        iso_label,
    });
    // The log is rebuilt from the state for a page that is built mid-build.
    app.st.borrow_mut().build.log_shown = 0;
    *app.iso_live.borrow_mut() = Some(live);
    live_refresh(app);
}

/// Shows the state in the live widgets. Cheap; called on every tick while the page is visible.
pub fn live_refresh(app: &Rc<App>) {
    let Some(l) = app.iso_live.borrow().clone() else {
        return;
    };
    let mut st = app.st.borrow_mut();
    let running = st.build.running.is_some();
    l.spinner.set_spinning(running);
    l.spinner.set_visible(running);
    l.cancel.set_visible(running);
    let problems_empty = st.model.problems().is_empty();
    // A build may start only from a valid config with nothing running.
    let (volumes, to_file, stick_pref) =
        (st.media.volumes.clone(), st.media.to_file, st.media.stick);
    let vv: Vec<usize> = volumes
        .iter()
        .enumerate()
        .filter(|(_, v)| media::is_ventoy(v, &volumes))
        .map(|(i, _)| i)
        .collect();
    let stick_idx = stick_pref
        .filter(|i| vv.contains(i))
        .or(vv.first().copied());
    let to_stick = !to_file && stick_idx.is_some();
    l.build_btn.set_sensitive(
        problems_empty
            && !running
            && (!to_stick || stick_idx.and_then(|i| volumes.get(i)).is_some_and(|v| v.mounted)),
    );
    let marks: Vec<String> = st
        .build
        .stages
        .iter()
        .map(|(name, s)| {
            let (mark, color) = match s {
                Stage::Start => ("…", "#e5a50a"),
                Stage::Ok => ("ok", "#26a269"),
                Stage::Fail => ("failed", "#e01b24"),
            };
            format!(
                "<span foreground=\"{color}\">[{} {mark}]</span>",
                glib::markup_escape_text(name)
            )
        })
        .collect();
    l.stages.set_markup(&marks.join("  "));
    l.error.set_text(st.build.error.as_deref().unwrap_or(""));
    l.error.set_visible(st.build.error.is_some());
    match &st.build.iso {
        Some((p, size, sha)) => {
            l.done
                .set_text(&format!("Built {} ({} KiB)", p.display(), size >> 10));
            l.sha.set_text(&format!("sha256 {sha}"));
            l.summary
                .set_text(st.build.summary.as_deref().unwrap_or(""));
        }
        None => {
            l.done.set_text("");
            l.sha.set_text("");
            l.summary.set_text("");
        }
    }
    // New log lines only.
    let shown = st.build.log_shown;
    if shown < st.build.log.len() {
        let mut end = l.log.end_iter();
        for line in st.build.log.get(shown..).into_iter().flatten() {
            l.log.insert(&mut end, line);
            l.log.insert(&mut end, "\n");
        }
        st.build.log_shown = st.build.log.len();
        let mark = l.log.create_mark(None, &l.log.end_iter(), false);
        l.log_view.scroll_to_mark(&mark, 0.0, false, 0.0, 0.0);
        l.log.delete_mark(&mark);
    }
    let iso = st
        .media
        .iso_override
        .clone()
        .or_else(|| st.build.iso.as_ref().map(|i| i.0.clone()));
    l.iso_label.set_text(&match &iso {
        Some(p) => p.display().to_string(),
        None => "build one first, or pick one".to_string(),
    });
    let busy = st.media.rx.is_some();
    l.copy_btn
        .set_sensitive(iso.is_some() && st.media.selected.is_some() && !busy);
    if let Some(b) = &l.write_btn {
        b.set_sensitive(!busy && st.build.running.is_none());
    }
    l.media_cancel.set_visible(busy);
    match (st.media.progress, busy) {
        (Some((phase, done, total)), true) => {
            l.media_progress.set_visible(true);
            l.media_progress.set_fraction(if total == 0 {
                0.0
            } else {
                done as f64 / total as f64
            });
            l.media_progress.set_text(Some(phase.label()));
        }
        _ => l.media_progress.set_visible(false),
    }
    l.media_result.remove_css_class("note-ok");
    l.media_result.remove_css_class("note-bad");
    match &st.media.result {
        Some(Ok(m)) => {
            l.media_result.set_text(m);
            l.media_result.add_css_class("note-ok");
        }
        Some(Err(e)) => {
            l.media_result.set_text(&format!(
                "{}: {e}",
                if st.media.flashing {
                    "Not flashed"
                } else {
                    "Not copied"
                }
            ));
            l.media_result.add_css_class("note-bad");
        }
        None => l.media_result.set_text(""),
    }
    l.drive_note.remove_css_class("note-ok");
    l.drive_note.remove_css_class("note-bad");
    match (&st.media.drive_busy, &st.media.drive_msg) {
        (Some(d), _) => l.drive_note.set_text(&format!("Working on {d} …")),
        (None, Some(Ok(m))) => {
            l.drive_note.set_text(m);
            l.drive_note.add_css_class("note-ok");
        }
        (None, Some(Err(e))) => {
            l.drive_note.set_text(e);
            l.drive_note.add_css_class("note-bad");
        }
        _ => l.drive_note.set_text(""),
    }
}

/// The "Build" button and the launcher action: save as needed, pick where the ISO goes, start.
pub fn start_from_action(app: &Rc<App>) {
    let a = app.clone();
    glib::spawn_future_local(async move {
        let (problems, running, volumes, to_file, stick_pref, path) = {
            let st = a.st.borrow();
            (
                !st.model.problems().is_empty(),
                st.build.running.is_some(),
                st.media.volumes.clone(),
                st.media.to_file,
                st.media.stick,
                st.model.path.clone(),
            )
        };
        if problems || running {
            a.set_status(if running {
                "a build is already running"
            } else {
                "the config is invalid; see the problem bar"
            });
            return;
        }
        let vv: Vec<usize> = volumes
            .iter()
            .enumerate()
            .filter(|(_, v)| media::is_ventoy(v, &volumes))
            .map(|(i, _)| i)
            .collect();
        let stick_idx = stick_pref
            .filter(|i| vv.contains(i))
            .or(vv.first().copied());
        if let Some(idx) = stick_idx.filter(|_| !to_file) {
            let Some(v) = volumes.get(idx).cloned() else {
                return;
            };
            if !v.mounted {
                a.set_status("mount the Ventoy drive first");
                return;
            }
            // Straight to the stick: an unsaved config is built from a temporary copy, no dialog.
            let name = path
                .as_ref()
                .map(|p| file_stem(p))
                .unwrap_or_else(|| "archstaller".into());
            let cfg = match path {
                Some(p) => {
                    if !a.save_async().await {
                        return;
                    }
                    Ok(p)
                }
                None => {
                    let p = std::env::temp_dir()
                        .join(format!("archstaller-{}.toml", std::process::id()));
                    let text = a.st.borrow().model.toml();
                    std::fs::write(&p, text)
                        .map(|_| p)
                        .map_err(|e| format!("cannot write the temporary config: {e}"))
                }
            };
            match cfg {
                Ok(cfg) => {
                    let out = std::env::temp_dir().join(format!("{name}.iso"));
                    start_build(&a, cfg, out);
                    a.st.borrow_mut().build.stick = Some((v, format!("{name}.iso")));
                }
                Err(e) => a.st.borrow_mut().build.error = Some(e),
            }
        } else {
            if !a.save_async().await {
                return;
            }
            let Some(cfg) = a.st.borrow().model.path.clone() else {
                return;
            };
            let name = file_stem(&cfg);
            let Some(out) = dialogs::save_file(
                a.win.upcast_ref(),
                "Save the ISO as",
                &format!("{name}.iso"),
                Some(("ISO image", "*.iso")),
            )
            .await
            else {
                return;
            };
            // The save dialog already asks before replacing a file; no second question.
            start_build(&a, cfg, out);
        }
    });
}

/// "Flash" without an ISO: save the config (a temporary copy while it has no file), build the ISO in the
/// scratch workspace, then `pump_copy` writes it to `dev` and deletes the workspace.
fn start_build_and_flash(app: &Rc<App>, dev: flash::RawDevice) {
    let a = app.clone();
    glib::spawn_future_local(async move {
        let path = a.st.borrow().model.path.clone();
        let cfg = match path {
            Some(p) => {
                if !a.save_async().await {
                    return;
                }
                Ok(p)
            }
            None => {
                let p =
                    std::env::temp_dir().join(format!("archstaller-{}.toml", std::process::id()));
                let text = a.st.borrow().model.toml();
                std::fs::write(&p, text)
                    .map(|_| p)
                    .map_err(|e| format!("cannot write the temporary config: {e}"))
            }
        };
        match cfg {
            Ok(cfg) => {
                if dev.media_kind() == flash::MediaKind::Floppy {
                    let out = std::env::temp_dir().join("archstaller-flash.img");
                    start_build_floppy(&a, cfg, out);
                } else if dev.media_kind() == flash::MediaKind::RaspberryPi {
                    let out = std::env::temp_dir().join("archstaller-rpi.img");
                    start_build_rpi(&a, cfg, out);
                } else {
                    let out = std::env::temp_dir().join("archstaller-flash.iso");
                    start_build(&a, cfg, out);
                }
                let mut st = a.st.borrow_mut();
                if st.build.running.is_some() {
                    st.build.flash = Some(dev);
                }
            }
            Err(e) => a.st.borrow_mut().build.error = Some(e),
        }
    });
}

fn start_build_rpi(app: &Rc<App>, cfg: PathBuf, out: PathBuf) {
    let root = match build::find_root() {
        Ok(r) => r,
        Err(e) => {
            app.st.borrow_mut().build.error = Some(e);
            return;
        }
    };
    let mut st = app.st.borrow_mut();
    st.next_build += 1;
    let id = format!("{}-{}", std::process::id(), st.next_build);
    st.build = BuildState::default();
    match Build::start_rpi(&root, &cfg, &id) {
        Ok(b) => {
            st.build.out = Some(out);
            let profile = st.model.effective_profile();
            st.build.summary = Some(format!(
                "Raspberry Pi config {}, profile {}",
                cfg.display(),
                profile
            ));
            st.build.running = Some(b);
        }
        Err(e) => st.build.error = Some(e),
    }
}

fn start_build_floppy(app: &Rc<App>, cfg: PathBuf, out: PathBuf) {
    let root = match build::find_root() {
        Ok(r) => r,
        Err(e) => {
            app.st.borrow_mut().build.error = Some(e);
            return;
        }
    };
    let mut st = app.st.borrow_mut();
    st.next_build += 1;
    let id = format!("{}-{}", std::process::id(), st.next_build);
    st.build = BuildState::default();
    match Build::start_floppy(&root, &cfg, &id) {
        Ok(b) => {
            st.build.out = Some(out);
            let profile = st.model.effective_profile();
            st.build.summary = Some(format!(
                "floppy config {}, profile {}",
                cfg.display(),
                profile
            ));
            st.build.running = Some(b);
        }
        Err(e) => st.build.error = Some(e),
    }
}

fn start_build(app: &Rc<App>, cfg: PathBuf, out: PathBuf) {
    let root = match build::find_root() {
        Ok(r) => r,
        Err(e) => {
            app.st.borrow_mut().build.error = Some(e);
            return;
        }
    };
    let mut st = app.st.borrow_mut();
    st.next_build += 1;
    let id = format!("{}-{}", std::process::id(), st.next_build);
    st.build = BuildState::default();
    match Build::start(&root, &cfg, &id) {
        Ok(b) => {
            st.build.out = Some(out);
            let profile = st.model.effective_profile();
            st.build.summary = Some(format!("config {}, profile {}", cfg.display(), profile));
            st.build.running = Some(b);
        }
        Err(e) => st.build.error = Some(e),
    }
}

/// Moves what the build and copy threads reported into the state. `true` when something changed.
pub fn pump(app: &Rc<App>) -> bool {
    let mut changed = false;
    let mut start_copy_after: Option<(PathBuf, Volume, PathBuf, String)> = None;
    {
        let mut st = app.st.borrow_mut();
        // Build.
        let mut exit = None;
        if let Some(b) = &st.build.running {
            let mut msgs = Vec::new();
            let mut disconnected = false;
            loop {
                match b.rx.try_recv() {
                    Ok(m) => msgs.push(m),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            for m in msgs {
                changed = true;
                match m {
                    Msg::Log(l) => {
                        if st.build.log.len() < 20_000 {
                            st.build.log.push(l);
                        }
                    }
                    Msg::Event(Event::Stage { name, state }) => {
                        match st.build.stages.iter_mut().find(|(n, _)| *n == name) {
                            Some(s) => s.1 = state,
                            None => st.build.stages.push((name, state)),
                        }
                    }
                    Msg::Event(Event::Done {
                        path, size, sha256, ..
                    }) => st.build.done = Some((path, size, sha256)),
                    Msg::Event(Event::Failed { message }) => st.build.error = Some(message),
                    Msg::Exit(ok) => exit = Some(ok),
                }
            }
            if exit.is_none() && disconnected {
                changed = true;
                exit = Some(false);
                if st.build.error.is_none() {
                    st.build.error = Some("Build worker thread disconnected unexpectedly".into());
                }
            }
        }
        if let Some(ok) = exit {
            let Some(b) = st.build.running.take() else {
                return pump_copy(app, start_copy_after, changed);
            };
            drop(std::fs::write(&b.log_path, st.build.log.join("\n")));
            match (ok, st.build.done.take()) {
                (true, Some((path, size, sha)))
                    if st.build.flash.is_some()
                        && build::usable(std::path::Path::new(&path), size) =>
                {
                    if let Some(dev) = st.build.flash.take() {
                        st.media.cleanup = Some(b.workdir.clone());
                        st.build.iso = Some((PathBuf::from(&path), size, sha));
                        st.media.result = None;
                        st.build.flash_pending = Some((PathBuf::from(path), dev));
                    }
                }
                (true, Some((path, size, sha)))
                    if st.build.stick.is_some()
                        && build::usable(std::path::Path::new(&path), size) =>
                {
                    if let Some((v, name)) = st.build.stick.take() {
                        st.media.cleanup = Some(b.workdir.clone());
                        st.build.iso = Some((PathBuf::from(&path), size, sha));
                        st.media.result = None;
                        let dir = v.mount.clone();
                        start_copy_after = Some((PathBuf::from(path), v, dir, name));
                    }
                }
                (true, Some((path, size, sha)))
                    if build::usable(std::path::Path::new(&path), size) =>
                {
                    if let Some(out) = st.build.out.take() {
                        let moved = std::fs::rename(&path, &out)
                            .or_else(|_| std::fs::copy(&path, &out).map(|_| ()));
                        match moved {
                            Ok(()) if build::usable(&out, size) => {
                                drop(std::fs::remove_dir_all(&b.workdir));
                                st.build.iso = Some((out, size, sha));
                            }
                            Ok(()) => {
                                st.build.error = Some(
                                    "the ISO was moved but is not readable at the expected size".into(),
                                )
                            }
                            Err(e) => {
                                st.build.error =
                                    Some(format!("cannot place the ISO at its destination: {e}"))
                            }
                        }
                    }
                }
                _ => {
                    if st.build.error.is_none() {
                        st.build.error = Some(format!(
                            "The build failed; see the log (kept at {})",
                            b.log_path.display()
                        ));
                    }
                }
            }
        }
        // Mount / unmount.
        if let Some(rx) = &st.media.drive_rx {
            match rx.try_recv() {
                Ok(r) => {
                    st.media.drive_rx = None;
                    st.media.drive_busy = None;
                    st.media.drive_msg = Some(r);
                    st.media.volumes = media::volumes();
                    st.media.selected = None;
                    drop(st);
                    app.rebuild(Page::Iso);
                    return pump_copy(app, start_copy_after, true);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    st.media.drive_rx = None;
                    st.media.drive_busy = None;
                    st.media.drive_msg = Some(Err("Drive worker thread disconnected unexpectedly".into()));
                    st.media.volumes = media::volumes();
                    st.media.selected = None;
                    drop(st);
                    app.rebuild(Page::Iso);
                    return pump_copy(app, start_copy_after, true);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
    }
    pump_copy(app, start_copy_after, changed)
}

fn pump_copy(
    app: &Rc<App>,
    start_copy_after: Option<(PathBuf, Volume, PathBuf, String)>,
    mut changed: bool,
) -> bool {
    let pending = app.st.borrow_mut().build.flash_pending.take();
    if let Some((iso, dev)) = pending {
        start_flash(app, iso, dev);
        changed = true;
    }
    {
        let mut st = app.st.borrow_mut();
        let mut finished = false;
        if let Some(rx) = &st.media.rx {
            let mut msgs = Vec::new();
            let mut disconnected = false;
            loop {
                match rx.try_recv() {
                    Ok(m) => msgs.push(m),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            for m in msgs {
                changed = true;
                match m {
                    MediaMsg::Progress(p, d, t) => st.media.progress = Some((p, d, t)),
                    MediaMsg::Done(r) => {
                        // A flash built its ISO only for this write: the workspace goes whether or not it worked.
                        if r.is_ok() || st.media.flashing {
                            if let Some(d) = st.media.cleanup.take() {
                                drop(std::fs::remove_dir_all(d));
                                st.build.iso = None;
                            }
                        }
                        st.media.result = Some(r.map(|r| {
                            let (what, size) = if r.flashed {
                                ("Flashed and verified", format!("{} bytes", r.size))
                            } else {
                                ("Copied and verified", format!("{} KiB", r.size >> 10))
                            };
                            let note = r.note.map(|n| format!(" {n}")).unwrap_or_default();
                            format!(
                                "{what} {} ({size}, sha256 {}).{note}",
                                r.dest.display(),
                                r.sha256
                            )
                        }));
                        finished = true;
                    }
                }
            }
            if !finished && disconnected {
                changed = true;
                finished = true;
                if let Some(d) = st.media.cleanup.take() {
                    drop(std::fs::remove_dir_all(d));
                    st.build.iso = None;
                }
                st.media.result = Some(Err("Worker thread disconnected unexpectedly".into()));
            }
        }
        if finished {
            st.media.rx = None;
            st.media.progress = None;
            st.media.volumes = media::volumes();
        }
        // Pick up sticks plugged in or removed while the page is open.
        if app.current() == Page::Iso
            && st.media.rx.is_none()
            && st.media.drive_rx.is_none()
            && st
                .media
                .last_scan
                .is_none_or(|t| t.elapsed().as_secs() >= 3)
        {
            let fresh = media::volumes();
            if fresh.len() != st.media.volumes.len()
                || fresh
                    .iter()
                    .zip(&st.media.volumes)
                    .any(|(a, b)| a.name != b.name || a.mounted != b.mounted)
            {
                st.media.selected = None;
                st.media.volumes = fresh;
                st.media.last_scan = Some(std::time::Instant::now());
                drop(st);
                app.rebuild(Page::Iso);
                if let Some((iso, v, dir, name)) = start_copy_after {
                    begin_copy(app, iso, v, dir, name);
                }
                return true;
            }
            st.media.last_scan = Some(std::time::Instant::now());
        }
    }
    if let Some((iso, v, dir, name)) = start_copy_after {
        begin_copy(app, iso, v, dir, name);
        changed = true;
    }
    changed
}

/// Copies `iso` to `dir` on `v` as `name`; asks first when earlier builds are already there.
fn begin_copy(app: &Rc<App>, iso: PathBuf, v: Volume, dir: PathBuf, name: String) {
    let files = media::older_isos(&dir, &name);
    if files.is_empty() {
        start_copy(app, iso, v, dir, vec![], Some(name));
        return;
    }
    let new_name = media::free_name(&dir, &name);
    older_prompt(app, files, iso, v, dir, name, new_name);
}

fn older_prompt(
    app: &Rc<App>,
    files: Vec<PathBuf>,
    iso: PathBuf,
    v: Volume,
    dir: PathBuf,
    name: String,
    new_name: String,
) {
    let win = gtk::Window::builder()
        .title("Older ISO on the stick")
        .modal(true)
        .transient_for(&app.window())
        .resizable(false)
        .build();
    let b = ui::vbox(8);
    b.set_margin_top(14);
    b.set_margin_bottom(14);
    b.set_margin_start(16);
    b.set_margin_end(16);
    b.append(
        &gtk::Label::builder()
            .label("The stick already holds an earlier build of this ISO:")
            .xalign(0.0)
            .selectable(true)
            .build(),
    );
    for f in &files {
        let size = std::fs::metadata(f).map(|m| m.len() >> 20).unwrap_or(0);
        b.append(
            &gtk::Label::builder()
                .label(format!(
                    "  {} ({} MiB)",
                    f.file_name().unwrap_or_default().to_string_lossy(),
                    size
                ))
                .xalign(0.0)
                .selectable(true)
                .build(),
        );
    }
    b.append(
        &gtk::Label::builder()
            .label(
                "Delete the old one(s) for good, or keep them and store the new ISO under another name.",
            )
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(60)
            .selectable(true)
            .build(),
    );
    let entry = ui::entry(&new_name, |_| {});
    ui::row(&b, "New name", &entry);
    let taken = ui::hint(&b, "That name is taken; choose another.");
    let btns = ui::hbox(8);
    btns.set_halign(gtk::Align::End);
    let del = gtk::Button::with_label("Delete old, copy");
    let keep = gtk::Button::with_label("Keep, rename new");
    let cancel = gtk::Button::with_label("Cancel");
    btns.append(&cancel);
    btns.append(&keep);
    btns.append(&del);
    b.append(&btns);
    let check = {
        let (entry, keep, taken, dir) = (entry.clone(), keep.clone(), taken.clone(), dir.clone());
        move || {
            let n = entry.text().trim().to_string();
            let exists = !n.is_empty() && dir.join(&n).exists();
            taken.set_visible(exists);
            keep.set_sensitive(!n.is_empty() && !exists);
        }
    };
    check();
    entry.connect_changed(move |_| check());
    win.set_child(Some(&b));
    {
        let (a, w, iso, v, dir, name, files) = (
            app.clone(),
            win.clone(),
            iso.clone(),
            v.clone(),
            dir.clone(),
            name.clone(),
            files.clone(),
        );
        del.connect_clicked(move |_| {
            w.close();
            start_copy(
                &a,
                iso.clone(),
                v.clone(),
                dir.clone(),
                files.clone(),
                Some(name.clone()),
            );
        });
    }
    {
        let (a, w, iso, v, dir, entry) = (
            app.clone(),
            win.clone(),
            iso.clone(),
            v.clone(),
            dir.clone(),
            entry.clone(),
        );
        keep.connect_clicked(move |_| {
            let n = entry.text().trim().to_string();
            let n = if n.to_ascii_lowercase().ends_with(".iso") {
                n
            } else {
                format!("{n}.iso")
            };
            w.close();
            start_copy(&a, iso.clone(), v.clone(), dir.clone(), vec![], Some(n));
        });
    }
    {
        let (a, w, iso) = (app.clone(), win.clone(), iso);
        cancel.connect_clicked(move |_| {
            w.close();
            let mut st = a.st.borrow_mut();
            if st.media.cleanup.take().is_some() {
                st.build.error = Some(format!(
                    "Not copied. The built ISO is kept at {}",
                    iso.display()
                ));
            }
        });
    }
    win.present();
}

fn start_copy(
    app: &Rc<App>,
    iso: PathBuf,
    v: Volume,
    dir: PathBuf,
    delete: Vec<PathBuf>,
    rename_to: Option<String>,
) {
    let target = FolderCopy {
        dir,
        available: Some(v.available),
        overwrite: false,
        delete,
        trash_root: Some(v.mount.clone()),
        rename_to,
    };
    run_media(app, iso, Box::new(target), false);
}

fn start_flash(app: &Rc<App>, iso: PathBuf, dev: flash::RawDevice) {
    let target: Box<dyn MediaTarget + Send> = match dev.media_kind() {
        flash::MediaKind::Optical => Box::new(flash::OpticalBurn { dev }),
        flash::MediaKind::Floppy => Box::new(flash::FloppyFlash {
            dev,
            backend: Box::new(flash::UDisks),
        }),
        flash::MediaKind::Usb | flash::MediaKind::RaspberryPi => Box::new(flash::RawBlockFlash {
            dev,
            backend: Box::new(flash::UDisks),
        }),
    };
    run_media(app, iso, target, true);
}

/// Runs a copy or a flash on a thread; `pump_copy` picks up its progress and result.
fn run_media(app: &Rc<App>, iso: PathBuf, target: Box<dyn MediaTarget + Send>, flashing: bool) {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut st = app.st.borrow_mut();
        st.media.rx = Some(rx);
        st.media.result = None;
        st.media.cancel = cancel.clone();
        st.media.flashing = flashing;
    }
    std::thread::spawn(move || {
        let t2 = tx.clone();
        let r = target.write(
            &iso,
            &mut |p, d, t| {
                drop(t2.send(MediaMsg::Progress(p, d, t)));
            },
            &cancel,
        );
        drop(tx.send(MediaMsg::Done(r)));
    });
}

/// Mounts or unmounts (after a sync) a device in the background; the answer is shown under the list.
fn run_drive(app: &Rc<App>, dev: String, mount: bool) {
    let (tx, rx) = channel();
    {
        let mut st = app.st.borrow_mut();
        st.media.drive_rx = Some(rx);
        st.media.drive_busy = Some(dev.clone());
        st.media.drive_msg = None;
    }
    std::thread::spawn(move || {
        let r = if mount {
            media::mount(&dev).map(|p| format!("Mounted {dev} at {}", p.display()))
        } else {
            media::sync_and_unmount(&dev)
                .map(|_| format!("{dev} is synced and unmounted: it is safe to remove"))
        };
        drop(tx.send(r));
    });
}

/// Raw write: a dialog that looks for appropriate media drives (USB, optical, or floppy) off the main thread,
/// lists the ones that may be written (none preselected) and the relevant ones that may not, with the reasons,
/// and starts only after the user selected a drive and typed its kernel name.
fn flash_dialog(app: &Rc<App>, iso: Option<PathBuf>, kind: flash::MediaKind) {
    let win = gtk::Window::builder()
        .title(kind.title())
        .modal(true)
        .transient_for(&app.window())
        .default_width(560)
        .default_height(420)
        .build();
    let body = ui::vbox(8);
    body.set_margin_top(14);
    body.set_margin_bottom(14);
    body.set_margin_start(16);
    body.set_margin_end(16);
    win.set_child(Some(&body));
    win.present();
    flash_scan(app, &win, &body, iso, kind);
}

fn flash_scan(
    app: &Rc<App>,
    win: &gtk::Window,
    body: &gtk::Box,
    iso: Option<PathBuf>,
    kind: flash::MediaKind,
) {
    ui::clear(body);
    let searching_label = match kind {
        flash::MediaKind::Usb => "Looking for USB drives…",
        flash::MediaKind::Optical => "Looking for optical drives (CD/DVD/Blu-ray)…",
        flash::MediaKind::Floppy => "Looking for floppy drives…",
        flash::MediaKind::RaspberryPi => "Looking for SD cards and USB drives for Raspberry Pi…",
    };
    body.append(&ui::spinner_row(searching_label).0);
    let (a, w, b) = (app.clone(), win.clone(), body.clone());
    glib::spawn_future_local(async move {
        let path = iso.clone();
        let found = gtk::gio::spawn_blocking(move || {
            let src = match &path {
                Some(p) => match kind {
                    flash::MediaKind::Floppy => flash::Source::floppy(p)?,
                    flash::MediaKind::RaspberryPi => flash::Source::rpi(p)?,
                    _ => flash::Source::iso(p)?,
                },
                None => match kind {
                    flash::MediaKind::Floppy => flash::Source {
                        path: PathBuf::new(),
                        len: 1_474_560,
                        dev: (0, 0),
                        kind: flash::SourceKind::Floppy,
                    },
                    flash::MediaKind::RaspberryPi => flash::Source {
                        path: PathBuf::new(),
                        len: 32 * 1024 * 1024,
                        dev: (0, 0),
                        kind: flash::SourceKind::RawImg,
                    },
                    _ => flash::Source {
                        path: PathBuf::new(),
                        len: 0,
                        dev: (0, 0),
                        kind: flash::SourceKind::Iso,
                    },
                },
            };
            if kind == flash::MediaKind::Usb || kind == flash::MediaKind::RaspberryPi {
                flash::UDisks::ready()?;
            }
            let list = flash::scan_for(&src, kind)?;
            Ok::<_, String>((src, list))
        })
        .await
        .unwrap_or_else(|_| Err("the drive scan failed unexpectedly".into()));
        flash_show(&a, &w, &b, iso, kind, found);
    });
}

fn flash_show(
    app: &Rc<App>,
    win: &gtk::Window,
    body: &gtk::Box,
    iso: Option<PathBuf>,
    kind: flash::MediaKind,
    found: Result<(flash::Source, Vec<Candidate>), String>,
) {
    ui::clear(body);
    let heading = match kind {
        flash::MediaKind::Usb => "Write the ISO over a whole USB stick",
        flash::MediaKind::Optical => "Burn the ISO to an optical disc (CD/DVD/Blu-ray)",
        flash::MediaKind::Floppy => "Write floppy image to a 1.44 MB diskette",
        flash::MediaKind::RaspberryPi => "Flash Raspberry Pi boot image to SD card / USB drive",
    };
    body.append(&ui::strong(heading));

    let target_row = ui::hbox(8);
    let target_label = gtk::Label::builder()
        .label("Target Media:")
        .xalign(0.0)
        .selectable(true)
        .build();
    let dropdown = gtk::DropDown::from_strings(&[
        "USB Flash Drive (x86_64)",
        "Optical Disc (CD / DVD / Blu-ray)",
        "Floppy Diskette (1.44 MB)",
        "Raspberry Pi (AArch64 SD / USB)",
    ]);
    dropdown.set_selected(match kind {
        flash::MediaKind::Usb => 0,
        flash::MediaKind::Optical => 1,
        flash::MediaKind::Floppy => 2,
        flash::MediaKind::RaspberryPi => 3,
    });
    let (a, w, b, iso_for_change) = (app.clone(), win.clone(), body.clone(), iso.clone());
    dropdown.connect_selected_notify(move |dd| {
        let new_kind = match dd.selected() {
            0 => flash::MediaKind::Usb,
            1 => flash::MediaKind::Optical,
            2 => flash::MediaKind::Floppy,
            _ => flash::MediaKind::RaspberryPi,
        };
        if new_kind != kind {
            w.set_title(Some(new_kind.title()));
            flash_scan(&a, &w, &b, iso_for_change.clone(), new_kind);
        }
    });
    target_row.append(&target_label);
    target_row.append(&dropdown);
    body.append(&target_row);

    let buttons = ui::hbox(8);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&ui::button("Refresh", {
        let (a, w, b, iso) = (app.clone(), win.clone(), body.clone(), iso.clone());
        move || flash_scan(&a, &w, &b, iso.clone(), kind)
    }));
    buttons.append(&ui::button("Cancel", {
        let w = win.clone();
        move || w.close()
    }));
    let go = gtk::Button::with_label(kind.action_label());
    go.add_css_class("destructive-action");
    go.set_sensitive(false);
    buttons.append(&go);
    let (src, list) = match found {
        Ok(x) => x,
        Err(e) => {
            let note_msg = match kind {
                flash::MediaKind::Usb => format!("Raw flashing is not possible: {e}"),
                flash::MediaKind::Optical => format!("Optical burning is not possible: {e}"),
                flash::MediaKind::Floppy => format!("Floppy writing is not possible: {e}"),
                flash::MediaKind::RaspberryPi => {
                    format!("Raspberry Pi flashing is not possible: {e}")
                }
            };
            ui::note(body, Kind::Bad, &note_msg).set_selectable(true);
            ui::hint(
                body,
                "Copying the ISO into a folder of a mounted volume still works.",
            );
            body.append(&buttons);
            return;
        }
    };
    if iso.is_some() {
        let desc = match kind {
            flash::MediaKind::Usb => format!("{} ({}) is written from the first byte of the device. Every partition and file on the stick is destroyed; this cannot be undone.", src.path.display(), flash::human(src.len)),
            flash::MediaKind::Optical => format!("{} ({}) will be burned to the optical disc in the selected drive.", src.path.display(), flash::human(src.len)),
            flash::MediaKind::Floppy => format!("{} ({}) will be written to sector 0 of the floppy diskette. All data on the diskette will be overwritten.", src.path.display(), flash::human(src.len)),
            flash::MediaKind::RaspberryPi => format!("{} ({}) will be written from byte 0 to the SD card / USB drive. All partitions and data on the device will be destroyed.", src.path.display(), flash::human(src.len)),
        };
        ui::hint(body, &desc);
    } else {
        let desc = match kind {
            flash::MediaKind::Usb => "The config is saved, the ISO is built, written from the first byte of the device and checked, then the built ISO is deleted. Every partition and file on the stick is destroyed; this cannot be undone.",
            flash::MediaKind::Optical => "The config is saved, the ISO is built, burned to the optical disc in the selected drive, then the built ISO is deleted.",
            flash::MediaKind::Floppy => "The config is saved, the floppy image (1.44 MB) is built, written to the floppy diskette and verified, then the temporary image is deleted. All data on the diskette will be overwritten.",
            flash::MediaKind::RaspberryPi => "The config is saved, the Raspberry Pi bootable image (32 MiB) is built, written to the SD card / USB drive and verified, then the temporary image is deleted. All data on the device will be overwritten.",
        };
        ui::hint(body, desc);
    }
    let (ok, rest): (Vec<Candidate>, Vec<Candidate>) = list.into_iter().partition(|c| c.eligible());
    if ok.is_empty() {
        let empty_msg = match kind {
            flash::MediaKind::Usb => "No USB drive can be flashed. Plug one in and press Refresh.",
            flash::MediaKind::Optical => "No writable optical drive detected. Insert a blank/rewritable disc and press Refresh.",
            flash::MediaKind::Floppy => "No floppy drive detected. Insert a floppy diskette and press Refresh.",
            flash::MediaKind::RaspberryPi => "No removable SD card (mmcblk*) or USB drive detected. Insert one and press Refresh.",
        };
        ui::note(body, Kind::Warn, empty_msg);
    }
    let chosen: Rc<RefCell<Option<flash::RawDevice>>> = Rc::new(RefCell::new(None));
    let warn = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(70)
        .css_classes(["note-bad"])
        .visible(false)
        .selectable(true)
        .build();
    let prompt = gtk::Label::builder()
        .xalign(0.0)
        .visible(false)
        .selectable(true)
        .build();
    let typed = gtk::Entry::builder().visible(false).build();
    let mut group: Option<gtk::CheckButton> = None;
    for cand in ok {
        let d = cand.dev;
        let size_str = if d.size > 0 {
            flash::human(d.size)
        } else {
            String::new()
        };
        let rb = ui::check_button(&format!("{}   {}   {}", d.name(), d.path, size_str));
        if let Some(g) = &group {
            rb.set_group(Some(g));
        } else {
            group = Some(rb.clone());
        }
        rb.set_active(false);
        body.append(&rb);
        let serial = if d.serial.is_empty() {
            "unknown".to_string()
        } else {
            d.serial.clone()
        };
        let mounted: Vec<String> = d
            .mounted()
            .into_iter()
            .map(|(_, p, m)| format!("{p} at {m}"))
            .collect();
        let detail = match kind {
            flash::MediaKind::Usb => {
                if mounted.is_empty() {
                    format!("USB, serial {serial}; nothing mounted")
                } else {
                    format!(
                        "USB, serial {serial}; mounted: {} (unmounted before writing)",
                        mounted.join(", ")
                    )
                }
            }
            flash::MediaKind::Optical => {
                if mounted.is_empty() {
                    format!(
                        "Optical drive, {}; nothing mounted",
                        if d.removable { "removable" } else { "internal" }
                    )
                } else {
                    format!(
                        "Optical drive; mounted: {} (unmounted before burning)",
                        mounted.join(", ")
                    )
                }
            }
            flash::MediaKind::Floppy => {
                if mounted.is_empty() {
                    "Floppy drive (1.44 MB standard); nothing mounted".to_string()
                } else {
                    format!(
                        "Floppy drive; mounted: {} (unmounted before writing)",
                        mounted.join(", ")
                    )
                }
            }
            flash::MediaKind::RaspberryPi => {
                let bus = if d.kname.starts_with("mmcblk") {
                    "SD / MMC card"
                } else {
                    "USB drive"
                };
                if mounted.is_empty() {
                    format!("{bus}, serial {serial}; nothing mounted")
                } else {
                    format!(
                        "{bus}, serial {serial}; mounted: {} (unmounted before writing)",
                        mounted.join(", ")
                    )
                }
            }
        };
        ui::hint(body, &detail).set_margin_start(24);
        let (chosen, warn, prompt, typed, go) = (
            chosen.clone(),
            warn.clone(),
            prompt.clone(),
            typed.clone(),
            go.clone(),
        );
        rb.connect_toggled(move |b| {
            if !b.is_active() {
                return;
            }
            let warn_text = match kind {
                flash::MediaKind::Usb => format!(
                    "Every partition and file on {} ({}, {}) will be destroyed.",
                    d.name(),
                    d.path,
                    flash::human(d.size)
                ),
                flash::MediaKind::Optical => {
                    format!("The disc in {} ({}) will be written.", d.name(), d.path)
                }
                flash::MediaKind::Floppy => format!(
                    "All data on floppy {} ({}) will be overwritten.",
                    d.name(),
                    d.path
                ),
                flash::MediaKind::RaspberryPi => format!(
                    "Every partition and file on {} ({}, {}) will be destroyed.",
                    d.name(),
                    d.path,
                    flash::human(d.size)
                ),
            };
            warn.set_text(&warn_text);
            prompt.set_text(&format!("Type {} to confirm:", d.kname));
            warn.set_visible(true);
            prompt.set_visible(true);
            typed.set_visible(true);
            *chosen.borrow_mut() = Some(d.clone());
            typed.set_text("");
            go.set_sensitive(false);
        });
    }
    body.append(&warn);
    body.append(&prompt);
    body.append(&typed);
    {
        let (chosen, go) = (chosen.clone(), go.clone());
        typed.connect_changed(move |e| {
            let matches = chosen
                .borrow()
                .as_ref()
                .is_some_and(|d| e.text().trim() == d.kname);
            go.set_sensitive(matches);
        });
    }
    let shown: Vec<&Candidate> = rest.iter().filter(|c| c.relevant_for(kind)).collect();
    let hidden = rest.len() - shown.len();
    if !shown.is_empty() || hidden > 0 {
        body.append(&ui::strong("Not offered"));
        for c in &shown {
            ui::hint(
                body,
                &format!(
                    "{} ({}): {}",
                    c.dev.name(),
                    c.dev.path,
                    c.problems.join("; ")
                ),
            );
        }
        if hidden > 0 {
            ui::hint(body, &format!("{hidden} other disk(s) are not offered."));
        }
    }
    body.append(&buttons);
    let (a, w) = (app.clone(), win.clone());
    go.connect_clicked(move |_| {
        let Some(d) = chosen.borrow().clone() else {
            return;
        };
        if typed.text().trim() != d.kname {
            return;
        }
        w.close();
        match iso.clone() {
            Some(iso) => start_flash(&a, iso, d),
            None => start_build_and_flash(&a, d),
        }
    });
}
