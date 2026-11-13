//! egui desktop console for the route planner product shell (M6).
//!
//! Provides an operator UI for building custom general routes (via depot/place
//! addresses, coordinates, or map picking), generating or importing problems,
//! solving them (greedy or LNS/ALNS, with optional OSM routing), viewing
//! solutions and KPIs, and watching live solve events via SSE.

mod api_client;
mod app;
mod geocode;
mod kpis;
mod map_nodes;
mod problem_builder;

pub use app::ConsoleApp;
