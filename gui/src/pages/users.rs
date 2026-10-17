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
        name.set_width_request(100);
        r.append(&name);
        let g = ui::dim(&format!("groups: {}", u.groups.join(", ")));
        g.set_ellipsize(gtk::pango::EllipsizeMode::End);
        g.set_tooltip_text(Some(&format!("groups: {}", u.groups.join(", "))));
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

    let autoload_box = ui::vbox(8);
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
    autoload_box.append(&autoload_btn);

    let a = app.clone();
    let auth_shadow_btn = ui::button("Retrieve password hashes (sudo / root)", move || {
        open_shadow_auth_dialog(&a);
    });
    auth_shadow_btn.set_tooltip_text(Some(
        "Authenticates via sudo password, root password, or Polkit to retrieve crypt hashes from /etc/shadow.",
    ));
    autoload_box.append(&auth_shadow_btn);
    c.append(&autoload_box);

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

    let c = ui::card(content, Some("Home directory archives & configs"));
    ui::note(
        &c,
        Kind::Warn,
        "Archives are extracted into every configured user's home directory (.config, dotfiles, scripts). Supports .tar.zst (recommended), .tar.xz, .tar.gz, .tar.bz2, and .zip up to 256 MiB. Downloads and disk extractions run on first boot with full Linux drivers.",
    );
    ui::note(
        &c,
        Kind::Warn,
        "A user config can execute code at login. Only use archives from sources you trust. Embedded archives are not supported on floppy disks.",
    );

    if cfg.user_archives.is_empty() {
        ui::hint(&c, "No user archives configured.");
    }
    for (i, a_item) in cfg.user_archives.iter().enumerate() {
        let item_box = ui::hbox(8);
        let desc = if let Some(url) = &a_item.url {
            format!("Remote URL: {url}")
        } else if let Some(file) = &a_item.file {
            format!("Local file (embedded): {file}")
        } else if let Some(uuid) = &a_item.disk_uuid {
            let path = a_item.path.as_deref().unwrap_or("home.tar.zst");
            format!("Disk partition UUID: {uuid} ({path})")
        } else {
            "Embedded archive".to_string()
        };
        let l = ui::strong(&desc);
        l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        l.set_tooltip_text(Some(&desc));
        l.set_hexpand(true);
        item_box.append(&l);
        if let Some(sha) = &a_item.sha256 {
            let prefix = sha.get(..12).unwrap_or(sha.as_str());
            item_box.append(&ui::dim(&format!("sha256: {prefix}…")));
        }
        let a = app.clone();
        item_box.append(&ui::button("Remove", move || {
            a.edit(move |m| {
                m.cfg.user_archives.remove(i);
            });
            a.rebuild_all();
        }));
        c.append(&item_box);
    }

    // Add archive section
    let add_title = ui::strong("Add user archive");
    c.append(&add_title);

    let src_box = ui::vbox(6);
    let r_url = ui::check_button("Remote URL (HTTPS)");
    let r_file = ui::check_button("Local file (embed into ISO)");
    let r_uuid = ui::check_button("Disk partition (UUID)");
    r_file.set_group(Some(&r_url));
    r_uuid.set_group(Some(&r_url));
    r_url.set_active(true);
    src_box.append(&r_url);
    src_box.append(&r_file);
    src_box.append(&r_uuid);
    c.append(&src_box);

    let url_input = ui::entry("", |_| {});
    let url_row = ui::row(&c, "HTTPS URL", &url_input);
    let url_hint = ui::hint(&c, "Remote https:// URL, downloaded on first boot.");

    let file_input = ui::entry("", |_| {});
    let file_browse = ui::button("Browse…", {
        let win = app.win.clone();
        let fe = file_input.clone();
        move || {
            let win = win.clone();
            let fe = fe.clone();
            gtk::glib::spawn_future_local(async move {
                if let Some(p) = crate::dialogs::open_file(win.upcast_ref(), "Pick Archive File", None).await {
                    fe.set_text(&p.display().to_string());
                }
            });
        }
    });
    let file_box = ui::hbox(8);
    file_box.append(&file_input);
    file_box.append(&file_browse);
    let file_row = ui::row(&c, "Local file", &file_box);
    let file_hint = ui::hint(&c, "Path to .tar.zst, .tar.xz, or .zip. Will be embedded into the ISO (max 256 MiB; not supported on floppy).");

    let uuid_input = ui::entry("", |_| {});
    let path_input = ui::entry("home.tar.zst", |_| {});
    let uuid_detect = ui::button("Detect partition…", {
        let a = app.clone();
        let ue = uuid_input.clone();
        let pe = path_input.clone();
        move || {
            open_partition_picker_dialog(&a, &ue, &pe);
        }
    });
    let uuid_box = ui::hbox(8);
    uuid_box.append(&uuid_input);
    uuid_box.append(&uuid_detect);
    let uuid_row = ui::row(&c, "Disk UUID", &uuid_box);
    let path_row = ui::row(&c, "Path on disk", &path_input);
    let uuid_hint = ui::hint(&c, "Partition UUID (from `lsblk -f`) containing the archive file at the specified path (defaults to home.tar.zst in the root of the partition).");

    file_row.set_visible(false);
    file_hint.set_visible(false);
    uuid_row.set_visible(false);
    path_row.set_visible(false);
    uuid_hint.set_visible(false);

    let sha_input = ui::entry("", |_| {});
    let _sha_row = ui::row(&c, "SHA-256 (opt)", &sha_input);
    let _sha_hint = ui::hint(&c, "Optional 64-character lowercase hex digest.");

    {
        let (u_r, u_h, f_r, f_h, d_r, p_r, d_h) = (
            url_row.clone(),
            url_hint.clone(),
            file_row.clone(),
            file_hint.clone(),
            uuid_row.clone(),
            path_row.clone(),
            uuid_hint.clone(),
        );
        let (ru, rf, rd) = (r_url.clone(), r_file.clone(), r_uuid.clone());
        let on_toggle = move || {
            let is_u = ru.is_active();
            let is_f = rf.is_active();
            let is_d = rd.is_active();
            u_r.set_visible(is_u);
            u_h.set_visible(is_u);
            f_r.set_visible(is_f);
            f_h.set_visible(is_f);
            d_r.set_visible(is_d);
            p_r.set_visible(is_d);
            d_h.set_visible(is_d);
        };
        let ot1 = on_toggle.clone();
        r_url.connect_toggled(move |_| ot1());
        let ot2 = on_toggle.clone();
        r_file.connect_toggled(move |_| ot2());
        r_uuid.connect_toggled(move |_| on_toggle());
    }

    let add_archive_btn = ui::button("Add archive", {
        let a = app.clone();
        let ru = r_url;
        let rf = r_file;
        let rd = r_uuid;
        let ui_entry = url_input;
        let fi_entry = file_input;
        let di_entry = uuid_input;
        let pi_entry = path_input;
        let si_entry = sha_input;
        move || {
            let sha = {
                let s = si_entry.text().trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            };
            let mut new_arch = None;
            if ru.is_active() {
                let u = ui_entry.text().trim().to_string();
                if !u.is_empty() {
                    new_arch = Some(config::UserArchive {
                        url: Some(u),
                        file: None,
                        content: None,
                        disk_uuid: None,
                        path: None,
                        sha256: sha,
                    });
                }
            } else if rf.is_active() {
                let f = fi_entry.text().trim().to_string();
                if !f.is_empty() {
                    new_arch = Some(config::UserArchive {
                        url: None,
                        file: Some(f),
                        content: None,
                        disk_uuid: None,
                        path: None,
                        sha256: sha,
                    });
                }
            } else if rd.is_active() {
                let d = di_entry.text().trim().to_string();
                let p = pi_entry.text().trim().to_string();
                if !d.is_empty() {
                    let path = if p.is_empty() { None } else { Some(p) };
                    new_arch = Some(config::UserArchive {
                        url: None,
                        file: None,
                        content: None,
                        disk_uuid: Some(d),
                        path,
                        sha256: sha,
                    });
                }
            }
            if let Some(arch) = new_arch {
                a.edit(move |m| m.cfg.user_archives.push(arch));
                a.rebuild_all();
            }
        }
    });
    c.append(&add_archive_btn);
}

