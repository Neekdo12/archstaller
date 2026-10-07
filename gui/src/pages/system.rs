use crate::app::App;
use crate::ui;
use std::rc::Rc;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let (cfg, lists) = {
        let st = app.st.borrow();
        (st.model.cfg.clone(), (st.lists.timezones.clone(), st.lists.locales.clone(), st.lists.keymaps.clone()))
    };
    ui::heading(content, "System", "Name and localization of the installed system.");
    let c = ui::card(content, Some("Identity"));
    let a = app.clone();
    ui::row(&c, "Hostname", &ui::entry(&cfg.hostname, move |t| {
        let t = t.to_string();
        a.edit(move |m| m.cfg.hostname = t);
    }));
    let a = app.clone();
    ui::row(&c, "Timezone", &ui::suggest(&cfg.timezone, lists.0, false, move |t| {
        let t = t.to_string();
        a.edit(move |m| m.cfg.timezone = t);
    }));
    let c = ui::card(content, Some("Language"));
    let a = app.clone();
    ui::row(&c, "Locale", &ui::suggest(&cfg.locale, lists.1, false, move |t| {
        let t = t.to_string();
        a.edit(move |m| m.cfg.locale = t);
    }));
    let a = app.clone();
    ui::row(&c, "Keymap", &ui::suggest(&cfg.keymap, lists.2, false, move |t| {
        let t = t.to_string();
        a.edit(move |m| m.cfg.keymap = t);
    }));
}
