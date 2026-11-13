//! egui desktop console entry point.
//!
//! Run with `cargo run -p console`. Embeds the grocery-routes API in-process
//! on `127.0.0.1:8080` (SQLite database `data/demo/api.sqlite`, created if
//! missing) so a single command starts everything. Pass `--no-embedded-api`
//! to run the UI against an already-running API instead; change the address
//! or database with `--addr` / `--db`.

use clap::Parser as _;
use console::ConsoleApp;
use eframe::egui;

/// Command-line arguments of the console.
#[derive(Debug, clap::Parser)]
#[command(
    name = "grocery-routes-console",
    about = "Operator console for grocery-routes (embeds the API by default)"
)]
struct Args {
    /// Address the embedded API listens on.
    #[arg(long, default_value = "127.0.0.1:8080")]
    addr: std::net::SocketAddr,
    /// SQLite database file for the embedded API (created if missing).
    #[arg(long, default_value = "data/demo/api.sqlite")]
    db: std::path::PathBuf,
    /// Do not embed the API; point the UI at an already-running API on
    /// `--addr` instead.
    #[arg(long)]
    no_embedded_api: bool,
}

fn main() -> eframe::Result {
    let args = Args::parse();

    // Create a Tokio runtime that lives on a background thread so that
    // background API work (import, solve, SSE polling) has a runtime to
    // spawn tasks on. The egui main thread never blocks; it only drives UI.
    let runtime = tokio::runtime::Runtime::new().expect("failed to create Tokio runtime");
    let handle = runtime.handle().clone();

    // Decide base URL and, if embedding, bind the TCP listener before
    // building the UI so ConsoleApp's initial list refresh never races bind.
    let base_url = if args.no_embedded_api {
        eprintln!("console: external API mode — using http://{}", args.addr);
        format!("http://{}", args.addr)
    } else {
        match runtime.block_on(tokio::net::TcpListener::bind(args.addr)) {
            Ok(listener) => {
                if let Some(parent) = args.db.parent()
                    && !parent.as_os_str().is_empty()
                    && let Err(err) = std::fs::create_dir_all(parent)
                {
                    eprintln!(
                        "console: cannot create database directory {}: {err}",
                        parent.display()
                    );
                }
                let db = args.db.clone();
                eprintln!(
                    "console: embedded API — serving http://{} (database: {})",
                    args.addr,
                    args.db.display()
                );
                handle.spawn(async move {
                    if let Err(err) = api::server::serve_with_listener(listener, &db).await {
                        eprintln!("console: embedded API stopped: {err:#}");
                    }
                });
                format!("http://{}", args.addr)
            }
            Err(_) => {
                eprintln!(
                    "console: {} already has a listener — assuming an API is there and using it",
                    args.addr
                );
                format!("http://{}", args.addr)
            }
        }
    };

    std::thread::spawn(move || {
        runtime.block_on(async {
            // Keep the runtime alive for the lifetime of the process.
            std::future::pending::<()>().await;
        });
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("Grocery Routes - Ops Console"),
        ..Default::default()
    };
    eframe::run_native(
        "Grocery Routes Console",
        options,
        Box::new(move |_cc| Ok(Box::new(ConsoleApp::new(handle, base_url)))),
    )
}
