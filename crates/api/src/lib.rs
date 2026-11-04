//! HTTP API for the grocery-routes product shell (M4).
//!
//! Provides a REST API built on axum + SQLite for importing problems,
//! solving them (greedy or LNS/ALNS, with optional OSM routing), and
//! retrieving solutions and KPIs.
//!
//! Run with [`crate::server::serve`].

pub mod db;
pub mod handlers;
pub mod server;

pub use handlers::router;
