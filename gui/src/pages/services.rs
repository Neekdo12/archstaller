use crate::app::{App, Page};
use crate::model::Model;
use crate::ui;
use gtk::prelude::*;
use std::rc::Rc;

/// A list card: an "add" field with suggestions on top of the list editor.
fn list_card(app: &Rc<App>, content: &gtk::Box, title: &str, label: &str, hint: &str, options: Rc<Vec<String>>, items: Vec<String>, height: i32, set: fn(&mut Model, Vec<String>), get: fn(&Model) -> Vec<String>) {
    let c = ui::card(content, Some(title));
    let add = ui::suggest("", options, false, |_| {});
    let r = ui::row(&c, label, &add);
    let btn = ui::button("Add to the list", || {});
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

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let (cfg, services, params) = {
        let st = app.st.borrow();
        (st.model.cfg.clone(), st.lists.services.clone(), st.lists.kernel_params.clone())
    };
    ui::heading(content, "Services & kernel", "What starts on the first boot and how the kernel is started.");
    list_card(app, content, "Services enabled on first boot", "Unit", "One systemd unit per line, for example sshd.service.", services, cfg.services.clone(), 130, |m, v| m.cfg.services = v, |m| m.cfg.services.clone());
    list_card(app, content, "Extra kernel parameters", "Parameter", "One word per line, appended to the installed system's kernel command line.", params, cfg.kernel_params.clone(), 110, |m, v| m.cfg.kernel_params = v, |m| m.cfg.kernel_params.clone());
}
