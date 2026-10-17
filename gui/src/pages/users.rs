use crate::app::{App, Page};
use crate::ui::{self, Kind};
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let (cfg, groups, shells) = {
        let st = app.st.borrow();
        (
            st.model.cfg.clone(),
            st.lists.groups.clone(),
            st.lists.shells.clone(),
        )
    };
    ui::heading(
        content,
        "Users",
        "Passwords are stored as SHA-512 crypt hashes only; the plaintext is never saved.",
    );

    let c = ui::card(content, Some("Accounts"));
    if cfg.users.is_empty() {
        ui::hint(
            &c,
            "No users yet: the system would have only a locked root account.",
        );
    }
    for (i, u) in cfg.users.iter().enumerate() {
        let user_box = ui::vbox(6);
        let r = ui::hbox(14);
        let name = ui::strong(&u.name);
        name.set_width_request(150);
        r.append(&name);
        let g = ui::dim(&format!("groups: {}", u.groups.join(", ")));
        g.set_hexpand(true);
        r.append(&g);
        r.append(&ui::dim(&u.shell));

        let hash_type = if u.password_hash.starts_with("$y$") {
            "yescrypt"
        } else if u.password_hash.starts_with("$gy$") {
            "gost-yescrypt"
        } else if u.password_hash.starts_with("$6$") {
            "SHA-512"
        } else if u.password_hash.starts_with("$5$") {
            "SHA-256"
        } else if u.password_hash.starts_with("$2") {
            "bcrypt"
        } else if u.password_hash.starts_with("$7$") {
            "scrypt"
        } else if u.password_hash.starts_with("$argon2") {
            "argon2"
        } else if u.password_hash.starts_with("$1$") {
            "MD5"
        } else if u.password_hash.is_empty() {
            "none"
        } else {
            "crypt"
        };
        let hash_tag = ui::dim(&format!("pwd: {hash_type}"));
        r.append(&hash_tag);

        let a = app.clone();
        r.append(&ui::button("Remove", move || {
            a.edit(move |m| {
                m.cfg.users.remove(i);
            });
            a.rebuild(Page::Users);
        }));
        user_box.append(&r);

        let a = app.clone();
        let he = ui::entry(&u.password_hash, move |t| {
            let t = t.trim().to_string();
            a.edit(move |m| {
                if let Some(user) = m.cfg.users.get_mut(i) {
                    user.password_hash = t.clone();
                }
            });
        });
        ui::row(&user_box, "Password hash", &he);
        c.append(&user_box);
    }

    let autoload_row = ui::hbox(8);
    let a = app.clone();
    let autoload_btn = ui::button("Autoload users from system", move || {
        let host_users = hostcfg::users::discover_host_users();
        let mut added = 0;
        let mut need_auth = false;
        a.edit(|m| {
            for u in &host_users {
                if let Some(existing) = m.cfg.users.iter_mut().find(|e| e.name == u.name) {
                    existing.groups = u.groups.clone();
                    existing.shell = u.shell.clone();
                    if let Some(hash) = &u.password_hash {
                        existing.password_hash = hash.clone();
                    } else {
                        need_auth = true;
                    }
                } else if let Some(hash) = &u.password_hash {
                    m.cfg.users.push(config::User {
                        name: u.name.clone(),
                        password_hash: hash.clone(),
                        groups: u.groups.clone(),
                        shell: u.shell.clone(),
                    });
                    added += 1;
                } else {
                    need_auth = true;
                }
            }
        });
        if need_auth {
            open_shadow_auth_dialog(&a);
        } else if added > 0 {
            a.set_status(format!("Autoloaded {added} user(s) with password hash from shadow."));
            a.rebuild(Page::Users);
        } else {
            a.set_status("All host users are already up to date.");
            a.rebuild(Page::Users);
        }
    });
    autoload_btn.set_tooltip_text(Some(
        "Detects regular user accounts on this computer from /etc/passwd and /etc/group.",
    ));
    autoload_row.append(&autoload_btn);

    let a = app.clone();
    let auth_shadow_btn = ui::button("Retrieve password hashes (sudo / root)", move || {
        open_shadow_auth_dialog(&a);
    });
    auth_shadow_btn.set_tooltip_text(Some(
        "Authenticates via sudo password, root password, or Polkit to retrieve crypt hashes from /etc/shadow.",
    ));
    autoload_row.append(&auth_shadow_btn);
    c.append(&autoload_row);

    let c = ui::card(content, Some("Add a user"));
    let host_curr = hostcfg::users::current_user();
    let name = ui::entry("", |_| {});
    let grp = ui::suggest("wheel", groups, true, |_| {});
    let sh = ui::suggest("/bin/bash", shells, false, |_| {});

    let is_plain_mode = Rc::new(RefCell::new(false));
    let hash_entry = ui::entry("", |_| {});
    let single_hash_box = ui::vbox(4);
    ui::row(&single_hash_box, "Password hash", &hash_entry);
    ui::hint(
        &single_hash_box,
        "A valid crypt hash (e.g. yescrypt $y$... or SHA-512 $6$...).",
    );

    let pw = ui::password(|_| {});
    let pw2 = ui::password(|_| {});
    let plain_pw_box = ui::vbox(4);
    ui::row(&plain_pw_box, "Password", &pw);
    ui::row(&plain_pw_box, "Again", &pw2);
    let mismatch = ui::note(&plain_pw_box, Kind::Bad, "The passwords differ");
    mismatch.set_visible(false);
    plain_pw_box.set_visible(false);

    let mode_switch = ui::check(
        "Enter plain password instead of hash",
        false,
        |_| {},
    );

    let add = ui::primary("Add user", || {});
    add.set_sensitive(false);

    let update = {
        let (name, pw, pw2, hash_entry, mismatch, add, is_plain) = (
            name.clone(),
            pw.clone(),
            pw2.clone(),
            hash_entry.clone(),
            mismatch.clone(),
            add.clone(),
            is_plain_mode.clone(),
        );
        Rc::new(move || {
            let n = name.text().trim().to_string();
            let plain = *is_plain.borrow();
            if plain {
                let (p1, p2) = (pw.text().to_string(), pw2.text().to_string());
                mismatch.set_visible(!p1.is_empty() && p1 != p2);
                add.set_sensitive(!n.is_empty() && !p1.is_empty() && p1 == p2);
            } else {
                let h = hash_entry.text().trim().to_string();
                let valid_h = h.starts_with('$') && h.len() > 13;
                add.set_sensitive(!n.is_empty() && valid_h);
            }
        })
    };

    for e in [
        &name.clone().upcast::<gtk::Editable>(),
        &pw.clone().upcast::<gtk::Editable>(),
        &pw2.clone().upcast::<gtk::Editable>(),
        &hash_entry.clone().upcast::<gtk::Editable>(),
    ] {
        let u = update.clone();
        e.connect_changed(move |_| u());
    }

    {
        let (u, s_hash, s_plain, is_plain) = (
            update.clone(),
            single_hash_box.clone(),
            plain_pw_box.clone(),
            is_plain_mode.clone(),
        );
        mode_switch.connect_toggled(move |btn| {
            let plain = btn.is_active();
            *is_plain.borrow_mut() = plain;
            s_hash.set_visible(!plain);
            s_plain.set_visible(plain);
            u();
        });
    }

    if let Some(hu) = host_curr {
        let a = app.clone();
        let n_ent = name.clone();
        let g_ent = grp.clone();
        let s_ent = sh.clone();
        let h_ent = hash_entry.clone();
        let m_switch = mode_switch.clone();
        let u_update = update.clone();
        let auto_user_btn =
            ui::button(&format!("Autoload user '{}' from system", hu.name), move || {
                n_ent.set_text(&hu.name);
                g_ent.set_text(&hu.groups.join(", "));
                s_ent.set_text(&hu.shell);
                if let Some(hash) = &hu.password_hash {
                    h_ent.set_text(hash);
                    m_switch.set_active(false);
                    u_update();
                    h_ent.grab_focus();
                    a.set_status(format!(
                        "Autoloaded host user '{}' with password hash.",
                        hu.name
                    ));
                } else {
                    u_update();
                    a.set_status(format!(
                        "Autoloaded host user '{}'. Please enter password hash or switch to plain password.",
                        hu.name
                    ));
                }
            });
        auto_user_btn.set_tooltip_text(Some(
            "Pre-fills username, supplemental groups, and shell from this machine.",
        ));
        c.append(&auto_user_btn);
    }

    ui::row(&c, "Name", &name);
    c.append(&single_hash_box);
    c.append(&plain_pw_box);
    c.append(&mode_switch);
    ui::row(&c, "Groups", &grp);
    ui::row(&c, "Shell", &sh);
    {
        let (a, name, pw, hash_entry, grp, sh, is_plain) = (
            app.clone(),
            name.clone(),
            pw.clone(),
            hash_entry.clone(),
            grp.clone(),
            sh.clone(),
            is_plain_mode,
        );
        add.connect_clicked(move |_| {
            let groups: Vec<String> = grp
                .text()
                .split(',')
                .map(|g| g.trim().to_string())
                .filter(|g| !g.is_empty())
                .collect();
            let user_name = name.text().trim().to_string();
            let shell_path = sh.text().trim().to_string();
            let plain = *is_plain.borrow();

            if plain {
                match hostcfg::password::hash(pw.text().as_str()) {
                    Ok(h) => {
                        let user = config::User {
                            name: user_name,
                            password_hash: h,
                            groups,
                            shell: shell_path,
                        };
                        a.edit(move |m| m.cfg.users.push(user));
                        a.rebuild(Page::Users);
                    }
                    Err(e) => a.set_status(format!("cannot hash the password: {e}")),
                }
            } else {
                let user = config::User {
                    name: user_name,
                    password_hash: hash_entry.text().trim().to_string(),
                    groups,
                    shell: shell_path,
                };
                a.edit(move |m| m.cfg.users.push(user));
                a.rebuild(Page::Users);
            }
        });
    }
    c.append(&add);

    let c = ui::card(content, Some("root account"));
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
                // Unlocking brings back what was typed before locking, not an empty (invalid) hash.
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
        open_shadow_auth_dialog(&a);
    });
    root_auth_btn.set_tooltip_text(Some(
        "Authenticates via sudo password, root password, or Polkit to read root's crypt hash from /etc/shadow.",
    ));
    c.append(&root_auth_btn);

    let c = ui::card(content, Some("Home directory config (zip)"));
    ui::note(&c, Kind::Warn, "Only for the Hyprland preset. The zip is laid out like a Hyprland setup (.config/hypr/hyprland.conf, ...) and is unpacked into every user's home. Do not use it for other desktops.");
    ui::note(&c, Kind::Warn, "A Hyprland config can run any command when the session starts. Only use a server you trust: the archive is checked by HTTPS alone.");
    let url = cfg
        .user_archives
        .first()
        .map(|a| a.url.clone())
        .unwrap_or_default();
    let a = app.clone();
    ui::row(
        &c,
        "Zip URL",
        &ui::entry(&url, move |t| {
            let t = t.trim().to_string();
            a.edit(move |m| {
                m.cfg.user_archives = if t.is_empty() {
                    vec![]
                } else {
                    vec![config::UserArchive { url: t }]
                }
            });
        }),
    );
    ui::hint(&c, "https:// only, at most 16 MiB. Leave empty to skip. If the download fails at install time it is skipped with a warning.");
}

