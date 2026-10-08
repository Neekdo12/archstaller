use crate::app::{App, Page};
use crate::dialogs;
use crate::ui::{self, Kind};
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let cfg = app.st.borrow().model.cfg.clone();
    ui::heading(content, "First-boot scripts", "Run as root on the installed system, in this order, at the end of the first boot.");
    ui::note(content, Kind::Warn, "Scripts are code. A failing script only logs a warning.");

    let c = ui::card(content, Some("Scripts in this config"));
    if cfg.scripts.is_empty() {
        ui::hint(&c, "None yet.");
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
        id.set_width_request(140);
        r.append(&id);
        let d = ui::dim(&format!("{src}  {}", s.args.join(" ")));
        d.set_hexpand(true);
        r.append(&d);
        let a = app.clone();
        r.append(&ui::button("Remove", move || {
            a.edit(move |m| {
                m.cfg.scripts.remove(i);
            });
            a.rebuild(Page::Scripts);
        }));
        c.append(&r);
    }

    let c = ui::card(content, Some("Built-in scripts"));
    for b in hostcfg::scripts::BUILTINS {
        let r = ui::hbox(10);
        let id = ui::strong(b.id);
        id.set_hexpand(true);
        r.append(&id);
        let (a, bid) = (app.clone(), b.id);
        r.append(&ui::button("Add", move || {
            a.edit(move |m| {
                if !m.cfg.scripts.iter().any(|s| s.id == bid) {
                    m.cfg.scripts.push(config::Script { id: bid.into(), ..Default::default() });
                }
            });
            a.rebuild(Page::Scripts);
        }));
        c.append(&r);
        c.append(&gtk::Label::builder().label(b.description).xalign(0.0).wrap(true).build());
        ui::hint(&c, &format!("runs as {}, {}; needs: {}", b.runs_as, b.phase, if b.requires.is_empty() { "nothing" } else { b.requires }));
    }

    let c = ui::card(content, Some("Custom script"));
    ui::hint(&c, "A local file, or an https URL pinned by its SHA-256 digest.");
    let id = ui::entry("", |_| {});
    ui::row(&c, "Id", &id);
    let file = ui::entry("", |_| {});
    let fr = ui::row(&c, "Local file", &file);
    let url = ui::entry("", |_| {});
    ui::row(&c, "URL", &url);
    let sha = ui::entry("", |_| {});
    ui::row(&c, "SHA-256", &sha);
    let digest = ui::dim("");
    c.append(&digest);
    let ack = ui::check_button("I understand a custom script runs as root on the installed system");
    let add = ui::primary("Add custom script", || {});
    add.set_sensitive(false);
    let update = {
        let (id, file, url, ack, add, digest) = (id.clone(), file.clone(), url.clone(), ack.clone(), add.clone(), digest.clone());
        Rc::new(move || {
            let path = file.text().to_string();
            let text = if path.is_empty() { None } else { std::fs::read(&path).ok() };
            digest.set_text(&match &text {
                Some(t) => {
                    use sha2::Digest;
                    format!("sha256 {}", sha2::Sha256::digest(t).iter().map(|b| format!("{b:02x}")).collect::<String>())
                }
                None => String::new(),
            });
            let valid = !id.text().is_empty() && (!path.is_empty() || !url.text().is_empty());
            add.set_sensitive(valid && ack.is_active());
        })
    };
    for e in [&id, &file, &url] {
        let u = update.clone();
        e.connect_changed(move |_| u());
    }
    {
        let u = update.clone();
        ack.connect_toggled(move |_| u());
    }
    {
        let (a, file) = (app.clone(), file.clone());
        fr.append(&ui::button("Browse…", move || {
            let (a, file) = (a.clone(), file.clone());
            glib::spawn_future_local(async move {
                if let Some(p) = dialogs::open_file(a.win.upcast_ref(), "Pick a script", None).await {
                    file.set_text(&p.display().to_string());
                }
            });
        }));
    }
    c.append(&ack);
    {
        let a = app.clone();
        add.connect_clicked(move |_| {
            let (f, u, s) = (file.text().to_string(), url.text().to_string(), sha.text().to_string());
            let script = config::Script { id: id.text().to_string(), file: (!f.is_empty()).then(|| f.clone()), url: (f.is_empty() && !u.is_empty()).then(|| u.clone()), sha256: (f.is_empty() && !s.is_empty()).then(|| s.clone()), ..Default::default() };
            a.edit(move |m| m.cfg.scripts.push(script));
            a.rebuild(Page::Scripts);
        });
    }
    c.append(&add);
}
