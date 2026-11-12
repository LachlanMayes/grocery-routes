//! Standalone API server for the operator console.
//!
//! Run with `cargo run -p api`. The console (`cargo run -p console`) talks to
//! `http://127.0.0.1:8080` by default, which is also this server's default
//! address. The SQLite database is created if it does not exist.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;

/// Command-line arguments of the API server.
#[derive(Debug, Parser)]
#[command(
    name = "grocery-routes-api",
    about = "HTTP API backing the grocery-routes operator console"
)]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8080")]
    addr: SocketAddr,
    /// SQLite database file (created if missing).
    #[arg(long, default_value = "data/api.sqlite")]
    db: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if let Some(parent) = args.db.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating database directory {}", parent.display()))?;
    }

    println!(
        "grocery-routes API listening on http://{} (database: {})",
        args.addr,
        args.db.display()
    );
    api::server::serve(args.addr, &args.db).await
}
