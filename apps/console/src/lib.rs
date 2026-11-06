//! egui desktop console for the grocery-routes product shell (M6).
//!
//! Provides an operator UI for importing problems, solving them (greedy or
//! LNS/ALNS, with optional OSM routing), viewing solutions and KPIs, and
//! watching live solve events via SSE.

mod api_client;
mod app;

pub use app::ConsoleApp;
