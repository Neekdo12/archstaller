//! The Lua source page: a GtkSourceView editor for the config's text. Source and form editing are
//! separate modes: entering source mode serializes the form once; the forms stay unavailable while
//! source mode is on, and leaving it needs a successful evaluation by the same loader the CLI uses.
use crate::app::App;
use crate::completion::{self, Item, Sources};
use crate::dialogs;
use crate::model::located;
use crate::ui::{self, Kind};
use gtk::glib;
use gtk::prelude::*;
use sourceview5::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

fn scheme_buffer(app: &Rc<App>, text: &str) -> (sourceview5::Buffer, bool) {
    let buf = sourceview5::Buffer::new(None);
    let lang = sourceview5::LanguageManager::default().language("lua");
    let has_lang = lang.is_some();
    buf.set_language(lang.as_ref());
    buf.set_highlight_syntax(true);
    let dark = ui::is_dark(&app.win);
    let mgr = sourceview5::StyleSchemeManager::default();
    let scheme = [if dark { "Adwaita-dark" } else { "Adwaita" }, if dark { "classic-dark" } else { "classic" }].iter().find_map(|n| mgr.scheme(n));
    buf.set_style_scheme(scheme.as_ref());
    buf.set_text(text);
    buf.set_enable_undo(true);
    (buf, has_lang)
}

fn editor_view(buf: &sourceview5::Buffer, editable: bool) -> sourceview5::View {
    let v = sourceview5::View::with_buffer(buf);
    v.set_monospace(true);
    v.set_show_line_numbers(true);
    v.set_highlight_current_line(editable);
    v.set_auto_indent(true);
    v.set_indent_width(2);
    v.set_tab_width(2);
    v.set_insert_spaces_instead_of_tabs(true);
    v.set_editable(editable);
    v.set_cursor_visible(editable);
    v.set_left_margin(6);
    v.set_top_margin(4);
    v
}

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let (raw, foreign, text) = {
        let st = app.st.borrow();
        (st.model.raw.is_some(), st.model.foreign, st.model.lua())
    };
    if !raw {
        form_mode(app, content, &text);
    } else {
        source_mode(app, content, &text, foreign);
    }
}

/// The generated text, read-only, and the way into source mode.
fn form_mode(app: &Rc<App>, content: &gtk::Box, text: &str) {
    ui::heading(content, "Lua source", "The file this config is saved as. It is plain Lua; the forms and this text describe the same config. Source mode lets you edit the text itself.");
    let c = ui::card(content, None);
    let (buf, has_lang) = scheme_buffer(app, text);
    let view = editor_view(&buf, false);
    let scroll = gtk::ScrolledWindow::builder().min_content_height(480).vexpand(true).has_frame(true).child(&view).build();
    c.append(&scroll);
    if !has_lang {
        ui::hint(&c, "Lua highlighting is not available (the GtkSourceView Lua language definition was not found); the text is shown plain.");
    }
    let r = ui::hbox(8);
    let a = app.clone();
    r.append(&ui::primary("Edit source", move || {
        a.st.borrow_mut().model.enter_source();
        a.rebuild_all();
        a.set_status("Source mode: the forms are unavailable until you return to them");
    }));
    let a = app.clone();
    r.append(&ui::button("Copy", move || {
        let t = a.st.borrow().model.lua();
        a.copy_text(&t, "Lua");
    }));
    c.append(&r);
}

struct Completion {
    pop: gtk::Popover,
    list: gtk::ListBox,
    items: RefCell<Vec<Item>>,
    prefix_chars: Cell<usize>,
}

