//! egui desktop console entry point.
//!
//! Run with `cargo run -p console`.

use console::ConsoleApp;
use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("Grocery Routes — Ops Console"),
        ..Default::default()
    };
    eframe::run_native(
        "Grocery Routes Console",
        options,
        Box::new(|_cc| Ok(Box::<ConsoleApp>::default())),
    )
}
