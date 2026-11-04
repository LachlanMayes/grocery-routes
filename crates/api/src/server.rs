//! HTTP server entry point for the M4 product shell API.
//!
//! Runs the axum router on a TCP listener with graceful shutdown.

use std::net::SocketAddr;

use crate::db::open_db;
use crate::handlers::router;

/// Serve the API on `addr`, storing data in the SQLite database at `db_path`.
pub async fn serve(addr: SocketAddr, db_path: &std::path::Path) -> anyhow::Result<()> {
    let pool = open_db(db_path).await?;
    let app = router(pool);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", addr);

    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
