use crate::app::{App, Page};
use crate::ui::{self, Kind};
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let (cfg, groups, shells) = {
        let st = app.st.borrow();
        (st.model.cfg.clone(), st.lists.groups.clone(), st.lists.shells.clone())
    };
    ui::heading(content, "Users", "Passwords are stored as SHA-512 crypt hashes only; the plaintext is never saved.");

    let c = ui::card(content, Some("Accounts"));
    if cfg.users.is_empty() {
        ui::hint(&c, "No users yet: the system would have only a locked root account.");
    }
    for (i, u) in cfg.users.iter().enumerate() {
        let r = ui::hbox(14);
        let name = ui::strong(&u.name);
        name.set_width_request(150);
        r.append(&name);
        let g = ui::dim(&format!("groups: {}", u.groups.join(", ")));
        g.set_hexpand(true);
        r.append(&g);
        r.append(&ui::dim(&u.shell));
        let a = app.clone();
        r.append(&ui::button("Remove", move || {
            a.edit(move |m| {
                m.cfg.users.remove(i);
            });
            a.rebuild(Page::Users);
        }));
        c.append(&r);
    }

    let c = ui::card(content, Some("Add a user"));
    let name = ui::entry("", |_| {});
    ui::row(&c, "Name", &name);
    let pw = ui::password(|_| {});
    ui::row(&c, "Password", &pw);
    let pw2 = ui::password(|_| {});
    ui::row(&c, "Again", &pw2);
    let grp = ui::suggest("wheel", groups, true, |_| {});
    ui::row(&c, "Groups", &grp);
    let sh = ui::suggest("/bin/bash", shells, false, |_| {});
    ui::row(&c, "Shell", &sh);
    let mismatch = ui::note(&c, Kind::Bad, "The passwords differ");
    mismatch.set_visible(false);
    let add = ui::primary("Add user", || {});
    add.set_sensitive(false);
    let update = {
        let (name, pw, pw2, mismatch, add) = (name.clone(), pw.clone(), pw2.clone(), mismatch.clone(), add.clone());
        Rc::new(move || {
            let (p1, p2) = (pw.text().to_string(), pw2.text().to_string());
            mismatch.set_visible(!p1.is_empty() && p1 != p2);
            add.set_sensitive(!name.text().is_empty() && !p1.is_empty() && p1 == p2);
        })
    };
    for e in [&name.clone().upcast::<gtk::Editable>(), &pw.clone().upcast::<gtk::Editable>(), &pw2.clone().upcast::<gtk::Editable>()] {
        let u = update.clone();
        e.connect_changed(move |_| u());
    }
    {
        let (a, name, pw, grp, sh) = (app.clone(), name.clone(), pw.clone(), grp.clone(), sh.clone());
        add.connect_clicked(move |_| {
            match hostcfg::password::hash(pw.text().as_str()) {
                Ok(h) => {
                    let groups: Vec<String> = grp.text().split(',').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
                    let user = config::User { name: name.text().to_string(), password_hash: h, groups, shell: sh.text().to_string() };
                    a.edit(move |m| m.cfg.users.push(user));
                    a.rebuild(Page::Users);
                }
                Err(e) => a.set_status(format!("cannot hash the password: {e}")),
            }
        });
    }
    c.append(&add);

    let c = ui::card(content, Some("root account"));
    let hash = Rc::new(RefCell::new(cfg.root_password_hash.clone().unwrap_or_default()));
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
    ui::hint(&hash_row, "A $6$ SHA-512 crypt hash, for example from `openssl passwd -6`.");
    hash_row.set_visible(cfg.root_password_hash.is_some());
    let locked = ui::check("Leave root locked (use sudo)", cfg.root_password_hash.is_none(), {
        let (a, hash_row) = (app.clone(), hash_row.clone());
        move |locked| {
            hash_row.set_visible(!locked);
            a.edit(move |m| m.cfg.root_password_hash = if locked { None } else { Some(String::new()) });
        }
    });
    c.append(&locked);
    c.append(&hash_row);

    let c = ui::card(content, Some("Home directory config (zip)"));
    ui::note(&c, Kind::Warn, "Only for the Hyprland preset. The zip is laid out like a Hyprland setup (.config/hypr/hyprland.lua, ...) and is unpacked into every user's home. Do not use it for other desktops.");
    ui::note(&c, Kind::Warn, "A Hyprland config can run any command when the session starts. Only use a server you trust: the archive is checked by HTTPS alone.");
    let url = cfg.user_archives.first().map(|a| a.url.clone()).unwrap_or_default();
    let a = app.clone();
    ui::row(&c, "Zip URL", &ui::entry(&url, move |t| {
        let t = t.trim().to_string();
        a.edit(move |m| m.cfg.user_archives = if t.is_empty() { vec![] } else { vec![config::UserArchive { url: t }] });
    }));
    ui::hint(&c, "https:// only, at most 16 MiB. Leave empty to skip. If the download fails at install time it is skipped with a warning.");
}
