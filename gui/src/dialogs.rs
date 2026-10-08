//! File choosers and confirmations (GTK dialogs), as small async helpers. Call them from
//! `glib::spawn_future_local`; they never block the main loop.
use gtk::prelude::*;
use std::path::{Path, PathBuf};

fn filter(name: &str, pattern: &str) -> gtk::FileFilter {
    let f = gtk::FileFilter::new();
    f.set_name(Some(name));
    f.add_pattern(pattern);
    f
}

fn dialog(title: &str, filter_name: Option<(&str, &str)>) -> gtk::FileDialog {
    let d = gtk::FileDialog::builder().title(title).modal(true).build();
    if let Some((n, p)) = filter_name {
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter(n, p));
        d.set_filters(Some(&filters));
    }
    d
}

pub async fn open_file(win: &gtk::Window, title: &str, kind: Option<(&str, &str)>) -> Option<PathBuf> {
    dialog(title, kind).open_future(Some(win)).await.ok().and_then(|f| f.path())
}

pub async fn save_file(win: &gtk::Window, title: &str, name: &str, kind: Option<(&str, &str)>) -> Option<PathBuf> {
    let d = dialog(title, kind);
    d.set_initial_name(Some(name));
    d.save_future(Some(win)).await.ok().and_then(|f| f.path())
}

pub async fn open_folder(win: &gtk::Window, title: &str, start: Option<&Path>) -> Option<PathBuf> {
    let d = dialog(title, None);
    if let Some(s) = start {
        d.set_initial_folder(Some(&gtk::gio::File::for_path(s)));
    }
    d.select_folder_future(Some(win)).await.ok().and_then(|f| f.path())
}

/// Asks a question with a destructive or irreversible flavour; `true` only when the user picks `yes`.
pub async fn confirm(win: &gtk::Window, message: &str, detail: &str, yes: &str) -> bool {
    let d = gtk::AlertDialog::builder().modal(true).message(message).detail(detail).buttons([yes, "Cancel"]).cancel_button(1).default_button(1).build();
    matches!(d.choose_future(Some(win)).await, Ok(0))
}

pub fn notice(win: &gtk::Window, message: &str, detail: &str) {
    let d = gtk::AlertDialog::builder().modal(true).message(message).detail(detail).buttons(["OK"]).build();
    let w = win.clone();
    gtk::glib::spawn_future_local(async move {
        let _ = d.choose_future(Some(&w)).await;
    });
}