fn source_mode(app: &Rc<App>, content: &gtk::Box, text: &str, foreign: bool) {
    ui::heading(content, "Lua source", if foreign { "This file was not written by the GUI, so it is edited as text and checked as it is." } else { "Source mode: edit the text. The forms are unavailable until you return to them." });
    let c = ui::card(content, None);
    let (buf, has_lang) = scheme_buffer(app, text);
    let view = editor_view(&buf, true);

    // Toolbar.
    let bar = ui::hbox(8);
    let search_toggle = gtk::ToggleButton::builder().icon_name("edit-find-symbolic").tooltip_text("Search (Ctrl+F)").build();
    search_toggle.update_property(&[gtk::accessible::Property::Label("Search in the source")]);
    bar.append(&search_toggle);
    let hint = ui::dim("Ctrl+Space: suggestions");
    hint.set_hexpand(true);
    bar.append(&hint);
    let diag_btn = gtk::Button::with_label("Check");
    bar.append(&diag_btn);
    let back = ui::primary("Return to the forms", || {});
    bar.append(&back);
    let discard = gtk::Button::with_label(if foreign { "Replace with a form-based starter" } else { "Discard source edits" });
    bar.append(&discard);
    c.append(&bar);

    // Search.
    let settings = sourceview5::SearchSettings::new();
    settings.set_wrap_around(true);
    let search_ctx = sourceview5::SearchContext::new(&buf, Some(&settings));
    search_ctx.set_highlight(true);
    let search = gtk::SearchEntry::new();
    let revealer = gtk::Revealer::builder().child(&search).reveal_child(false).build();
    c.append(&revealer);
    {
        let r = revealer.clone();
        let s = search.clone();
        search_toggle.connect_toggled(move |t| {
            r.set_reveal_child(t.is_active());
            if t.is_active() {
                s.grab_focus();
            }
        });
    }
    {
        let settings = settings.clone();
        search.connect_search_changed(move |e| settings.set_search_text(Some(e.text().as_str()).filter(|t| !t.is_empty())));
    }
    {
        let (buf, view, ctx) = (buf.clone(), view.clone(), search_ctx.clone());
        search.connect_activate(move |_| {
            let from = buf.selection_bounds().map(|(_, e)| e).unwrap_or_else(|| buf.iter_at_mark(&buf.get_insert()));
            if let Some((mut s, e, _)) = ctx.forward(&from) {
                buf.select_range(&s, &e);
                view.scroll_to_iter(&mut s, 0.1, false, 0.0, 0.0);
            }
        });
    }

    let scroll = gtk::ScrolledWindow::builder().min_content_height(420).vexpand(true).has_frame(true).child(&view).build();
    c.append(&scroll);
    if !has_lang {
        ui::hint(&c, "Lua highlighting is not available (the GtkSourceView Lua language definition was not found); editing works as plain text.");
    }

    // Diagnostics: the loader's message, and a jump to its line when it names one.
    let diag = ui::vbox(4);
    c.append(&diag);
    let show_diag: Rc<dyn Fn()> = {
        let (a, buf, view, diag) = (app.clone(), buf.clone(), view.clone(), diag.clone());
        Rc::new(move || {
            ui::clear(&diag);
            let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
            let r = a.st.borrow().model.check_source(&text);
            match r {
                Ok(()) => {
                    ui::note(&diag, Kind::Ok, "The text evaluates and passes the same checks as the command line.");
                }
                Err(e) => {
                    let first = e.lines().next().unwrap_or("").to_string();
                    let l = ui::note(&diag, Kind::Bad, &first);
                    l.set_selectable(true);
                    if let Some(n) = located(&e) {
                        let (buf, view) = (buf.clone(), view.clone());
                        diag.append(&ui::button(&format!("Go to line {n}"), move || {
                            if let Some(mut it) = buf.iter_at_line(n as i32 - 1) {
                                buf.place_cursor(&it);
                                view.scroll_to_iter(&mut it, 0.1, true, 0.0, 0.3);
                                view.grab_focus();
                            }
                        }));
                    } else {
                        ui::hint(&diag, "No line is known for this message.");
                    }
                }
            }
        })
    };
    show_diag();
    {
        let sd = show_diag.clone();
        diag_btn.connect_clicked(move |_| sd());
    }

    // Buffer -> model, and the debounced diagnostics.
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    {
        let (a, sd, pending) = (app.clone(), show_diag.clone(), pending.clone());
        buf.connect_changed(move |b| {
            let t = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
            a.edit(move |m| m.raw = Some(t));
            if let Some(id) = pending.borrow_mut().take() {
                id.remove();
            }
            let (sd, pend) = (sd.clone(), pending.clone());
            let id = glib::timeout_add_local_once(Duration::from_millis(500), move || {
                pend.borrow_mut().take();
                sd();
            });
            *pending.borrow_mut() = Some(id);
        });
    }

    // Leaving source mode.
    {
        let a = app.clone();
        back.connect_clicked(move |_| {
            let a = a.clone();
            glib::spawn_future_local(async move {
                let foreign = a.st.borrow().model.foreign;
                if foreign && !dialogs::confirm(a.win.upcast_ref(), "Replace the file's text with form output?", "This file was not written by the GUI. The forms can only represent what they know: comments, helper modules and custom Lua in the file are lost when it is saved from the forms.", "Replace with form output").await {
                    return;
                }
                let r = a.st.borrow_mut().model.leave_source();
                match r {
                    Ok(()) => {
                        a.rebuild_all();
                        a.set_status("Back to the forms");
                    }
                    Err(e) => a.set_status(format!("Stayed in source mode: {}", e.lines().next().unwrap_or(""))),
                }
            });
        });
    }
    {
        let a = app.clone();
        discard.connect_clicked(move |_| {
            let a = a.clone();
            glib::spawn_future_local(async move {
                let foreign = a.st.borrow().model.foreign;
                if foreign {
                    if dialogs::confirm(a.win.upcast_ref(), "Replace this file's text with a starter?", "The text in the editor is dropped for a new form-based starter config (the file on disk changes only when you save).", "Replace").await {
                        let p = a.st.borrow().model.path.clone();
                        let mut m = crate::model::Model::starter();
                        m.path = p;
                        m.dirty = true;
                        a.load(m, "Replaced with a form-based starter".into());
                    }
                } else if dialogs::confirm(a.win.upcast_ref(), "Discard the source edits?", "The text goes back to what the forms say.", "Discard").await {
                    {
                        let mut st = a.st.borrow_mut();
                        st.model.raw = None;
                    }
                    a.rebuild_all();
                }
            });
        });
    }

    // Completion on Ctrl+Space.
    let comp = Rc::new(Completion { pop: gtk::Popover::new(), list: gtk::ListBox::new(), items: RefCell::new(vec![]), prefix_chars: Cell::new(0) });
    comp.pop.set_parent(&view);
    comp.pop.set_autohide(false);
    comp.pop.set_has_arrow(false);
    comp.pop.set_position(gtk::PositionType::Bottom);
    comp.list.set_selection_mode(gtk::SelectionMode::Single);
    comp.pop.set_child(Some(&gtk::ScrolledWindow::builder().child(&comp.list).min_content_width(380).max_content_height(240).propagate_natural_height(true).build()));
    let fields = Rc::new(completion::schema_fields());
    let compute: Rc<dyn Fn() -> Vec<Item>> = {
        let (a, buf, comp, fields) = (app.clone(), buf.clone(), comp.clone(), fields.clone());
        Rc::new(move || {
            let cursor = buf.iter_at_mark(&buf.get_insert());
            let before = buf.text(&buf.start_iter(), &cursor, false).to_string();
            let ctx = completion::context(&before);
            let src = {
                let st = a.st.borrow();
                Sources {
                    services: st.lists.services.clone(),
                    kernel_params: st.lists.kernel_params.clone(),
                    timezones: st.lists.timezones.clone(),
                    locales: st.lists.locales.clone(),
                    keymaps: st.lists.keymaps.clone(),
                    shells: st.lists.shells.clone(),
                    groups: st.lists.groups.clone(),
                    packages: match &st.resolve {
                        Some(Ok(r)) => r.packages.iter().map(|p| p.name.clone()).collect(),
                        _ => vec![],
                    },
                }
            };
            comp.prefix_chars.set(ctx.prefix.chars().count());
            completion::candidates(&ctx, &src, &fields)
        })
    };
    let fill: Rc<dyn Fn(Vec<Item>)> = {
        let (comp, view, buf) = (comp.clone(), view.clone(), buf.clone());
        Rc::new(move |items: Vec<Item>| {
            while let Some(c) = comp.list.first_child() {
                comp.list.remove(&c);
            }
            if items.is_empty() {
                comp.pop.popdown();
                comp.items.borrow_mut().clear();
                return;
            }
            for i in &items {
                let r = ui::hbox(10);
                r.append(&gtk::Label::builder().label(&i.label).xalign(0.0).build());
                let d = ui::dim(&i.detail);
                d.set_ellipsize(gtk::pango::EllipsizeMode::End);
                d.set_max_width_chars(46);
                r.append(&d);
                comp.list.append(&r);
            }
            if let Some(first) = comp.list.row_at_index(0) {
                comp.list.select_row(Some(&first));
            }
            *comp.items.borrow_mut() = items;
            let it = buf.iter_at_mark(&buf.get_insert());
            let rect = view.iter_location(&it);
            let (x, y) = view.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y() + rect.height());
            comp.pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x, y, 1, 1)));
            comp.pop.popup();
        })
    };
    let accept: Rc<dyn Fn()> = {
        let (comp, buf) = (comp.clone(), buf.clone());
        Rc::new(move || {
            let Some(row) = comp.list.selected_row() else { return };
            let Some(item) = comp.items.borrow().get(row.index() as usize).cloned() else { return };
            comp.pop.popdown();
            let mut end = buf.iter_at_mark(&buf.get_insert());
            let mut start = end;
            start.backward_chars(comp.prefix_chars.get() as i32);
            buf.begin_user_action();
            buf.delete(&mut start, &mut end);
            buf.insert(&mut start, &item.insert);
            buf.end_user_action();
        })
    };
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (comp, compute, fill, accept, revealer, toggle) = (comp.clone(), compute.clone(), fill.clone(), accept.clone(), revealer.clone(), search_toggle.clone());
        key.connect_key_pressed(move |_, k, _, mods| {
            use gtk::gdk::Key;
            let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            if ctrl && k == Key::space {
                fill(compute());
                return glib::Propagation::Stop;
            }
            if ctrl && (k == Key::f || k == Key::F) {
                toggle.set_active(true);
                return glib::Propagation::Stop;
            }
            if k == Key::Escape && revealer.reveals_child() && !comp.pop.is_visible() {
                toggle.set_active(false);
                return glib::Propagation::Stop;
            }
            if comp.pop.is_visible() {
                let n = comp.items.borrow().len() as i32;
                let cur = comp.list.selected_row().map(|r| r.index()).unwrap_or(0);
                match k {
                    Key::Down => {
                        if let Some(r) = comp.list.row_at_index((cur + 1).min(n - 1)) {
                            comp.list.select_row(Some(&r));
                        }
                        return glib::Propagation::Stop;
                    }
                    Key::Up => {
                        if let Some(r) = comp.list.row_at_index((cur - 1).max(0)) {
                            comp.list.select_row(Some(&r));
                        }
                        return glib::Propagation::Stop;
                    }
                    Key::Return | Key::KP_Enter | Key::Tab => {
                        accept();
                        return glib::Propagation::Stop;
                    }
                    Key::Escape => {
                        comp.pop.popdown();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
            }
            glib::Propagation::Proceed
        });
    }
    view.add_controller(key);
    // While the popup is open, typing narrows it; moving away closes it.
    {
        let (comp, compute, fill) = (comp.clone(), compute.clone(), fill.clone());
        buf.connect_changed(move |_| {
            if comp.pop.is_visible() {
                fill(compute());
            }
        });
    }
    {
        let comp = comp.clone();
        view.connect_unrealize(move |_| comp.pop.unparent());
    }
}
