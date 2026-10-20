use crate::app::{App, Page};
use crate::dialogs;
use crate::model::Model;
use crate::ui::{self, Kind};
use gtk::glib;
use gtk::prelude::*;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// A list card: an "add" field with suggestions on top of the list editor.
struct ListCard<'a> {
    app: &'a Rc<App>,
    content: &'a gtk::Box,
    title: &'a str,
    label: &'a str,
    hint: &'a str,
    options: Rc<Vec<String>>,
    items: Vec<String>,
    height: i32,
    set: fn(&mut Model, Vec<String>),
    get: fn(&Model) -> Vec<String>,
}

fn list_card(args: ListCard<'_>) {
    let ListCard {
        app,
        content,
        title,
        label,
        hint,
        options,
        items,
        height,
        set,
        get,
    } = args;
    let c = ui::card(content, Some(title));
    let add = ui::suggest("", options, false, |_| {});
    let r = ui::row(&c, label, &add);
    let btn = ui::button("Add", || {});
    btn.set_sensitive(false);
    r.append(&btn);
    {
        let b = btn.clone();
        add.connect_changed(move |e| b.set_sensitive(!e.text().trim().is_empty()));
    }
    {
        let (a, add) = (app.clone(), add.clone());
        btn.connect_clicked(move |_| {
            let v = add.text().trim().to_string();
            if v.is_empty() {
                return;
            }
            a.edit(move |m| {
                let mut list = get(m);
                if !list.contains(&v) {
                    list.push(v);
                }
                set(m, list);
            });
            a.rebuild(Page::System);
        });
    }
    let a = app.clone();
    c.append(&ui::lines_editor(&items, height, move |v| {
        a.edit(move |m| set(m, v))
    }));
    ui::hint(&c, hint);
}

