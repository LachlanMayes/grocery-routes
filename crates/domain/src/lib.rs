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
//! - Every timestamp is horizon-relative. When [`Problem::horizon_start`] is
//!   set, [`clock`] converts horizon minutes to local civil time at each
//!   depot/store using its IANA `timezone`.

pub mod clock;
pub mod error;
pub mod geo;
pub mod holidays;
pub mod ids;
pub mod ledger;
pub mod model;
pub mod time;

pub use clock::{
    ClockError, is_valid_timezone, local_date, local_time, local_time_short, local_zoned,
    parse_horizon_start,
};
pub use error::Violation;
pub use geo::{LatLon, haversine_km};
pub use holidays::{Holiday, holidays_in_year, is_public_holiday, national_public_holidays};
pub use ids::{DepotId, DriverId, StoreId, VehicleId};
pub use ledger::{ReturnablesLedger, StoreBalance, ledger};
pub use model::{
    Chain, ChainRules, CostModel, CurfewRule, CurfewRules, Depot, Driver, FatigueRules, Load, Node,
    PlannedStop, Problem, ReturnablesDefaults, RoutePlan, Shift, Solution, Store, TimeWindow,
    Vehicle,
};
pub use time::Minutes;
