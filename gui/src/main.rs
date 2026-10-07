//! archstaler-gui: compose an archstaler config, check it with the same code as the CLI, build the
//! ISO through `cargo xtask`, and copy it onto a Ventoy stick. Host only, Linux, GTK 4.
mod app;
mod aur;
mod build;
mod completion;
mod data;
mod dialogs;
mod launcher;
mod media;
mod model;
mod pages;
mod state;
mod ui;

use gtk::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let gapp = gtk::Application::builder().application_id("org.archstaler.Gui").build();
    gapp.connect_activate(|gapp| {
        let app = app::App::new(gapp);
        // The status colours follow the theme's lightness; the theme itself is the system's.
        let css = gtk::CssProvider::new();
        css.load_from_string(&ui::css(ui::is_dark(&app.win)));
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        // `ARCHSTALER_PAGE=lua` (a page id of the sidebar) opens on that page: handy for screenshots.
        if let Ok(id) = std::env::var("ARCHSTALER_PAGE") {
            if let Some((page, ..)) = app::PAGES.iter().find(|p| p.1 == id) {
                app.show(*page);
            }
        }
        app.win.present();
    });
    gapp.run()
}