/// Modal dialog to edit or create a custom service unit.
fn open_custom_service_editor(
    app: &Rc<App>,
    existing_index: Option<usize>,
    initial: config::CustomService,
) {
    let (w, h) = (
        app.win.width().clamp(360, 680),
        app.win.height().clamp(360, 540),
    );
    let dialog = gtk::Window::builder()
        .title(if existing_index.is_some() {
            "Edit Custom Service"
        } else {
            "New Custom Service"
        })
        .transient_for(&app.win)
        .modal(true)
        .default_width(w)
        .default_height(h)
        .build();

    let root = ui::vbox(12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    let name_entry = ui::entry(&initial.name, |_| {});
    ui::row(&root, "Unit name", &name_entry);
    ui::hint(
        &root,
        "e.g. rclone-onedrive.service, backup.timer, my-daemon.service",
    );

    let users = app.st.borrow().model.cfg.users.clone();
    let mut user_options = vec!["root (machine-wide)".to_string()];
    for u in &users {
        user_options.push(format!("user: {}", u.name));
    }
    let user_strs: Vec<&str> = user_options.iter().map(|s| s.as_str()).collect();
    let user_dd = gtk::DropDown::from_strings(&user_strs);
    let selected_user_idx = match &initial.user {
        Some(u) if u != "root" => users
            .iter()
            .position(|usr| usr.name == *u)
            .map(|p| p + 1)
            .unwrap_or(0),
        _ => 0,
    };
    user_dd.set_selected(selected_user_idx as u32);
    ui::row(&root, "Target user", &user_dd);

    let enable_check = ui::check("Enable on first boot", initial.enable, |_| {});
    root.append(&enable_check);

    let is_file_initial = initial.file.is_some();
    let mode_box = ui::hbox(16);
    let inline_radio = gtk::CheckButton::builder()
        .label("Inline unit content")
        .active(!is_file_initial)
        .build();
    let file_radio = gtk::CheckButton::builder()
        .label("Local file path")
        .group(&inline_radio)
        .active(is_file_initial)
        .build();
    mode_box.append(&inline_radio);
    mode_box.append(&file_radio);
    root.append(&mode_box);

    let file_box = ui::vbox(6);
    let file_row = ui::hbox(8);
    let file_entry = ui::entry(initial.file.as_deref().unwrap_or(""), |_| {});
    file_entry.set_hexpand(true);
    let browse_btn = ui::button("Browse...", {
        let win = dialog.clone();
        let fe = file_entry.clone();
        move || {
            let win = win.clone();
            let fe = fe.clone();
            gtk::glib::spawn_future_local(async move {
                if let Some(path) = dialogs::open_file(&win, "Select Systemd Unit File", None).await
                {
                    fe.set_text(&path.to_string_lossy());
                }
            });
        }
    });
    file_row.append(&file_entry);
    file_row.append(&browse_btn);
    file_box.append(&file_row);
    ui::hint(
        &file_box,
        "Path to a local unit file (relative to the config file).",
    );
    root.append(&file_box);

    let inline_box = ui::vbox(6);
    let dark = ui::is_dark(&app.win);
    let buf = sourceview5::Buffer::new(None);
    if let Some(lang) = sourceview5::LanguageManager::default().language("ini") {
        buf.set_language(Some(&lang));
    }
    buf.set_highlight_syntax(true);
    let mgr = sourceview5::StyleSchemeManager::default();
    let scheme = [
        if dark { "Adwaita-dark" } else { "Adwaita" },
        if dark { "classic-dark" } else { "classic" },
    ]
    .iter()
    .find_map(|n| mgr.scheme(n));
    buf.set_style_scheme(scheme.as_ref());

    let default_template = "[Unit]\nDescription=My Custom Service\nAfter=network.target\n\n[Service]\nType=simple\nExecStart=/usr/bin/echo \"Hello world\"\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n";
    let text = match &initial.content {
        Some(bytes) => String::from_utf8_lossy(bytes).to_string(),
        None => {
            if initial.file.is_none() && initial.name.is_empty() {
                default_template.to_string()
            } else {
                String::new()
            }
        }
    };
    buf.set_text(&text);

    let view = sourceview5::View::with_buffer(&buf);
    view.set_monospace(true);
    view.set_show_line_numbers(true);
    view.set_highlight_current_line(true);
    view.set_left_margin(8);
    view.set_top_margin(6);
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .min_content_height(220)
        .child(&view)
        .has_frame(true)
        .build();
    inline_box.append(&scroll);
    root.append(&inline_box);

    let update_visibility = {
        let inline_radio = inline_radio.clone();
        let file_box = file_box.clone();
        let inline_box = inline_box.clone();
        Rc::new(move || {
            let is_inline = inline_radio.is_active();
            inline_box.set_visible(is_inline);
            file_box.set_visible(!is_inline);
        })
    };
    update_visibility();
    {
        let uv = update_visibility.clone();
        inline_radio.connect_toggled(move |_| uv());
    }

    let err_label = ui::note(&root, Kind::Bad, "");
    err_label.set_visible(false);

    let btn_row = ui::hbox(8);
    btn_row.set_halign(gtk::Align::End);
    let cancel = ui::button("Cancel", {
        let d = dialog.clone();
        move || d.close()
    });
    btn_row.append(&cancel);

    let save = ui::primary("Save", {
        let dialog = dialog.clone();
        let app = app.clone();
        let name_entry = name_entry.clone();
        let user_dd = user_dd.clone();
        let enable_check = enable_check.clone();
        let inline_radio = inline_radio.clone();
        let file_entry = file_entry.clone();
        let buf = buf.clone();
        let err_label = err_label.clone();
        let users = users.clone();
        move || {
            let name = name_entry.text().trim().to_string();
            if name.is_empty() {
                err_label.set_text("Unit name cannot be empty.");
                err_label.set_visible(true);
                return;
            }
            if !config::valid_unit_name(&name) {
                err_label.set_text(
                    "Invalid unit name (must end in .service, .timer, etc., without slashes).",
                );
                err_label.set_visible(true);
                return;
            }

            let user_idx = user_dd.selected() as usize;
            let user = if user_idx == 0 {
                None
            } else {
                users.get(user_idx - 1).map(|u| u.name.clone())
            };

            let enable = enable_check.is_active();

            let (file, content) = if inline_radio.is_active() {
                let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
                if text.trim().is_empty() {
                    err_label.set_text("Inline unit content cannot be empty.");
                    err_label.set_visible(true);
                    return;
                }
                (None, Some(text.as_bytes().to_vec()))
            } else {
                let f = file_entry.text().trim().to_string();
                if f.is_empty() {
                    err_label.set_text("Local file path cannot be empty.");
                    err_label.set_visible(true);
                    return;
                }
                (Some(f), None)
            };

            let service = config::CustomService {
                name,
                user,
                enable,
                file,
                content,
            };

            app.edit(move |m| {
                if let Some(idx) = existing_index {
                    if let Some(slot) = m.cfg.custom_services.get_mut(idx) {
                        *slot = service;
                    }
                } else {
                    m.cfg.custom_services.push(service);
                }
            });
            app.rebuild(Page::System);
            dialog.close();
        }
    });
    btn_row.append(&save);
    root.append(&btn_row);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .child(&root)
        .build();
    dialog.set_child(Some(&scroll));
    dialog.present();
}

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let (cfg, lists, report) = {
        let st = app.st.borrow();
        (
            st.model.cfg.clone(),
            (
                st.lists.timezones.clone(),
                st.lists.locales.clone(),
                st.lists.keymaps.clone(),
                st.lists.services.clone(),
                st.lists.kernel_params.clone(),
            ),
            st.services_import_report.clone(),
        )
    };

    ui::heading(
        content,
        "System",
        "Identity, root credentials, display manager, services, first-boot scripts, and overlays.",
    );

    // 1. Identity & Localization
    let c = ui::card(content, Some("Identity & Localization"));
    let a = app.clone();
    ui::row(
        &c,
        "Hostname",
        &ui::entry(&cfg.hostname, move |t| {
            let t = t.to_string();
            a.edit(move |m| m.cfg.hostname = t);
        }),
    );
    let a = app.clone();
    ui::row(
        &c,
        "Timezone",
        &ui::suggest(&cfg.timezone, lists.0, false, move |t| {
            let t = t.to_string();
            a.edit(move |m| m.cfg.timezone = t);
        }),
    );
    let a = app.clone();
    ui::row(
        &c,
        "Locale",
        &ui::suggest(&cfg.locale, lists.1, false, move |t| {
            let t = t.to_string();
            a.edit(move |m| m.cfg.locale = t);
        }),
    );
    let a = app.clone();
    ui::row(
        &c,
        "Keymap",
        &ui::suggest(&cfg.keymap, lists.2, false, move |t| {
            let t = t.to_string();
            a.edit(move |m| m.cfg.keymap = t);
        }),
    );
    let a = app.clone();
    let auto_id_btn = ui::button("Autoload localization from host", move || {
        let sys = hostcfg::system::discover_host_system();
        a.edit(|m| {
            m.cfg.hostname = sys.hostname.clone();
            m.cfg.timezone = sys.timezone.clone();
            m.cfg.locale = sys.locale.clone();
            m.cfg.keymap = sys.keymap.clone();
        });
        a.set_status("Autoloaded hostname, timezone, locale, and keymap from host system.");
        a.rebuild_all();
    });
    auto_id_btn.set_tooltip_text(Some(
        "Detects hostname (/etc/hostname), timezone (/etc/localtime), locale (/etc/locale.conf), and keymap (/etc/vconsole.conf).",
    ));
    c.append(&auto_id_btn);

    // 2. Root Account
    let c = ui::card(content, Some("Root Account"));
    let hash = Rc::new(RefCell::new(
        cfg.root_password_hash.clone().unwrap_or_default(),
    ));
    let hash_row = ui::vbox(6);
    let is_plain = Rc::new(RefCell::new(false));

    let he = ui::entry(&hash.borrow(), {
        let (a, hash) = (app.clone(), hash.clone());
        move |t| {
            *hash.borrow_mut() = t.to_string();
            let t = t.to_string();
            a.edit(move |m| m.cfg.root_password_hash = Some(t));
        }
    });
    let he_box = ui::vbox(4);
    ui::row(&he_box, "Password hash", &he);
    ui::hint(
        &he_box,
        "A crypt hash (e.g. $6$... SHA-512 or $y$... yescrypt).",
    );
    hash_row.append(&he_box);

    let pw = ui::password(|_| {});
    let pw2 = ui::password(|_| {});
    let plain_pw_box = ui::vbox(4);
    ui::row(&plain_pw_box, "Password", &pw);
    ui::row(&plain_pw_box, "Again", &pw2);
    let mismatch = ui::note(&plain_pw_box, Kind::Bad, "The passwords differ");
    mismatch.set_visible(false);
    plain_pw_box.set_visible(false);
    hash_row.append(&plain_pw_box);

    let mode_switch = ui::check("Enter plain password instead of hash", false, |_| {});
    {
        let (he_box, plain_pw_box, is_plain) = (
            he_box.clone(),
            plain_pw_box.clone(),
            is_plain.clone(),
        );
        mode_switch.connect_toggled(move |btn| {
            let plain = btn.is_active();
            *is_plain.borrow_mut() = plain;
            he_box.set_visible(!plain);
            plain_pw_box.set_visible(plain);
        });
    }
    {
        let (a, pw_val, pw2_val, mismatch, is_plain) = (
            app.clone(),
            pw.clone(),
            pw2.clone(),
            mismatch.clone(),
            is_plain.clone(),
        );
        let update_plain = move || {
            if *is_plain.borrow() {
                let (p1, p2) = (pw_val.text().to_string(), pw2_val.text().to_string());
                let matches = !p1.is_empty() && p1 == p2;
                mismatch.set_visible(!p1.is_empty() && p1 != p2);
                if matches {
                    if let Ok(h) = hostcfg::password::hash(&p1) {
                        a.edit(move |m| m.cfg.root_password_hash = Some(h));
                    }
                }
            }
        };
        let u1 = Rc::new(update_plain);
        let u2 = u1.clone();
        pw.upcast_ref::<gtk::Editable>().connect_changed(move |_| u1());
        pw2.upcast_ref::<gtk::Editable>().connect_changed(move |_| u2());
    }
    hash_row.append(&mode_switch);

    hash_row.set_visible(cfg.root_password_hash.is_some());
    let locked = ui::check(
        "Leave root locked (use sudo)",
        cfg.root_password_hash.is_none(),
        {
            let (a, hash_row, hash) = (app.clone(), hash_row.clone(), hash.clone());
            move |locked| {
                hash_row.set_visible(!locked);
                let value = if locked {
                    None
                } else {
                    Some(hash.borrow().clone())
                };
                a.edit(move |m| m.cfg.root_password_hash = value);
            }
        },
    );
    c.append(&locked);
    c.append(&hash_row);

    // 3. Display Manager
    let c = ui::card(content, Some("Display Manager & Login"));
    ui::hint(
        &c,
        "Select the display manager to start graphical login. Permanent Archstaller branding is automatically applied.",
    );
    let dms = [
        ("none", "None (Direct console login / TTY only)"),
        ("ly", "Ly (TUI Display Manager on TTY2)"),
        ("sddm", "SDDM (Simple Desktop Display Manager, Qt/KDE)"),
        ("gdm", "GDM (GNOME Display Manager)"),
        ("lightdm", "LightDM (Lightweight Display Manager, GTK)"),
    ];

    let current_dm = if cfg.services.iter().any(|s| s == "ly.service") {
        "ly"
    } else if cfg.services.iter().any(|s| s == "sddm.service") {
        "sddm"
    } else if cfg.services.iter().any(|s| s == "gdm.service") {
        "gdm"
    } else if cfg.services.iter().any(|s| s == "lightdm.service") {
        "lightdm"
    } else {
        "none"
    };

    let mut dm_group: Option<gtk::CheckButton> = None;
    for (id, desc) in dms {
        let b = if let Some(ref leader) = dm_group {
            gtk::CheckButton::builder().label(desc).group(leader).build()
        } else {
            let leader = gtk::CheckButton::builder().label(desc).build();
            dm_group = Some(leader.clone());
            leader
        };
        b.set_active(current_dm == id);
        let a = app.clone();
        b.connect_toggled(move |btn| {
            if btn.is_active() {
                a.edit(move |m| {
                    m.cfg.services.retain(|s| {
                        !matches!(
                            s.as_str(),
                            "ly.service"
                                | "sddm.service"
                                | "gdm.service"
                                | "lightdm.service"
                                | "greetd.service"
                        )
                    });
                    match id {
                        "ly" => m.cfg.services.push("ly.service".into()),
                        "sddm" => m.cfg.services.push("sddm.service".into()),
                        "gdm" => m.cfg.services.push("gdm.service".into()),
                        "lightdm" => m.cfg.services.push("lightdm.service".into()),
                        _ => {}
                    }
                });
            }
        });
        c.append(&b);
    }

    // 4. System Services & Kernel Parameters
    if let Some(rep) = report {
        let banner = ui::card(content, None);
        ui::note(&banner, Kind::Ok, &rep);
        let a = app.clone();
        banner.append(&ui::button("Dismiss", move || {
            a.st.borrow_mut().services_import_report = None;
            a.rebuild(Page::System);
        }));
    }

    list_card(ListCard {
        app,
        content,
        title: "Services enabled on first boot",
        label: "Unit",
        hint: "One systemd unit per line, for example sshd.service or chronyd.service.",
        options: lists.3,
        items: cfg.services.clone(),
        height: 130,
        set: |m, v| m.cfg.services = v,
        get: |m| m.cfg.services.clone(),
    });

    let c = ui::card(content, Some("Custom systemd services"));
    ui::hint(&c, "Custom unit files installed into /etc/systemd/system/ (machine-wide) or ~/.config/systemd/user/ (per-user).");

    if cfg.custom_services.is_empty() {
        ui::hint(&c, "No custom services configured yet.");
    } else {
        for (i, cs) in cfg.custom_services.iter().enumerate() {
            let item_box = ui::vbox(3);
            let r = ui::hbox(8);
            r.append(&ui::dim(&format!("{}", i + 1)));

            let name = ui::strong(&cs.name);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            name.set_tooltip_text(Some(&cs.name));
            name.set_hexpand(true);
            r.append(&name);

            let user_str = cs.user.as_deref().unwrap_or("root");
            let user_chip = ui::chip(
                &format!("user: {user_str}"),
                if user_str == "root" { "chip-accent" } else { "" },
            );
            r.append(&user_chip);

            let status_chip = if cs.enable {
                ui::chip("enabled", "chip-ok")
            } else {
                ui::chip("disabled", "chip-warn")
            };
            r.append(&status_chip);

            let a = app.clone();
            let cs_clone = cs.clone();
            r.append(&ui::button("Edit", move || {
                open_custom_service_editor(&a, Some(i), cs_clone.clone());
            }));

            let a = app.clone();
            r.append(&ui::button("Remove", move || {
                a.edit(move |m| {
                    if i < m.cfg.custom_services.len() {
                        m.cfg.custom_services.remove(i);
                    }
                });
                a.rebuild(Page::System);
            }));

            item_box.append(&r);

            let src_desc = if let Some(f) = &cs.file {
                format!("file: {f}")
            } else if let Some(content) = &cs.content {
                format!("inline ({} bytes)", content.len())
            } else {
                "empty".to_string()
            };
            let sub_r = ui::hbox(8);
            let indent = gtk::Label::builder().width_request(16).build();
            sub_r.append(&indent);
            let src_label = ui::dim(&src_desc);
            src_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            src_label.set_tooltip_text(Some(&src_desc));
            src_label.set_hexpand(true);
            sub_r.append(&src_label);
            item_box.append(&sub_r);

            c.append(&item_box);
        }
    }

    let actions_row = ui::hbox(8);
    let a = app.clone();
    actions_row.append(&ui::button("Add custom service", move || {
        open_custom_service_editor(&a, None, config::CustomService::default());
    }));

    let a = app.clone();
    let import_btn = ui::button("Import services from host", move || {
        let discovered = hostcfg::services::discover_local_services();
        let mut added_pkg = 0;
        let mut added_custom = 0;

        a.edit(|m| {
            for s in discovered.system_package_services {
                if !m.cfg.services.contains(&s) {
                    m.cfg.services.push(s);
                    added_pkg += 1;
                }
            }
            for cs in discovered.custom_system_services {
                if !m
                    .cfg
                    .custom_services
                    .iter()
                    .any(|existing| existing.name == cs.name && existing.user == cs.user)
                {
                    m.cfg.custom_services.push(cs);
                    added_custom += 1;
                }
            }
            for mut cs in discovered.custom_user_services {
                if cs.user.is_none() {
                    cs.user = m.cfg.users.first().map(|u| u.name.clone());
                }
                if !m
                    .cfg
                    .custom_services
                    .iter()
                    .any(|existing| existing.name == cs.name && existing.user == cs.user)
                {
                    m.cfg.custom_services.push(cs);
                    added_custom += 1;
                }
            }
        });

        a.st.borrow_mut().services_import_report = Some(format!(
            "Imported {added_pkg} package services and {added_custom} custom services from this system."
        ));
        a.rebuild(Page::System);
    });
    import_btn.set_tooltip_text(Some(
        "Import enabled package services and custom unit files from this computer",
    ));
    actions_row.append(&import_btn);
    c.append(&actions_row);

    list_card(ListCard {
        app,
        content,
        title: "Extra kernel parameters",
        label: "Parameter",
        hint: "One word per line, appended to the installed system's kernel command line.",
        options: lists.4,
        items: cfg.kernel_params.clone(),
        height: 110,
        set: |m, v| m.cfg.kernel_params = v,
        get: |m| m.cfg.kernel_params.clone(),
    });

    // 5. Post-Install Scripts
    let c = ui::card(content, Some("First-boot Scripts"));
    ui::hint(
        &c,
        "Run as root on the installed system. Scripts execute in configured phases (pre-service or post-service).",
    );

    if cfg.scripts.is_empty() {
        ui::hint(&c, "No scripts configured yet.");
    }
    for (i, s) in cfg.scripts.iter().enumerate() {
        let src = match hostcfg::scripts::source(s) {
            hostcfg::scripts::Source::Builtin => "built-in".to_string(),
            hostcfg::scripts::Source::File(f) => format!("file {f}"),
            hostcfg::scripts::Source::Remote { url, .. } => format!("{url} (pinned)"),
        };
        let r = ui::hbox(14);
        r.append(&ui::dim(&format!("{}", i + 1)));
        let id = ui::strong(&s.id);
        id.set_width_request(100);
        r.append(&id);
        let phase = s.phase.as_deref().unwrap_or("post-service");
        let phase_chip = ui::chip(phase, if phase == "pre-service" { "chip-accent" } else { "" });
        r.append(&phase_chip);
        let d = ui::dim(&format!("{src}  {}", s.args.join(" ")));
        d.set_ellipsize(gtk::pango::EllipsizeMode::End);
        d.set_tooltip_text(Some(&format!("{src}  {} (phase: {phase})", s.args.join(" "))));
        d.set_hexpand(true);
        r.append(&d);
        let a = app.clone();
        r.append(&ui::button("Remove", move || {
            a.edit(move |m| {
                m.cfg.scripts.remove(i);
            });
            a.rebuild(Page::System);
        }));
        c.append(&r);
    }

    let c = ui::card(content, Some("Built-in scripts"));
    for b in hostcfg::scripts::BUILTINS {
        let r = ui::hbox(10);
        let id = ui::strong(b.id);
        id.set_hexpand(true);
        r.append(&id);
        let (a, bid, bphase) = (app.clone(), b.id, b.phase);
        r.append(&ui::button("Add", move || {
            a.edit(move |m| {
                if !m.cfg.scripts.iter().any(|s| s.id == bid) {
                    m.cfg.scripts.push(config::Script {
                        id: bid.into(),
                        phase: Some(bphase.into()),
                        ..Default::default()
                    });
                }
            });
            a.rebuild(Page::System);
        }));
        c.append(&r);
        c.append(
            &gtk::Label::builder()
                .label(b.description)
                .xalign(0.0)
                .wrap(true)
                .selectable(true)
                .build(),
        );
        ui::hint(
            &c,
            &format!(
                "runs as {}, {}; needs: {}",
                b.runs_as,
                b.phase,
                if b.requires.is_empty() { "nothing" } else { b.requires }
            ),
        );
    }

    let c = ui::card(content, Some("Custom script"));
    ui::hint(
        &c,
        "A local file, or an https URL pinned by its SHA-256 digest.",
    );
    let id = ui::entry("", |_| {});
    ui::row(&c, "Id", &id);
    let file = ui::entry("", |_| {});
    let fr = ui::row(&c, "Local file", &file);
    let url = ui::entry("", |_| {});
    ui::row(&c, "URL", &url);
    let sha = ui::entry("", |_| {});
    ui::row(&c, "SHA-256", &sha);
    let digest = ui::dim("");
    c.append(&digest);
    let ack = ui::check_button("I understand a custom script runs as root on the installed system");
    let add = ui::primary("Add custom script", || {});
    add.set_sensitive(false);
    let update = {
        let (id, file, url, ack, add, digest) = (
            id.clone(),
            file.clone(),
            url.clone(),
            ack.clone(),
            add.clone(),
            digest.clone(),
        );
        Rc::new(move || {
            let path = file.text().to_string();
            let text = if path.is_empty() {
                None
            } else {
                std::fs::read(&path).ok()
            };
            digest.set_text(&match &text {
                Some(t) => {
                    use sha2::Digest;
                    format!(
                        "sha256 {}",
                        sha2::Sha256::digest(t)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    )
                }
                None => String::new(),
            });
            let valid = !id.text().is_empty() && (!path.is_empty() || !url.text().is_empty());
            add.set_sensitive(valid && ack.is_active());
        })
    };
    for e in [&id, &file, &url] {
        let u = update.clone();
        e.connect_changed(move |_| u());
    }
    {
        let u = update.clone();
        ack.connect_toggled(move |_| u());
    }
    {
        let (a, file) = (app.clone(), file.clone());
        fr.append(&ui::button("Browse…", move || {
            let (a, file) = (a.clone(), file.clone());
            glib::spawn_future_local(async move {
                if let Some(p) = dialogs::open_file(a.win.upcast_ref(), "Pick a script", None).await
                {
                    file.set_text(&p.display().to_string());
                }
            });
        }));
    }
    let is_pre_service = Rc::new(std::cell::Cell::new(false));
    let pre_box = {
        let p = is_pre_service.clone();
        ui::check("Run in pre-service phase (before enabling services)", false, move |on| p.set(on))
    };
    c.append(&pre_box);
    c.append(&ack);
    {
        let a = app.clone();
        let is_pre = is_pre_service.clone();
        add.connect_clicked(move |_| {
            let (f, u, s) = (
                file.text().to_string(),
                url.text().to_string(),
                sha.text().to_string(),
            );
            let phase = if is_pre.get() { "pre-service" } else { "post-service" };
            let script = config::Script {
                id: id.text().to_string(),
                file: (!f.is_empty()).then(|| f.clone()),
                url: (f.is_empty() && !u.is_empty()).then(|| u.clone()),
                sha256: (f.is_empty() && !s.is_empty()).then(|| s.clone()),
                phase: Some(phase.into()),
                ..Default::default()
            };
            a.edit(move |m| m.cfg.scripts.push(script));
            a.rebuild(Page::System);
        });
    }
    c.append(&add);

    // 6. Filesystem Overlays
    let c = ui::card(content, Some("System Overlays & Hardware Configurations"));
    ui::hint(
        &c,
        "System overlays embed files or directories into root (e.g. Wi-Fi connections, SSH keys, custom services).",
    );

    if cfg.system_files.is_empty() {
        ui::hint(&c, "No system overlay files configured.");
    } else {
        let count_lbl = ui::dim(&format!("{} overlay item(s) configured.", cfg.system_files.len()));
        c.append(&count_lbl);

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Filter overlay files…")
            .build();
        c.append(&search_entry);

        let list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();

        for (idx, sf) in cfg.system_files.iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            let row_box = ui::hbox(8);
            row_box.set_margin_top(4);
            row_box.set_margin_bottom(4);
            row_box.set_margin_start(8);
            row_box.set_margin_end(8);

            let category = if sf.path.starts_with("etc/NetworkManager") || sf.path.starts_with("var/lib/iwd") {
                "Network"
            } else if sf.path.starts_with("var/lib/bluetooth") {
                "Bluetooth"
            } else if sf.path.starts_with("etc/ssh") {
                "SSH"
            } else if sf.path.starts_with("var/lib/fprint") || sf.path.starts_with("etc/pam.d") {
                "Biometric"
            } else if sf.path.starts_with("etc/systemd/zram") {
                "Zram"
            } else if sf.file.as_ref().is_some_and(|f| std::path::Path::new(f).is_dir()) {
                "Directory"
            } else {
                "Custom"
            };
            row_box.append(&ui::chip(category, "chip-accent"));

            let display_name = if sf.path.is_empty() {
                sf.file.as_deref().unwrap_or("/")
            } else {
                &sf.path
            };
            let p_lbl = ui::strong(display_name);
            p_lbl.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            p_lbl.set_width_chars(16);
            p_lbl.set_tooltip_text(Some(display_name));
            p_lbl.set_hexpand(true);
            row_box.append(&p_lbl);

            let mode_tag = ui::dim(&format!("mode: {:04o}", sf.mode));
            row_box.append(&mode_tag);

            if let Some(content) = &sf.content {
                row_box.append(&ui::dim(&format!("{} B", content.len())));
            } else if let Some(file) = &sf.file {
                let f_lbl = ui::dim(&format!("src: {file}"));
                f_lbl.set_ellipsize(gtk::pango::EllipsizeMode::Start);
                f_lbl.set_tooltip_text(Some(file));
                row_box.append(&f_lbl);
            }

            let a = app.clone();
            row_box.append(&ui::button("Remove", move || {
                a.edit(move |m| {
                    if idx < m.cfg.system_files.len() {
                        m.cfg.system_files.remove(idx);
                    }
                });
                a.rebuild_all();
            }));

            row.set_child(Some(&row_box));
            list_box.append(&row);
        }

        let filter_text = Rc::new(RefCell::new(String::new()));
        {
            let ft = filter_text.clone();
            list_box.set_filter_func(move |row| {
                let query = ft.borrow().trim().to_lowercase();
                if query.is_empty() {
                    return true;
                }
                if let Some(box_row) = row.child().and_downcast::<gtk::Box>() {
                    let mut found = false;
                    let mut child = box_row.first_child();
                    while let Some(w) = child {
                        if let Some(lbl) = w.downcast_ref::<gtk::Label>() {
                            if lbl.text().to_lowercase().contains(&query) {
                                found = true;
                                break;
                            }
                        }
                        child = w.next_sibling();
                    }
                    return found;
                }
                true
            });
        }
        {
            let ft = filter_text.clone();
            let lb = list_box.clone();
            search_entry.connect_search_changed(move |e| {
                *ft.borrow_mut() = e.text().to_string();
                lb.invalidate_filter();
            });
        }

        let scroll = gtk::ScrolledWindow::builder()
            .min_content_height(140)
            .max_content_height(240)
            .has_frame(true)
            .child(&list_box)
            .build();
        c.append(&scroll);
    }

    let act_box = ui::hbox(8);
    let a = app.clone();
    let auth_autoload_btn = ui::button(
        "Import hardware & network credentials (sudo/root)",
        move || {
            crate::pages::users::open_shadow_auth_dialog(&a);
        },
    );
    auth_autoload_btn.set_tooltip_text(Some(
        "Detects NetworkManager/iwd connections, Bluetooth pairings, SSH host keys, PAM fprint, and zram config.",
    ));
    act_box.append(&auth_autoload_btn);

    let a = app.clone();
    let add_dir_btn = ui::button("Add directory overlay…", move || {
        let a = a.clone();
        glib::spawn_future_local(async move {
            if let Some(path) = crate::dialogs::open_folder(a.win.upcast_ref(), "Select overlay directory", None).await {
                a.edit(|m| {
                    m.cfg.system_files.push(config::SystemFile {
                        path: "".into(),
                        mode: 0o644,
                        file: Some(path.to_string_lossy().to_string()),
                        content: None,
                    });
                });
                a.set_status(format!("Added directory overlay: {}", path.display()));
                a.rebuild_all();
            }
        });
    });
    add_dir_btn.set_tooltip_text(Some("Select a local directory whose files will be recursively merged into root /"));
    act_box.append(&add_dir_btn);

    let a = app.clone();
    let add_file_btn = ui::button("Add file…", move || {
        let a = a.clone();
        glib::spawn_future_local(async move {
            if let Some(path) = crate::dialogs::open_file(a.win.upcast_ref(), "Select file to overlay", None).await {
                let target_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "custom.conf".into());
                a.edit(|m| {
                    m.cfg.system_files.push(config::SystemFile {
                        path: format!("etc/{target_name}"),
                        mode: 0o644,
                        file: Some(path.to_string_lossy().to_string()),
                        content: None,
                    });
                });
                a.set_status(format!("Added file overlay: {}", path.display()));
                a.rebuild_all();
            }
        });
    });
    act_box.append(&add_file_btn);

    if !cfg.system_files.is_empty() {
        let a = app.clone();
        let clear_btn = ui::button("Clear all", move || {
            a.edit(|m| m.cfg.system_files.clear());
            a.set_status("Cleared all system overlay files.");
            a.rebuild_all();
        });
        act_box.append(&clear_btn);
    }
    c.append(&act_box);
}
