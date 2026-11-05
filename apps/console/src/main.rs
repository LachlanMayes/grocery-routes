//! egui desktop console entry point.
//!
//! Run with `cargo run -p console`.

use console::ConsoleApp;
use eframe::egui;

fn main() -> eframe::Result {
    // Create a Tokio runtime that lives on a background thread so that
    // background API work (import, solve, SSE polling) has a runtime to
    // spawn tasks on. The egui main thread never blocks; it only drives UI.
    let runtime = tokio::runtime::Runtime::new().expect("failed to create Tokio runtime");
    let handle = runtime.handle().clone();
    std::thread::spawn(move || {
        runtime.block_on(async {
            // Keep the runtime alive for the lifetime of the process.
            std::future::pending::<()>().await;
        });
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("Grocery Routes — Ops Console"),
        ..Default::default()
    };
    eframe::run_native(
        "Grocery Routes Console",
        options,
        Box::new(move |_cc| Ok(Box::new(ConsoleApp::new(handle)))),
    )
}
