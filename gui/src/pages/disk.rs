use crate::app::App;
use crate::dialogs;
use crate::ui::{self, Kind};
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let cfg = app.st.borrow().model.cfg.clone();
    ui::heading(content, "Target disk", "The installer erases the disk it selects. Name it by serial number, or let it take the largest one.");
    let c = ui::card(content, Some("Which disk"));

    let warn = ui::note(&c, Kind::Warn, "Every machine that boots this ISO loses its largest disk.");
    warn.set_visible(cfg.disk.auto_largest);
    let selectors = ui::vbox(8);

    let auto = gtk::CheckButton::with_label("Erase and install onto the largest disk, without asking");
    auto.set_active(cfg.disk.auto_largest);
    c.insert_child_after(&auto, c.first_child().as_ref());
    {
        let a = app.clone();
        let warn = warn.clone();
        let selectors = selectors.clone();
        auto.connect_toggled(move |btn| {
            let on = btn.is_active();
            if on && !a.st.borrow().model.cfg.disk.auto_largest {
                // Irreversible for every machine that boots the ISO: ask first, and undo the toggle on "no".
                let a2 = a.clone();
                let btn = btn.clone();
                let (warn, selectors) = (warn.clone(), selectors.clone());
                glib::spawn_future_local(async move {
                    let ok = dialogs::confirm(a2.win.upcast_ref(), "Erase the largest disk without asking?", "With this option the installer wipes and installs onto the largest disk of whatever machine boots this ISO, with no confirmation. Use it only for machines you intend to erase.", "Yes, erase").await;
                    if ok {
                        a2.edit(|m| m.cfg.disk.auto_largest = true);
                        warn.set_visible(true);
                        selectors.set_sensitive(false);
                    } else {
                        btn.set_active(false);
                    }
                });
            } else if !on {
                a.edit(|m| m.cfg.disk.auto_largest = false);
                warn.set_visible(false);
                selectors.set_sensitive(true);
            }
        });
    }

    c.append(&selectors);
    selectors.set_sensitive(!cfg.disk.auto_largest);
    let a = app.clone();
    ui::row(&selectors, "Serial", &ui::entry(&cfg.disk.confirm_serial, move |t| {
        let t = t.to_string();
        a.edit(move |m| m.cfg.disk.confirm_serial = t);
    }));
    let model_row = ui::hbox(8);
    let model_entry = ui::entry(cfg.disk.model.as_deref().unwrap_or(""), {
        let a = app.clone();
        move |t| {
            let t = t.to_string();
            a.edit(move |m| {
                if m.cfg.disk.model.is_some() {
                    m.cfg.disk.model = Some(t);
                }
            });
        }
    });
    model_entry.set_sensitive(cfg.disk.model.is_some());
    let has = ui::check("must also contain", cfg.disk.model.is_some(), {
        let a = app.clone();
        let e = model_entry.clone();
        move |on| {
            let text = e.text().to_string();
            e.set_sensitive(on);
            a.edit(move |m| m.cfg.disk.model = on.then_some(text));
        }
    });
    model_row.append(&has);
    model_row.append(&model_entry);
    ui::row(&selectors, "Model", &model_row);

    let c = ui::card(content, Some("Layout"));
    let a = app.clone();
    let sp = ui::spin(cfg.disk.esp_mib, 64, 8192, move |v| a.edit(move |m| m.cfg.disk.esp_mib = v));
    let r = ui::hbox(6);
    r.append(&sp);
    r.append(&ui::dim("MiB"));
    ui::row(&c, "ESP size", &r);
    ui::hint(&c, "The FAT32 boot partition, mounted at /boot. The rest of the disk is the root file system.");
}
