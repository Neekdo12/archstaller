use crate::app::{App, Page};
use crate::ui;
use gtk::prelude::*;
use hostcfg::host::{DriverClass, DRIVERS};
use std::rc::Rc;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let (host, tethering) = {
        let st = app.st.borrow();
        (st.model.host.clone(), st.model.host.build.tethering)
    };
    ui::heading(content, "Build & drivers", "How the installer ISO itself is built. These settings are not part of the installed system.");

    let c = ui::card(content, Some("Build profile"));
    let current = host.build.profile.clone().unwrap_or_else(|| "super-small".into());
    let small = gtk::CheckButton::with_label("super-small");
    let large = gtk::CheckButton::with_label("large");
    let xl = gtk::CheckButton::with_label("extra-large");
    large.set_group(Some(&small));
    xl.set_group(Some(&small));
    xl.set_sensitive(false);
    small.set_active(current != "large");
    large.set_active(current == "large");
    for (btn, name) in [(&small, "super-small"), (&large, "large")] {
        let a = app.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                a.edit(move |m| m.set_profile(name));
            }
        });
    }
    c.append(&small);
    ui::hint(&c, "Size-optimized installer (the default), about 0.7 MiB. A crash shows no panic message.");
    c.append(&large);
    ui::hint(&c, "The regular release build, about 0.8 MiB, with panic messages kept for debugging.");
    c.append(&xl);
    ui::hint(&c, "Reserved, not available yet.");

    let c = ui::card(content, Some("USB tethering"));
    let a = app.clone();
    c.append(&ui::check("Include USB tethering (iPhone, Android, USB Ethernet)", tethering, move |on| a.edit(move |m| m.host.build.tethering = on)));
    ui::hint(&c, "Adds about 100 KiB. Lets the installer use a phone's hotspot when no wired network has a link.");

    let c = ui::card(content, Some("Installer drivers"));
    ui::hint(&c, "Drivers of the installer itself, not packages for the installed system. Leave them all on unless you know the hardware.");
    let all = host.installer_drivers.is_none();
    let a = app.clone();
    c.append(&ui::check("All drivers", all, move |on| {
        a.edit(move |m| m.host.installer_drivers = if on { None } else { Some(DRIVERS.iter().map(|d| d.id.to_string()).collect()) });
        a.rebuild(Page::Build);
    }));
    if let Some(list) = host.installer_drivers.clone() {
        for (class, title) in [(DriverClass::Storage, "Storage"), (DriverClass::Network, "Network")] {
            c.append(&ui::strong(title));
            for d in DRIVERS.iter().filter(|d| d.class == class) {
                let r = ui::hbox(12);
                let id = d.id;
                let a = app.clone();
                let on = list.iter().any(|i| i == id);
                let cb = ui::check(id, on, move |on| {
                    a.edit(move |m| {
                        let l = m.host.installer_drivers.get_or_insert_with(Vec::new);
                        if on {
                            if !l.iter().any(|i| i == id) {
                                l.push(id.to_string());
                            }
                        } else {
                            l.retain(|i| i != id);
                        }
                    });
                });
                cb.set_width_request(130);
                r.append(&cb);
                let desc = gtk::Label::builder().label(d.description).xalign(0.0).hexpand(true).wrap(true).build();
                r.append(&desc);
                r.append(&ui::dim(d.status));
                c.append(&r);
            }
        }
    }
}
