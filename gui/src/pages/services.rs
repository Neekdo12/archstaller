use crate::app::{App, Page};
use crate::dialogs;
use crate::model::Model;
use crate::ui::{self, Kind};
use gtk::prelude::*;
use sourceview5::prelude::*;
use std::rc::Rc;

/// A list card: an "add" field with suggestions on top of the list editor.
fn list_card(
    app: &Rc<App>,
    content: &gtk::Box,
    title: &str,
    label: &str,
    hint: &str,
    options: Rc<Vec<String>>,
    items: Vec<String>,
    height: i32,
    set: fn(&mut Model, Vec<String>),
    get: fn(&Model) -> Vec<String>,
) {
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
            a.rebuild(Page::Services);
        });
    }
    let a = app.clone();
    c.append(&ui::lines_editor(&items, height, move |v| a.edit(move |m| set(m, v))));
    ui::hint(&c, hint);
}

/// Modal dialog to edit or create a custom service unit.
fn open_custom_service_editor(
    app: &Rc<App>,
    existing_index: Option<usize>,
    initial: config::CustomService,
) {
    let (w, h) = (
        app.win.width().min(680).max(360),
        app.win.height().min(540).max(360),
    );
    let dialog = gtk::Window::builder()
        .title(if existing_index.is_some() { "Edit Custom Service" } else { "New Custom Service" })
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

    // Name entry
    let name_entry = ui::entry(&initial.name, |_| {});
    ui::row(&root, "Unit name", &name_entry);
    ui::hint(&root, "e.g. rclone-onedrive.service, backup.timer, my-daemon.service");

    // Target user dropdown
    let users = app.st.borrow().model.cfg.users.clone();
    let mut user_options = vec!["root (machine-wide)".to_string()];
    for u in &users {
        user_options.push(format!("user: {}", u.name));
    }
    let user_strs: Vec<&str> = user_options.iter().map(|s| s.as_str()).collect();
    let user_dd = gtk::DropDown::from_strings(&user_strs);
    let selected_user_idx = match &initial.user {
        Some(u) if u != "root" => users.iter().position(|usr| usr.name == *u).map(|p| p + 1).unwrap_or(0),
        _ => 0,
    };
    user_dd.set_selected(selected_user_idx as u32);
    ui::row(&root, "Target user", &user_dd);

    // Enable on first boot
    let enable_check = ui::check("Enable on first boot", initial.enable, |_| {});
    root.append(&enable_check);

    // Source selection: Inline content vs Local file
    let is_file_initial = initial.file.is_some();
    let mode_box = ui::hbox(16);
    let inline_radio = gtk::CheckButton::builder().label("Inline unit content").active(!is_file_initial).build();
    let file_radio = gtk::CheckButton::builder().label("Local file path").group(&inline_radio).active(is_file_initial).build();
    mode_box.append(&inline_radio);
    mode_box.append(&file_radio);
    root.append(&mode_box);

    // Local file container
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
                if let Some(path) = dialogs::open_file(&win, "Select Systemd Unit File", None).await {
                    fe.set_text(&path.to_string_lossy());
                }
            });
        }
    });
    file_row.append(&file_entry);
    file_row.append(&browse_btn);
    file_box.append(&file_row);
    ui::hint(&file_box, "Path to a local unit file (relative to the config file).");
    root.append(&file_box);

    // Inline sourceview container
    let inline_box = ui::vbox(6);
    let dark = ui::is_dark(&app.win);
    let buf = sourceview5::Buffer::new(None);
    if let Some(lang) = sourceview5::LanguageManager::default().language("ini") {
        buf.set_language(Some(&lang));
    }
    buf.set_highlight_syntax(true);
    let mgr = sourceview5::StyleSchemeManager::default();
    let scheme = [if dark { "Adwaita-dark" } else { "Adwaita" }, if dark { "classic-dark" } else { "classic" }].iter().find_map(|n| mgr.scheme(n));
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

    // Visibility toggle based on radio buttons
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

    // Error note
    let err_label = ui::note(&root, Kind::Bad, "");
    err_label.set_visible(false);

    // Buttons
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
                err_label.set_text("Invalid unit name (must end in .service, .timer, etc., without slashes).");
                err_label.set_visible(true);
                return;
            }

            let user_idx = user_dd.selected() as usize;
            let user = if user_idx == 0 {
                None
            } else if let Some(u) = users.get(user_idx - 1) {
                Some(u.name.clone())
            } else {
                None
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
                    if idx < m.cfg.custom_services.len() {
                        m.cfg.custom_services[idx] = service;
                    }
                } else {
                    m.cfg.custom_services.push(service);
                }
            });
            app.rebuild(Page::Services);
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
    let (cfg, services, params, report) = {
        let st = app.st.borrow();
        (
            st.model.cfg.clone(),
            st.lists.services.clone(),
            st.lists.kernel_params.clone(),
            st.services_import_report.clone(),
        )
    };

    ui::heading(content, "Services & kernel", "What starts on the first boot and how the kernel is started.");

    // Import report banner (if an import just ran)
    if let Some(rep) = report {
        let banner = ui::card(content, None);
        ui::note(&banner, Kind::Ok, &rep);
        let a = app.clone();
        banner.append(&ui::button("Dismiss", move || {
            a.st.borrow_mut().services_import_report = None;
            a.rebuild(Page::Services);
        }));
    }

    // 1. Package services card
    list_card(
        app,
        content,
        "Services enabled on first boot",
        "Unit",
        "One systemd unit per line, for example sshd.service.",
        services,
        cfg.services.clone(),
        130,
        |m, v| m.cfg.services = v,
        |m| m.cfg.services.clone(),
    );

    // 2. Custom services card
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
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            name.set_tooltip_text(Some(&cs.name));
            name.set_hexpand(true);
            r.append(&name);

            // User scope chip
            let user_str = cs.user.as_deref().unwrap_or("root");
            let user_chip = ui::chip(&format!("user: {user_str}"), if user_str == "root" { "chip-accent" } else { "" });
            r.append(&user_chip);

            // Status chip
            let status_chip = if cs.enable {
                ui::chip("enabled", "chip-ok")
            } else {
                ui::chip("disabled", "chip-warn")
            };
            r.append(&status_chip);

            // Edit button
            let a = app.clone();
            let cs_clone = cs.clone();
            r.append(&ui::button("Edit", move || {
                open_custom_service_editor(&a, Some(i), cs_clone.clone());
            }));

            // Remove button
            let a = app.clone();
            r.append(&ui::button("Remove", move || {
                a.edit(move |m| {
                    if i < m.cfg.custom_services.len() {
                        m.cfg.custom_services.remove(i);
                    }
                });
                a.rebuild(Page::Services);
            }));

            item_box.append(&r);

            // Source description row (indented under the unit name)
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

    // Action buttons for custom services
    let actions_row = ui::hbox(8);
    let a = app.clone();
    actions_row.append(&ui::button("Add custom service", move || {
        open_custom_service_editor(&a, None, config::CustomService::default());
    }));

    // Import from local machine button
    let a = app.clone();
    let import_btn = ui::button("Import from host", move || {
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
                if !m.cfg.custom_services.iter().any(|existing| existing.name == cs.name && existing.user == cs.user) {
                    m.cfg.custom_services.push(cs);
                    added_custom += 1;
                }
            }
            for mut cs in discovered.custom_user_services {
                if cs.user.is_none() {
                    cs.user = m.cfg.users.first().map(|u| u.name.clone());
                }
                if !m.cfg.custom_services.iter().any(|existing| existing.name == cs.name && existing.user == cs.user) {
                    m.cfg.custom_services.push(cs);
                    added_custom += 1;
                }
            }
        });

        a.st.borrow_mut().services_import_report = Some(format!(
            "Imported {added_pkg} package services and {added_custom} custom services from this system."
        ));
        a.rebuild(Page::Services);
    });
    import_btn.set_tooltip_text(Some("Import enabled package services and custom unit files from this computer"));
    actions_row.append(&import_btn);
    c.append(&actions_row);

    // 3. Extra kernel parameters card
    list_card(
        app,
        content,
        "Extra kernel parameters",
        "Parameter",
        "One word per line, appended to the installed system's kernel command line.",
        params,
        cfg.kernel_params.clone(),
        110,
        |m, v| m.cfg.kernel_params = v,
        |m| m.cfg.kernel_params.clone(),
    );
}
