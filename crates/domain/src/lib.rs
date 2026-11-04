//! Core domain model for the grocery-routes system: route planning for
//! Australian supermarket distribution (Coles, Woolworths, ALDI, IGA/Metcash).
//!
//! The model is deliberately **timeline-based**: every timestamp is an offset
//! in [`Minutes`] from the start of a planning horizon. A horizon may span
//! several days, which is how overnight relay runs (Sydney → Albury →
//! Melbourne with a driver hand-over) are represented without a second model.
//!
//! Conventions:
//! - `*Id` newtypes index into the corresponding `Vec` on [`Problem`];
//!   look-ups go through the accessor methods, which are bounds-checked.
//! - Shifts and receiving windows use half-open `[start, end)` semantics.
//! - All money values are Australian cents; all distances are kilometres.

pub mod error;
pub mod geo;
pub mod ids;
pub mod model;
pub mod time;

pub use error::Violation;
pub use geo::{LatLon, haversine_km};
pub use ids::{DepotId, DriverId, StoreId, VehicleId};
pub use model::{
    Chain, ChainRules, CostModel, CurfewRules, Depot, Driver, FatigueRules, Load, Node,
    PlannedStop, Problem, RoutePlan, Shift, Solution, Store, TimeWindow, Vehicle,
};
pub use time::Minutes;
