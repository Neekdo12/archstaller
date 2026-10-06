//! archstaler-gui: compose an archstaler config, check it with the same code as the CLI, build the
//! ISO through `cargo xtask`, and copy it onto a Ventoy stick. Host only.
mod app;
mod aur;
mod build;
mod data;
mod media;
mod model;
mod style;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1100.0, 760.0]).with_title("Archstaler"),
        ..Default::default()
    };
    eframe::run_native("Archstaler", options, Box::new(|cc| {
            style::apply(&cc.egui_ctx);
            Ok(Box::new(app::App::new()))
        }))
}
