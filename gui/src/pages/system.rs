use crate::app::App;
use crate::ui;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let (cfg, lists) = {
        let st = app.st.borrow();
        (
            st.model.cfg.clone(),
            (
                st.lists.timezones.clone(),
                st.lists.locales.clone(),
                st.lists.keymaps.clone(),
            ),
        )
    };
    ui::heading(
        content,
        "System",
        "Name and localization of the installed system.",
    );
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

    let c = ui::card(content, Some("Language"));
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

    let c = ui::card(content, Some("System Overlays & Hardware Configurations"));
    ui::hint(
        &c,
        "System overlays embed files or directories into root (e.g. Wi-Fi connections, SSH keys, custom services).",
    );

    if cfg.system_files.is_empty() {
        ui::hint(
            &c,
            "No system overlay files configured.",
        );
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
                    m.cfg.system_files.remove(idx);
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
