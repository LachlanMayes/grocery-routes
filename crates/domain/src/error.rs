//! Rule violations reported by [`crate::Solution::validate`].

use thiserror::Error;

use crate::ids::{DepotId, DriverId, StoreId, VehicleId};
use crate::time::Minutes;

/// A rule broken by a [`crate::Solution`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Violation {
    #[error("route {route}: unknown vehicle {vehicle}")]
    UnknownVehicle { route: usize, vehicle: VehicleId },
    #[error("unknown store {store}")]
    UnknownStore { store: StoreId },
    #[error("unknown depot {depot}")]
    UnknownDepot { depot: DepotId },
    #[error("unknown driver {driver}")]
    UnknownDriver { driver: DriverId },
    #[error("route {route}: has no steps")]
    EmptyRoute { route: usize },
    #[error("route {route}: does not start at the vehicle's home depot")]
    RouteNotStartingAtHomeDepot { route: usize },
    #[error("route {route}: does not end at a depot")]
    RouteNotEndingAtDepot { route: usize },
    #[error("vehicle {vehicle} is used by more than one route")]
    VehicleUsedTwice { vehicle: VehicleId },
    #[error("route {route}: no driver assigned")]
    NoDriverAssigned { route: usize },
    #[error("route {route}: timestamps out of order at step {step}")]
    TimestampsOutOfOrder { route: usize, step: usize },
    #[error("route {route}: duration does not match the spanned time")]
    DurationMismatch { route: usize },
    #[error("route {route}: duration {duration} exceeds the vehicle limit {limit}")]
    MaxDurationExceeded {
        route: usize,
        duration: Minutes,
        limit: Minutes,
    },
    #[error("route {route}: load exceeds capacity at step {step}")]
    LoadExceeded { route: usize, step: usize },
    #[error("route {route}: store {store} served outside its receiving windows")]
    WindowMissed { route: usize, store: StoreId },
    #[error("route {route}: step {step} is not covered by the assigned drivers")]
    ShiftNotCovered { route: usize, step: usize },
    #[error("store {store} served {count} times")]
    DuplicateStore { store: StoreId, count: u32 },
    #[error("store {store} is neither served nor listed as unassigned")]
    MissingStore { store: StoreId },
    #[error("store {store} is both served and listed as unassigned")]
    ServedAndUnassigned { store: StoreId },
}
