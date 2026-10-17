use crate::app::App;
use crate::ui;
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

    let c = ui::card(content, Some("System Overlays & Hardware Configurations"));
    ui::note(
        &c,
        ui::Kind::Warn,
        "System overlays preserve configurations across reinstall: Wi-Fi/Ethernet network profiles (/etc/NetworkManager/system-connections/, /var/lib/iwd/), fingerprint PAM settings (/etc/pam.d/) and enrolled biometrics (/var/lib/fprint/), and zram swap (/etc/systemd/zram-generator.conf). Files are written directly to the root filesystem during installation and preserved in the first-boot overlay.",
    );

    if cfg.system_files.is_empty() {
        ui::hint(
            &c,
            "No system overlay files configured. Network connections and biometric fingerprint templates will not be restored unless autoloaded.",
        );
    } else {
        for (idx, sf) in cfg.system_files.iter().enumerate() {
            let row = ui::hbox(8);
            let p_lbl = ui::strong(&sf.path);
            p_lbl.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            p_lbl.set_width_chars(10);
            p_lbl.set_tooltip_text(Some(&sf.path));
            p_lbl.set_hexpand(true);
            row.append(&p_lbl);

            let mode_tag = ui::dim(&format!("mode: {:04o}", sf.mode));
            row.append(&mode_tag);

            if let Some(content) = &sf.content {
                row.append(&ui::dim(&format!("{} B", content.len())));
            } else if let Some(file) = &sf.file {
                let f_lbl = ui::dim(&format!("file: {file}"));
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

    let act_box = ui::vbox(8);
    let a = app.clone();
    let auth_autoload_btn = ui::button(
        "Autoload network & biometric settings (sudo / root)",
        move || {
            crate::pages::users::open_shadow_auth_dialog(&a);
        },
    );
    auth_autoload_btn.set_tooltip_text(Some(
        "Detects NetworkManager/iwd connections, PAM fprint configuration, biometric templates in /var/lib/fprint/, and zram config.",
    ));
    act_box.append(&auth_autoload_btn);

    if !cfg.system_files.is_empty() {
        let a = app.clone();
        let clear_btn = ui::button("Clear all system overlays", move || {
            a.edit(|m| m.cfg.system_files.clear());
            a.set_status("Cleared all system overlay files.");
            a.rebuild_all();
        });
        act_box.append(&clear_btn);
    }
    c.append(&act_box);
}