pub fn open_shadow_auth_dialog(app: &Rc<App>) {
    let dialog = gtk::Window::builder()
        .title("Retrieve Password Hashes & System Overlay")
        .transient_for(&app.win)
        .modal(true)
        .default_width(460)
        .build();

    let root = ui::vbox(12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    ui::heading(
        &root,
        "Authentication Required",
        "Reading /etc/shadow, NetworkManager profiles, and biometric credentials requires elevated privileges. Authenticate via your sudo password (touch your fingerprint sensor if enrolled), root password, or the desktop Polkit prompt.",
    );

    let err_note = ui::note(&root, Kind::Bad, "");
    err_note.set_visible(false);

    let pw = ui::password(|_| {});
    ui::row(&root, "Password", &pw);
    ui::hint(
        &root,
        "Your user password (for sudo) or the system root password.",
    );

    let (spinner_box, _) = ui::spinner_row(
        "Authenticating and reading system state… (Touch fingerprint sensor or wait for password verification)",
    );
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
    if !crate::polkit::is_agent_available() {
        pkexec_btn.set_tooltip_text(Some(
            "No Polkit authentication agent is running in this session (e.g. hyprpolkitagent). Please enter your password above.",
        ));
    }
    actions.append(&auth_btn);
    actions.append(&pkexec_btn);
    root.append(&actions);

    dialog.set_child(Some(&root));

    let on_priv_success = {
        let a = app.clone();
        let d = dialog.clone();
        Rc::new(
            move |data: hostcfg::autoload::PrivilegedAutoloadData,
                  entered_pw: Option<String>| {
                let mut updated_users = 0;
                let mut added_users = 0;
                let curr_user = std::env::var("USER").ok();

                a.edit(|m| {
                    if let Some(shadow) = &data.shadow {
                        updated_users +=
                            hostcfg::users::update_users_from_shadow(&mut m.cfg.users, shadow);
                    }
                    if let Some(rh) = data.root_password_hash {
                        m.cfg.root_password_hash = Some(rh);
                    }
                    for u in data.users {
                        if let Some(existing) = m.cfg.users.iter_mut().find(|e| e.name == u.name) {
                            existing.groups = u.groups;
                            existing.shell = u.shell;
                            if u.password_hash.starts_with('$')
                                && existing.password_hash != u.password_hash
                            {
                                existing.password_hash = u.password_hash;
                                updated_users += 1;
                            }
                        } else {
                            m.cfg.users.push(u);
                            added_users += 1;
                        }
                    }

                    if let (Some(curr), Some(pw)) = (&curr_user, &entered_pw) {
                        if let Some(user_obj) = m.cfg.users.iter_mut().find(|u| &u.name == curr) {
                            if !user_obj.password_hash.starts_with('$') {
                                if let Ok(h) = hostcfg::password::hash(pw) {
                                    user_obj.password_hash = h;
                                    updated_users += 1;
                                }
                            }
                        }
                    }

                    for sf in data.system_files {
                        if !m
                            .cfg
                            .system_files
                            .iter()
                            .any(|existing| existing.path == sf.path)
                        {
                            m.cfg.system_files.push(sf);
                        }
                    }

                    for p in data.extra_packages {
                        if !m.cfg.packages.contains(&p) {
                            m.cfg.packages.push(p);
                        }
                    }

                    for s in data.extra_services {
                        if !m.cfg.services.contains(&s) {
                            m.cfg.services.push(s);
                        }
                    }
                });

                d.close();
                let sys_count = a.st.borrow().model.cfg.system_files.len();
                let mut msg = format!(
                    "Retrieved system overlay: updated {updated_users} user(s), added {added_users} user(s)."
                );
                if a.st.borrow().model.cfg.root_password_hash.is_some() {
                    msg.push_str(" Root password hash updated.");
                }
                if sys_count > 0 {
                    msg.push_str(&format!(
                        " Preserved {sys_count} system overlay file(s) (network, biometrics)."
                    ));
                }
                a.set_status(msg);
                a.rebuild_all();
            },
        )
    };

    {
        let pw = pw.clone();
        let on_success = on_priv_success.clone();
        let spinner = spinner_box.clone();
        let err = err_note.clone();
        let b1 = auth_btn.clone();
        let b2 = pkexec_btn.clone();
        let b1_trigger = auth_btn.clone();
        pw.connect_activate(move |_| {
            b1_trigger.emit_clicked();
        });

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
                    hostcfg::autoload::discover_live_system_privileged(Some(&pass), false)
                })
                .await
                .unwrap_or_else(|_| Err("Authentication task panicked or failed".into()));

                spinner_done.set_visible(false);
                b1_done.set_sensitive(true);
                b2_done.set_sensitive(true);
                match res {
                    Ok(data) => on_succ(data, Some(pass_cb)),
                    Err(e) => {
                        err_done.set_text(&format!("Authentication failed: {e}"));
                        err_done.set_visible(true);
                    }
                }
            });
        });
    }

    {
        let on_success = on_priv_success;
        let spinner = spinner_box;
        let err = err_note;
        let b1 = auth_btn;
        let b2 = pkexec_btn.clone();
        pkexec_btn.connect_clicked(move |_| {
            if !crate::polkit::is_agent_available() {
                err.set_text("No desktop Polkit authentication agent is running in this session (e.g. hyprpolkitagent or polkit-gnome). Please enter your root/sudo password above.");
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

            gtk::glib::spawn_future_local(async move {
                let res = gtk::gio::spawn_blocking(move || {
                    hostcfg::autoload::discover_live_system_privileged(None, true)
                })
                .await
                .unwrap_or_else(|_| Err("Polkit task panicked or failed".into()));

                spinner_done.set_visible(false);
                b1_done.set_sensitive(true);
                b2_done.set_sensitive(true);
                match res {
                    Ok(data) => on_succ(data, None),
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

pub fn open_partition_picker_dialog(
    app: &Rc<App>,
    uuid_entry: &gtk::Entry,
    path_entry: &gtk::Entry,
) {
    let parts = crate::media::partitions();
    if parts.is_empty() {
        app.set_status("No disk partitions with UUIDs detected.");
        return;
    }

    let dialog = gtk::Window::builder()
        .title("Select Storage Partition")
        .transient_for(&app.win)
        .modal(true)
        .default_width(520)
        .default_height(380)
        .build();

    let root = ui::vbox(12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    ui::heading(
        &root,
        "Storage Partitions",
        "Select a partition to supply the archive on first boot (e.g. a USB flash drive). If an archive is found at the partition root, its path is filled automatically.",
    );

    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();

    let mut part_rows = Vec::new();
    for p in &parts {
        let r = ui::vbox(4);
        r.set_margin_top(6);
        r.set_margin_bottom(6);
        r.set_margin_start(8);
        r.set_margin_end(8);

        let top_row = ui::hbox(8);
        let title = if !p.label.is_empty() {
            format!("{} ({})", p.label, p.fstype)
        } else {
            format!("{} ({})", p.path, p.fstype)
        };
        let t_lbl = ui::strong(&title);
        t_lbl.set_hexpand(true);
        top_row.append(&t_lbl);

        let size_str = format_size(p.size);
        top_row.append(&ui::dim(&size_str));
        if p.removable {
            let rem_badge = ui::dim("[USB / removable]");
            top_row.append(&rem_badge);
        }
        r.append(&top_row);

        let sub_row = ui::hbox(8);
        let u_lbl = ui::dim(&format!("UUID: {}", p.uuid));
        u_lbl.set_hexpand(true);
        sub_row.append(&u_lbl);

        let detected_file = p.mountpoint.as_deref().and_then(detect_archive_on_mount);
        if let Some(ref f) = detected_file {
            let found_badge = ui::strong(&format!("Found: {f}"));
            found_badge.add_css_class("nav-good");
            sub_row.append(&found_badge);
        }
        r.append(&sub_row);

        list_box.append(&r);
        part_rows.push((p.clone(), detected_file));
    }

    let scroll = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .min_content_height(200)
        .build();
    root.append(&scroll);

    let actions = ui::hbox(8);
    let cancel = ui::button("Cancel", {
        let d = dialog.clone();
        move || d.close()
    });
    let select_btn = ui::button("Select", {
        let d = dialog.clone();
        let lb = list_box.clone();
        let ue = uuid_entry.clone();
        let pe = path_entry.clone();
        let rows = part_rows.clone();
        move || {
            if let Some(row) = lb.selected_row() {
                if let Some((part, detected)) = usize::try_from(row.index()).ok().and_then(|idx| rows.get(idx)) {
                    ue.set_text(&part.uuid);
                    if let Some(f) = detected {
                        pe.set_text(f);
                    }
                }
            }
            d.close();
        }
    });

    // Double-click row activates selection
    {
        let d = dialog.clone();
        let ue = uuid_entry.clone();
        let pe = path_entry.clone();
        let rows = part_rows;
        list_box.connect_row_activated(move |_, row| {
            if let Some((part, detected)) = usize::try_from(row.index()).ok().and_then(|idx| rows.get(idx)) {
                ue.set_text(&part.uuid);
                if let Some(f) = detected {
                    pe.set_text(f);
                }
            }
            d.close();
        });
    }

    actions.append(&cancel);
    actions.append(&select_btn);
    root.append(&actions);

    dialog.set_child(Some(&root));
    dialog.present();
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{} MB", bytes >> 20)
    } else {
        format!("{} KB", bytes >> 10)
    }
}

fn detect_archive_on_mount(mount: &str) -> Option<String> {
    let p = std::path::Path::new(mount);
    for candidate in [
        "home.tar.zst",
        "home.tar.xz",
        "home.tar.gz",
        "home.zip",
        "dotfiles.tar.zst",
        "dotfiles.tar.xz",
    ] {
        if p.join(candidate).is_file() {
            return Some(candidate.to_string());
        }
    }
    if let Ok(entries) = std::fs::read_dir(p) {
        for e in entries.flatten() {
            if let Ok(ft) = e.file_type() {
                if ft.is_file() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.ends_with(".tar.zst")
                        || name.ends_with(".tar.xz")
                        || name.ends_with(".zip")
                    {
                        return Some(name);
                    }
                }
            }
        }
    }
    None
}

