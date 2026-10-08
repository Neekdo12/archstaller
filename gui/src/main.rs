//! archstaller-gui: compose an archstaller config, check it with the same code as the CLI, build the
//! ISO through `cargo xtask`, and copy it onto a Ventoy stick or flash it over a USB stick. Host only, Linux, GTK 4.
mod app;
mod aur;
mod build;
mod completion;
mod data;
mod dialogs;
mod flash;
mod launcher;
mod media;
mod model;
mod official;
mod pages;
mod polkit;
mod state;
mod ui;

use gtk::prelude::*;

fn main() -> gtk::glib::ExitCode {
    // Dark by default, whatever the system says: the `Adwaita:dark` theme variant also wins over a desktop
    // portal that reports a light preference. An explicit GTK_THEME of the user is left alone;
    // `ARCHSTALLER_THEME=system` follows the system and `ARCHSTALLER_THEME=light` forces light.
    match std::env::var("ARCHSTALLER_THEME").as_deref() {
        Ok("system") => {}
        Ok("light") => std::env::set_var("GTK_THEME", "Adwaita:light"),
        _ => {
            if std::env::var_os("GTK_THEME").is_none() {
                std::env::set_var("GTK_THEME", "Adwaita:dark");
            }
        }
    }
    let gapp = gtk::Application::builder().application_id("org.archstaller.Gui").flags(gtk::gio::ApplicationFlags::NON_UNIQUE).build();
    gapp.connect_shutdown(|_| polkit::stop());
    gapp.connect_activate(|gapp| {
        polkit::start();
        let app = app::App::new(gapp);
        // The status colours follow the theme's lightness; the theme itself is the system's.
        let css = gtk::CssProvider::new();
        css.load_from_string(&ui::css(ui::is_dark(&app.win)));
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        // `ARCHSTALLER_PAGE=lua` (a page id of the sidebar) opens on that page: handy for screenshots.
        if let Ok(id) = std::env::var("ARCHSTALLER_PAGE") {
            if let Some((page, ..)) = app::PAGES.iter().find(|p| p.1 == id) {
                app.show(*page);
            }
        }
        app.win.present();
    });
    gapp.run()
}
