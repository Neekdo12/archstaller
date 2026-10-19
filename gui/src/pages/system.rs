use crate::app::{App, Page};
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

    let users = app.st.borrow().model.cfg.users.clone();
    let mut user_names = vec!["System (root)".to_string()];
    for u in &users {
        user_names.push(format!("User: {}", u.name));
    }
    let user_dd = ui::dropdown(&user_names, |_| {});
    let selected_user_idx = match &initial.user {
        None => 0,
        Some(name) => users
            .iter()
            .position(|u| u.name == *name)
            .map(|i| i + 1)
            .unwrap_or(0),
    };
    user_dd.set_selected(selected_user_idx as u32);
    ui::row(&root, "Scope", &user_dd);

    let enable_check = ui::check_button("Enable on boot (systemctl enable)");
    enable_check.set_active(initial.enable);
    root.append(&enable_check);

    let source_box = ui::vbox(6);
    let inline_radio = gtk::CheckButton::builder()
        .label("Inline unit definition (embedded in config)")
        .build();
    let file_radio = gtk::CheckButton::builder()
        .label("Local file reference (path on this machine)")
        .group(&inline_radio)
        .build();

    let is_inline = initial.content.is_some() || initial.file.is_none();
    inline_radio.set_active(is_inline);
    file_radio.set_active(!is_inline);

    source_box.append(&inline_radio);
    source_box.append(&file_radio);
    ui::row(&root, "Source", &source_box);

    let file_box = ui::vbox(6);
    let file_entry = ui::entry(initial.file.as_deref().unwrap_or(""), |_| {});
    let file_row = ui::hbox(8);
    file_row.append(&file_entry);
    let pick_btn = ui::button("Browse…", {
        let entry = file_entry.clone();
        let app = app.clone();
        move || {
            let e = entry.clone();
            let a = app.clone();
            glib::spawn_future_local(async move {
                if let Some(path) =
                    crate::dialogs::open_file(a.win.upcast_ref(), "Select Service File", None).await
                {
                    e.set_text(&path.to_string_lossy());
                }
            });
        }
    });
    file_row.append(&pick_btn);
    file_box.append(&file_row);
    file_box.append(&ui::dim(
        "Unit file copied into the installer image at build time.",
    ));
    root.append(&file_box);

    let inline_box = ui::vbox(6);
    inline_box.append(&ui::dim(
        "Unit file content written into the target system on first boot:",
    ));

    let buf = sourceview5::Buffer::new(None);
    let text = if let Some(bytes) = &initial.content {
        String::from_utf8_lossy(bytes).to_string()
    } else {
        "[Unit]\nDescription=My Custom Service\nAfter=network.target\n\n[Service]\nType=oneshot\nExecStart=/usr/bin/true\n\n[Install]\nWantedBy=multi-user.target\n"
            .to_string()
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
    let (cfg, lists, services, params, report) = {
        let st = app.st.borrow();
        (
            st.model.cfg.clone(),
            (
                st.lists.timezones.clone(),
                st.lists.locales.clone(),
                st.lists.keymaps.clone(),
            ),
            st.lists.services.clone(),
            st.lists.kernel_params.clone(),
            st.services_import_report.clone(),
        )
    };
    ui::heading(
        content,
        "System & Services",
        "Identity, localization, root account, display manager, services, scripts, and overlays.",
    );

    // 1. Identity
    let c = ui::card(content, Some("Identity"));
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
    let auto_id_btn = ui::button("Autoload identity & localization from system", move || {
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

    // 2. Language & Keyboard
    let c = ui::card(content, Some("Language & Keyboard"));
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
    let auto_lang_btn = ui::button("Autoload language & keymap from system", move || {
        let sys = hostcfg::system::discover_host_system();
        a.edit(|m| {
            m.cfg.locale = sys.locale.clone();
            m.cfg.keymap = sys.keymap.clone();
        });
        a.set_status("Autoloaded locale and keymap from host system.");
        a.rebuild_all();
    });
    auto_lang_btn.set_tooltip_text(Some(
        "Detects system/desktop locale (LANG/LC_ALL) and keyboard layout (Hyprland, localectl, vconsole.conf).",
    ));
    c.append(&auto_lang_btn);

    // 3. Root Account
    let c = ui::card(content, Some("Root Account"));
    let hash = Rc::new(RefCell::new(
        cfg.root_password_hash.clone().unwrap_or_default(),
    ));
    let hash_row = ui::vbox(6);
    let he = ui::entry(&hash.borrow(), {
        let (a, hash) = (app.clone(), hash.clone());
        move |t| {
            *hash.borrow_mut() = t.to_string();
            let t = t.to_string();
            a.edit(move |m| m.cfg.root_password_hash = Some(t));
        }
    });
    ui::row(&hash_row, "Password hash", &he);
    ui::hint(
        &hash_row,
        "A $6$ SHA-512 crypt hash, for example from `openssl passwd -6`.",
    );
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
    let a = app.clone();
    let root_auth_btn = ui::button("Retrieve root hash from system (sudo / root)", move || {
        crate::pages::users::open_shadow_auth_dialog(&a);
    });
    root_auth_btn.set_tooltip_text(Some(
        "Authenticates via sudo password, root password, or Polkit to read root's crypt hash from /etc/shadow.",
    ));
    c.append(&root_auth_btn);

    // 4. Display Manager
    let c = ui::card(content, Some("Display Manager"));
    ui::hint(
        &c,
        "Login manager enabled on first boot to start graphical or console sessions.",
    );
    let dm_options = [
        ("none", "None (console login prompt)"),
        ("ly@tty2.service", "Ly (lightweight console login manager on tty2)"),
        ("sddm.service", "SDDM (KDE Plasma & modern Wayland)"),
        ("gdm.service", "GDM (GNOME Display Manager)"),
        ("lightdm.service", "LightDM (Xorg desktop sessions)"),
        ("greetd.service", "greetd / tuigreet (modern terminal greeter)"),
    ];
    let dm_services = [
        "ly@tty2.service",
        "sddm.service",
        "gdm.service",
        "lightdm.service",
        "greetd.service",
    ];
    let current_dm = dm_services
        .iter()
        .find(|&&s| cfg.services.iter().any(|v| v == s))
        .copied()
        .unwrap_or("none");

    let mut dm_group_leader: Option<gtk::CheckButton> = None;
    for (unit, desc) in dm_options {
        let b = if let Some(ref leader) = dm_group_leader {
            gtk::CheckButton::builder()
                .label(desc)
                .group(leader)
                .build()
        } else {
            let leader = gtk::CheckButton::builder().label(desc).build();
            dm_group_leader = Some(leader.clone());
            leader
        };
        b.set_active(current_dm == unit);
        let a = app.clone();
        b.connect_toggled(move |btn| {
            if btn.is_active() {
                a.edit(move |m| {
                    for dm_svc in &dm_services {
                        m.cfg.services.retain(|s| s != *dm_svc);
                    }
                    if unit != "none" {
                        m.cfg.services.push(unit.to_string());
                    }
                });
            }
        });
        c.append(&b);
    }

    // 5. System Services & Kernel Parameters
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
        hint: "One systemd unit per line, for example sshd.service.",
        options: services,
        items: cfg.services.clone(),
        height: 130,
        set: |m, v| m.cfg.services = v,
        get: |m| m.cfg.services.clone(),
    });

    let c = ui::card(content, Some("Custom systemd services"));
    ui::hint(&c, "Custom unit files installed into /etc/systemd/system/ (machine-wide) or ~/.config/systemd/user/ (per-user).");

    if cfg.custom_services.is_empty() {
        ui::hint(&c, "No custom services yet.");
    } else {
        for (i, cs) in cfg.custom_services.iter().enumerate() {
            let item_box = ui::vbox(3);
            let r = ui::hbox(8);
            r.append(&ui::dim(&format!("{}", i + 1)));

            let name = ui::strong(&cs.name);
            name.set_width_request(120);
            r.append(&name);

            let user_str = cs.user.as_deref().unwrap_or("root");
            let user_chip = ui::chip(
                user_str,
                if user_str == "root" {
                    "chip-accent"
                } else {
                    ""
                },
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
        options: params,
        items: cfg.kernel_params.clone(),
        height: 110,
        set: |m, v| m.cfg.kernel_params = v,
        get: |m| m.cfg.kernel_params.clone(),
    });

    // 6. Post-Install Scripts
    let c = ui::card(content, Some("First-boot scripts"));
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
                if i < m.cfg.scripts.len() {
                    m.cfg.scripts.remove(i);
                }
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
                if b.requires.is_empty() {
                    "nothing"
                } else {
                    b.requires
                }
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
    let pick = ui::button("Browse…", {
        let file = file.clone();
        let a = app.clone();
        move || {
            let file = file.clone();
            let a = a.clone();
            glib::spawn_future_local(async move {
                if let Some(p) =
                    crate::dialogs::open_file(a.win.upcast_ref(), "Select a shell script", None).await
                {
                    file.set_text(&p.to_string_lossy());
                }
            });
        }
    });
    fr.append(&pick);

    let url = ui::entry("", |_| {});
    ui::row(&c, "Or https URL", &url);
    let sha = ui::entry("", |_| {});
    ui::row(&c, "Pinned SHA-256", &sha);
    let args = ui::entry("", |_| {});
    ui::row(&c, "Arguments", &args);

    let phases = ["post-service", "pre-service"];
    let phase_dd = ui::dropdown(&phases, |_| {});
    ui::row(&c, "Execution Phase", &phase_dd);

    let r = ui::row(&c, "", &ui::dim(""));
    let a = app.clone();
    let add_btn = ui::button("Add custom script", move || {
        let id_val = id.text().trim().to_string();
        if id_val.is_empty() {
            return;
        }
        let file_val = file.text().trim().to_string();
        let url_val = url.text().trim().to_string();
        let sha_val = sha.text().trim().to_string();
        let args_val: Vec<String> = args
            .text()
            .split_whitespace()
            .map(String::from)
            .collect();
        let phase_val = if phase_dd.selected() == 1 {
            "pre-service".to_string()
        } else {
            "post-service".to_string()
        };

        let script = if !file_val.is_empty() {
            config::Script {
                id: id_val,
                file: Some(file_val),
                args: args_val,
                phase: Some(phase_val),
                ..Default::default()
            }
        } else if !url_val.is_empty() {
            config::Script {
                id: id_val,
                url: Some(url_val),
                sha256: Some(sha_val),
                args: args_val,
                phase: Some(phase_val),
                ..Default::default()
            }
        } else {
            return;
        };

        a.edit(move |m| {
            m.cfg.scripts.push(script);
        });
        a.rebuild(Page::System);
    });
    r.append(&add_btn);

    // 7. System Overlays
    let c = ui::card(content, Some("System Overlays & Hardware Configurations"));
    ui::hint(
        &c,
        "System overlays embed files or directories into root (e.g. Wi-Fi connections, SSH keys, custom services).",
    );

    if cfg.system_files.is_empty() {
        ui::hint(&c, "No system overlay files configured.");
    } else {
        for (idx, sf) in cfg.system_files.iter().enumerate() {
            let row = ui::hbox(8);
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
            row.append(&ui::chip(category, "chip-accent"));

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
            row.append(&p_lbl);

            let mode_tag = ui::dim(&format!("mode: {:04o}", sf.mode));
            row.append(&mode_tag);

            if let Some(content) = &sf.content {
                row.append(&ui::dim(&format!("{} B", content.len())));
            } else if let Some(file) = &sf.file {
                let f_lbl = ui::dim(&format!("src: {file}"));
                f_lbl.set_ellipsize(gtk::pango::EllipsizeMode::Start);
                f_lbl.set_tooltip_text(Some(file));
                row.append(&f_lbl);
            }

            let a = app.clone();
            row.append(&ui::button("Remove", move || {
                a.edit(move |m| {
                    if idx < m.cfg.system_files.len() {
                        m.cfg.system_files.remove(idx);
                    }
                });
                a.rebuild_all();
            }));
            c.append(&row);
        }
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
            if let Some(path) =
                crate::dialogs::open_folder(a.win.upcast_ref(), "Select overlay directory", None).await
            {
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
    add_dir_btn.set_tooltip_text(Some(
        "Select a local directory whose files will be recursively merged into root /",
    ));
    act_box.append(&add_dir_btn);

    let a = app.clone();
    let add_file_btn = ui::button("Add file…", move || {
        let a = a.clone();
        glib::spawn_future_local(async move {
            if let Some(path) =
                crate::dialogs::open_file(a.win.upcast_ref(), "Select file to overlay", None).await
            {
                let target_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "custom.conf".into());
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
