//! Rule violations reported by [`crate::Solution::validate`].

use thiserror::Error;

use crate::ids::{DepotId, DriverId, StoreId, VehicleId};
use crate::time::Minutes;

/// A rule broken by a [`crate::Solution`].
#[derive(Debug, Error, PartialEq)]
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
    #[error("driver {driver} is scheduled on two routes with overlapping work spans")]
    DriverDoubleBooked { driver: DriverId },
    #[error(
        "driver {driver} worked {work_hours} hours in a 24-hour period, exceeding the {limit_hours}-hour limit"
    )]
    DriverWorkHoursExceeded {
        driver: DriverId,
        work_hours: f64,
        limit_hours: f64,
    },
    #[error(
        "driver {driver} had only {rest_hours} hours rest between shifts, below the {min_rest_hours}-hour minimum"
    )]
    DriverInsufficientRest {
        driver: DriverId,
        rest_hours: f64,
        min_rest_hours: f64,
    },
    #[error(
        "driver {driver} worked {work_hours} hours in a 7-day period, exceeding the {limit_hours}-hour limit"
    )]
    DriverWeeklyHoursExceeded {
        driver: DriverId,
        work_hours: f64,
        limit_hours: f64,
    },
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
