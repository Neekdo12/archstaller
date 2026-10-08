//! The command launcher (Ctrl+K or "/"): one searchable list of the window's actions and pages.
//! Every entry runs the same code as its menu item or button, including the confirmations those ask.
use crate::app::{App, Page, PAGES};
use crate::ui;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;

struct Command {
    title: String,
    hint: String,
    run: Rc<dyn Fn()>,
}

fn commands(app: &Rc<App>) -> Vec<Command> {
    let mut v: Vec<Command> = Vec::new();
    let mut add = |title: &str, hint: &str, f: Rc<dyn Fn()>| v.push(Command { title: title.into(), hint: hint.into(), run: f });
    let a = app.clone();
    add("New config", "Ctrl+N", Rc::new(move || a.new_doc()));
    let a = app.clone();
    add("Open a config…", "Ctrl+O", Rc::new(move || a.open_doc()));
    let a = app.clone();
    add("Save", "Ctrl+S", Rc::new(move || a.save()));
    let a = app.clone();
    add("Save as…", "Ctrl+Shift+S", Rc::new(move || a.save_as()));
    let a = app.clone();
    add("Build the ISO", "Build ISO page", Rc::new(move || {
        a.show(Page::Iso);
        crate::pages::iso::start_from_action(&a);
    }));
    let a = app.clone();
    add("Copy the config as Lua", "", Rc::new(move || {
        let t = a.st.borrow().model.lua();
        a.copy_text(&t, "Lua");
    }));
    let a = app.clone();
    add("Copy the ISO's SHA-256", "", Rc::new(move || {
        let sha = a.st.borrow().build.iso.as_ref().map(|i| i.2.clone());
        match sha {
            Some(s) => a.copy_text(&s, "SHA-256"),
            None => a.set_status("no ISO built yet"),
        }
    }));
    for (page, _, name, _) in PAGES {
        let (a, page) = (app.clone(), *page);
        add(&format!("Go to {name}"), "page", Rc::new(move || a.show(page)));
    }
    let presets: Vec<String> = app.st.borrow().presets.iter().map(|p| p.name.clone()).collect();
    for name in presets {
        let a = app.clone();
        let n = name.clone();
        add(&format!("New from preset: {name}"), "preset", Rc::new(move || a.preset_doc(n.clone())));
    }
    v
}

pub fn open(app: &Rc<App>) {
    let cmds = Rc::new(commands(app));
    let win = gtk::Window::builder().transient_for(&app.window()).modal(true).decorated(false).default_width(540).default_height(380).resizable(false).build();
    let b = ui::vbox(6);
    b.set_margin_top(10);
    b.set_margin_bottom(10);
    b.set_margin_start(10);
    b.set_margin_end(10);
    let search = gtk::SearchEntry::builder().placeholder_text("Type a command…").hexpand(true).build();
    b.append(&search);
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).activate_on_single_click(true).build();
    b.append(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build());
    b.append(&ui::dim("↑↓ choose · Enter run · Esc close"));
    win.set_child(Some(&b));

    let shown: Rc<std::cell::RefCell<Vec<usize>>> = Rc::new(std::cell::RefCell::new(vec![]));
    let fill = {
        let (cmds, list, shown) = (cmds.clone(), list.clone(), shown.clone());
        Rc::new(move |q: &str| {
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            let q = q.to_lowercase();
            let mut idx: Vec<usize> = (0..cmds.len()).filter(|&i| q.split_whitespace().all(|w| cmds[i].title.to_lowercase().contains(w))).collect();
            idx.sort_by_key(|&i| !cmds[i].title.to_lowercase().starts_with(&q));
            for &i in &idx {
                let r = ui::hbox(10);
                let l = gtk::Label::builder().label(&cmds[i].title).xalign(0.0).hexpand(true).build();
                r.append(&l);
                r.append(&ui::dim(&cmds[i].hint));
                list.append(&r);
            }
            if let Some(first) = list.row_at_index(0) {
                list.select_row(Some(&first));
            }
            *shown.borrow_mut() = idx;
        })
    };
    fill("");
    {
        let fill = fill.clone();
        search.connect_search_changed(move |e| fill(e.text().as_str()));
    }
    let run_selected = {
        let (cmds, list, shown, win) = (cmds.clone(), list.clone(), shown.clone(), win.clone());
        Rc::new(move || {
            if let Some(row) = list.selected_row() {
                if let Some(&i) = shown.borrow().get(row.index() as usize) {
                    let f = cmds[i].run.clone();
                    win.close();
                    // Run after the launcher is gone, so a dialog opened by the command gets the focus.
                    glib::idle_add_local_once(move || f());
                }
            }
        })
    };
    {
        let r = run_selected.clone();
        search.connect_activate(move |_| r());
    }
    {
        let r = run_selected.clone();
        list.connect_row_activated(move |_, _| r());
    }
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (list, win) = (list.clone(), win.clone());
        key.connect_key_pressed(move |_, k, _, _| {
            use gtk::gdk::Key;
            let cur = list.selected_row().map(|r| r.index()).unwrap_or(-1);
            match k {
                Key::Escape => {
                    win.close();
                    glib::Propagation::Stop
                }
                Key::Down => {
                    if let Some(r) = list.row_at_index(cur + 1) {
                        list.select_row(Some(&r));
                    }
                    glib::Propagation::Stop
                }
                Key::Up => {
                    if let Some(r) = list.row_at_index((cur - 1).max(0)) {
                        list.select_row(Some(&r));
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
    }
    win.add_controller(key);
    win.present();
    search.grab_focus();
}
