//! Small GTK helpers: the page, card and row layouts and the entry flavours the pages share.
//! Nothing here knows the config; it only builds widgets.
use gtk::prelude::*;
use std::rc::Rc;

/// The application stylesheet. The status colours depend on whether the theme is dark, which the
/// plain toolkit has no CSS switch for, so the caller says.
pub fn css(dark: bool) -> String {
    let (warn, bad, ok) = if dark { ("#f5c211", "#ff7b63", "#8ff0a4") } else { ("#9c6e03", "#c01c28", "#1a7f4b") };
    format!(
        "
.page-title {{ font-size: 150%; font-weight: 700; }}
.page-sub {{ opacity: 0.7; }}
.card-box {{ background: alpha(currentColor, 0.05); border-radius: 8px; padding: 12px 14px; }}
.card-title {{ font-weight: 700; opacity: 0.85; }}
.hint {{ opacity: 0.65; font-size: 90%; }}
.note-warn {{ color: {warn}; }}
.note-bad {{ color: {bad}; }}
.note-ok {{ color: {ok}; }}
.chip {{ border-radius: 99px; padding: 1px 9px; font-size: 85%; background: alpha(currentColor, 0.1); }}
.chip-ok {{ background: alpha({ok}, 0.25); }}
.chip-warn {{ background: alpha({warn}, 0.25); }}
.chip-accent {{ background: alpha(@accent_bg_color, 0.30); }}
.nav-bad {{ color: {bad}; font-weight: 700; }}
.mono {{ font-family: monospace; }}
.risky {{ color: {warn}; }}
.problem-bar {{ background: alpha(currentColor, 0.06); padding: 4px 10px; }}
.sidebar-list row {{ padding: 4px 6px; }}
/* Check and radio indicators keep one size on every theme and scale. */
check, radio {{ min-width: 16px; min-height: 16px; -gtk-icon-size: 16px; }}
"
    )
}

/// Whether the active theme draws light text on a dark background.
pub fn is_dark(w: &impl IsA<gtk::Widget>) -> bool {
    let c = w.color();
    0.299 * c.red() + 0.587 * c.green() + 0.114 * c.blue() > 0.5
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Warn,
    Bad,
    Ok,
}

impl Kind {
    fn class(self) -> &'static str {
        match self {
            Kind::Warn => "note-warn",
            Kind::Bad => "note-bad",
            Kind::Ok => "note-ok",
        }
    }
}

pub fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

pub fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

/// The scrolling container of one page and the box the page fills.
pub fn page_shell() -> (gtk::ScrolledWindow, gtk::Box) {
    let content = vbox(12);
    content.set_margin_top(18);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_hexpand(true);
    let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).hexpand(true).child(&content).build();
    (scroll, content)
}

pub fn heading(content: &gtk::Box, title: &str, sub: &str) {
    let t = gtk::Label::builder().label(title).xalign(0.0).css_classes(["page-title"]).build();
    content.append(&t);
    if !sub.is_empty() {
        content.append(&gtk::Label::builder().label(sub).xalign(0.0).wrap(true).css_classes(["page-sub"]).build());
    }
}

pub fn card(content: &gtk::Box, title: Option<&str>) -> gtk::Box {
    let c = vbox(8);
    c.add_css_class("card-box");
    if let Some(t) = title {
        c.append(&gtk::Label::builder().label(t).xalign(0.0).css_classes(["card-title"]).build());
    }
    content.append(&c);
    c
}

/// A labelled row: a fixed-width label and the control that takes the rest.
pub fn row(parent: &gtk::Box, label: &str, w: &impl IsA<gtk::Widget>) -> gtk::Box {
    let r = hbox(12);
    let l = gtk::Label::builder().label(label).xalign(0.0).width_request(130).valign(gtk::Align::Center).build();
    r.append(&l);
    w.set_hexpand(true);
    // The label names the control for screen readers.
    w.upcast_ref::<gtk::Widget>().update_relation(&[gtk::accessible::Relation::LabelledBy(&[l.upcast_ref::<gtk::Accessible>()])]);
    r.append(w);
    parent.append(&r);
    r
}

pub fn hint(parent: &gtk::Box, text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).selectable(false).css_classes(["hint"]).build();
    parent.append(&l);
    l
}

pub fn note(parent: &gtk::Box, kind: Kind, text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes([kind.class()]).build();
    parent.append(&l);
    l
}

pub fn chip(text: &str, extra: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).valign(gtk::Align::Center).build();
    l.add_css_class("chip");
    if !extra.is_empty() {
        l.add_css_class(extra);
    }
    l
}

pub fn strong(text: &str) -> gtk::Label {
    gtk::Label::builder().label(format!("<b>{}</b>", gtk::glib::markup_escape_text(text))).use_markup(true).xalign(0.0).build()
}