fn open_shadow_auth_dialog(app: &Rc<App>) {
    let dialog = gtk::Window::builder()
        .title("Retrieve Password Hashes")
        .transient_for(&app.win)
        .modal(true)
        .default_width(480)
        .build();

    let root = ui::vbox(12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    ui::heading(
        &root,
        "Authentication Required",
        "Reading /etc/shadow requires elevated privileges. Authenticate via your sudo password, root password, or the desktop Polkit prompt.",
    );

    let err_note = ui::note(&root, Kind::Bad, "");
    err_note.set_visible(false);

    let pw = ui::password(|_| {});
    ui::row(&root, "Password", &pw);
    ui::hint(
        &root,
        "Your user password (for sudo) or the system root password.",
    );

    let (spinner_box, _) = ui::spinner_row("Authenticating and reading /etc/shadow…");
    spinner_box.set_visible(false);
    root.append(&spinner_box);

    let actions = ui::hbox(8);
    let cancel = ui::button("Cancel", {
        let d = dialog.clone();
        move || d.close()
    });
    actions.append(&cancel);

    let auth_btn = ui::primary("Authenticate with password", || {});
    let pkexec_btn = ui::button("Use Polkit prompt (pkexec)", || {});
    actions.append(&auth_btn);
    actions.append(&pkexec_btn);
    root.append(&actions);

    dialog.set_child(Some(&root));

    let on_shadow_success = {
        let a = app.clone();
        let d = dialog.clone();
        Rc::new(move |shadow: String, entered_pw: Option<String>| {
            let mut updated = 0;
            let mut added = 0;
            let host_users = hostcfg::users::discover_host_users();
            let curr_user = std::env::var("USER").ok();
            a.edit(|m| {
                updated += hostcfg::users::update_users_from_shadow(&mut m.cfg.users, &shadow);
                for u in &host_users {
                    if let Some(existing) = m.cfg.users.iter_mut().find(|e| e.name == u.name) {
                        existing.groups = u.groups.clone();
                        existing.shell = u.shell.clone();
                        if let Some(hash) = hostcfg::users::parse_shadow_hash(&shadow, &u.name) {
                            if existing.password_hash != hash {
                                existing.password_hash = hash;
                                updated += 1;
                            }
                        }
                    } else {
                        let hash_opt = hostcfg::users::parse_shadow_hash(&shadow, &u.name)
                            .or_else(|| {
                                if curr_user.as_deref() == Some(&u.name) {
                                    entered_pw.as_deref().and_then(|p| hostcfg::password::hash(p).ok())
                                } else {
                                    None
                                }
                            });
                        if let Some(hash) = hash_opt {
                            m.cfg.users.push(config::User {
                                name: u.name.clone(),
                                password_hash: hash,
                                groups: u.groups.clone(),
                                shell: u.shell.clone(),
                            });
                            added += 1;
                        }
                    }
                }
                if let (Some(curr), Some(pw)) = (&curr_user, &entered_pw) {
                    if let Some(user_obj) = m.cfg.users.iter_mut().find(|u| &u.name == curr) {
                        if hostcfg::users::parse_shadow_hash(&shadow, curr).is_none() {
                            if let Ok(h) = hostcfg::password::hash(pw) {
                                user_obj.password_hash = h;
                                updated += 1;
                            }
                        }
                    }
                }
                if let Some(rh) = hostcfg::users::parse_shadow_hash(&shadow, "root") {
                    m.cfg.root_password_hash = Some(rh);
                }
            });

            d.close();
            let mut msg = format!(
                "Retrieved password hashes: updated {updated} user account(s), added {added} host account(s)."
            );
            if hostcfg::users::parse_shadow_hash(&shadow, "root").is_some() {
                msg.push_str(" Root password hash also updated.");
            }
            a.set_status(msg);
            a.rebuild(Page::Users);
        })
    };

    {
        let pw = pw.clone();
        let on_success = on_shadow_success.clone();
        let spinner = spinner_box.clone();
        let err = err_note.clone();
        let b1 = auth_btn.clone();
        let b2 = pkexec_btn.clone();
        auth_btn.connect_clicked(move |_| {
            let pass = pw.text().to_string();
            if pass.trim().is_empty() {
                err.set_text("Please enter a password.");
                err.set_visible(true);
                return;
            }
            spinner.set_visible(true);
            err.set_visible(false);
            b1.set_sensitive(false);
            b2.set_sensitive(false);

            let on_succ = on_success.clone();
            let spinner_done = spinner.clone();
            let err_done = err.clone();
            let b1_done = b1.clone();
            let b2_done = b2.clone();
            let pass_cb = pass.clone();

            gtk::glib::spawn_future_local(async move {
                let res = gtk::gio::spawn_blocking(move || {
                    hostcfg::users::authenticate_and_read_shadow(Some(&pass), false)
                })
                .await
                .unwrap_or_else(|_| Err("Authentication task panicked or failed".into()));

                spinner_done.set_visible(false);
                b1_done.set_sensitive(true);
                b2_done.set_sensitive(true);
                match res {
                    Ok(shadow) => on_succ(shadow, Some(pass_cb)),
                    Err(e) => {
                        err_done.set_text(&format!("Authentication failed: {e}"));
                        err_done.set_visible(true);
                    }
                }
            });
        });
    }

    {
        let on_success = on_shadow_success;
        let spinner = spinner_box;
        let err = err_note;
        let b1 = auth_btn;
        let b2 = pkexec_btn.clone();
        pkexec_btn.connect_clicked(move |_| {
            spinner.set_visible(true);
            err.set_visible(false);
            b1.set_sensitive(false);
            b2.set_sensitive(false);

            let on_succ = on_success.clone();
            let spinner_done = spinner.clone();
            let err_done = err.clone();
            let b1_done = b1.clone();
            let b2_done = b2.clone();

            gtk::glib::spawn_future_local(async move {
                let res = gtk::gio::spawn_blocking(move || {
                    hostcfg::users::authenticate_and_read_shadow(None, true)
                })
                .await
                .unwrap_or_else(|_| Err("Polkit task panicked or failed".into()));

                spinner_done.set_visible(false);
                b1_done.set_sensitive(true);
                b2_done.set_sensitive(true);
                match res {
                    Ok(shadow) => on_succ(shadow, None),
                    Err(e) => {
                        err_done.set_text(&format!("Polkit authentication failed: {e}"));
                        err_done.set_visible(true);
                    }
                }
            });
        });
    }

    dialog.present();
}