pub fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["hint"]).build()
}

pub fn button(label: &str, on_click: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.connect_clicked(move |_| on_click());
    b
}

pub fn primary(label: &str, on_click: impl Fn() + 'static) -> gtk::Button {
    let b = button(label, on_click);
    b.add_css_class("suggested-action");
    b
}

/// A check button (or radio, in a group) whose label wraps instead of widening the window.
pub fn check_button(label: &str) -> gtk::CheckButton {
    let c = gtk::CheckButton::new();
    let l = gtk::Label::builder().label(label).wrap(true).xalign(0.0).build();
    c.set_child(Some(&l));
    c
}

pub fn check(label: &str, active: bool, on_toggle: impl Fn(bool) + 'static) -> gtk::CheckButton {
    let c = check_button(label);
    c.set_active(active);
    c.connect_toggled(move |c| on_toggle(c.is_active()));
    c
}

pub fn entry(initial: &str, on_change: impl Fn(&str) + 'static) -> gtk::Entry {
    let e = gtk::Entry::builder().text(initial).hexpand(true).build();
    e.connect_changed(move |e| on_change(e.text().as_str()));
    e
}

pub fn password(on_change: impl Fn(&str) + 'static) -> gtk::PasswordEntry {
    let e = gtk::PasswordEntry::builder().show_peek_icon(true).hexpand(true).build();
    e.connect_changed(move |e| on_change(e.text().as_str()));
    e
}

/// A text entry with a completion popup over `options`. With `list`, the value is a comma-separated
/// list and the completion works on the last item. Free text stays allowed: the options are hints.
#[allow(deprecated)]
pub fn suggest(initial: &str, options: Rc<Vec<String>>, list: bool, on_change: impl Fn(&str) + 'static) -> gtk::Entry {
    let e = entry(initial, on_change);
    let store = gtk::ListStore::new(&[String::static_type()]);
    for o in options.iter() {
        store.set(&store.append(), &[(0, o)]);
    }
    let c = gtk::EntryCompletion::new();
    c.set_model(Some(&store));
    c.set_text_column(0);
    c.set_minimum_key_length(1);
    c.set_inline_completion(false);
    c.set_popup_single_match(true);
    let last = move |text: &str| -> String {
        if list {
            text.rsplit(',').next().unwrap_or("").trim().to_lowercase()
        } else {
            text.trim().to_lowercase()
        }
    };
    let opts = options.clone();
    c.set_match_func(move |comp, key, iter| {
        let _ = (comp, key);
        let model = comp.model().unwrap();
        let text: String = model.get(iter, 0);
        let typed = last(&comp.entry().map(|e| e.text().to_string()).unwrap_or_default());
        let _ = &opts;
        !typed.is_empty() && text.to_lowercase().contains(&typed) && text.to_lowercase() != typed
    });
    c.connect_match_selected(move |comp, model, iter| {
        let picked: String = model.get(iter, 0);
        if let Some(entry) = comp.entry() {
            let text = entry.text().to_string();
            let new = if list {
                match text.rfind(',') {
                    Some(i) => format!("{}, {picked}", &text[..i]),
                    None => picked,
                }
            } else {
                picked
            };
            entry.set_text(&new);
            entry.set_position(-1);
        }
        gtk::glib::Propagation::Stop
    });
    e.set_completion(Some(&c));
    e
}

pub fn spin(value: u32, min: u32, max: u32, on_change: impl Fn(u32) + 'static) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min as f64, max as f64, 1.0);
    s.set_value(value as f64);
    s.set_hexpand(false);
    s.connect_value_changed(move |s| on_change(s.value() as u32));
    s
}

/// A multi-line editor for a list: one item per line, empty lines dropped.
pub fn lines_editor(initial: &[String], height: i32, on_change: impl Fn(Vec<String>) + 'static) -> gtk::ScrolledWindow {
    let view = gtk::TextView::builder().monospace(true).left_margin(8).right_margin(8).top_margin(6).bottom_margin(6).wrap_mode(gtk::WrapMode::None).build();
    view.buffer().set_text(&initial.join("\n"));
    view.buffer().connect_changed(move |b| {
        let text = b.text(&b.start_iter(), &b.end_iter(), false);
        on_change(text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect());
    });
    gtk::ScrolledWindow::builder().min_content_height(height).max_content_height(height).child(&view).has_frame(true).build()
}

pub fn spinner_row(text: &str) -> (gtk::Box, gtk::Spinner) {
    let b = hbox(8);
    let s = gtk::Spinner::new();
    s.start();
    b.append(&s);
    b.append(&dim(text));
    (b, s)
}

pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}
