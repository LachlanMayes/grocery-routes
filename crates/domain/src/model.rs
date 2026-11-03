//! The problem and solution model, plus the validation rules that define a
//! well-formed plan.

use std::fmt;
use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize};

use crate::error::Violation;
use crate::geo::LatLon;
use crate::ids::{DepotId, DriverId, StoreId, VehicleId};
use crate::time::Minutes;

/// One of the four grocery banners the system models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    Coles,
    Woolworths,
    Aldi,
    Iga,
}

impl fmt::Display for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Coles => "Coles",
            Self::Woolworths => "Woolworths",
            Self::Aldi => "ALDI",
            Self::Iga => "IGA",
        })
    }
}

/// Handling-unit counts per temperature compartment (e.g. roll cages).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Load {
    pub ambient: u32,
    pub chilled: u32,
    pub frozen: u32,
}

impl Load {
    /// An empty load.
    pub const ZERO: Self = Self {
        ambient: 0,
        chilled: 0,
        frozen: 0,
    };

    /// Builds a load from per-compartment counts.
    pub const fn new(ambient: u32, chilled: u32, frozen: u32) -> Self {
        Self {
            ambient,
            chilled,
            frozen,
        }
    }

    /// Total handling units across all compartments.
    pub const fn total(self) -> u32 {
        self.ambient + self.chilled + self.frozen
    }

    /// True when every compartment fits inside `capacity`.
    pub const fn fits_within(self, capacity: Self) -> bool {
        self.ambient <= capacity.ambient
            && self.chilled <= capacity.chilled
            && self.frozen <= capacity.frozen
    }
}

impl Add for Load {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self {
            ambient: self.ambient + rhs.ambient,
            chilled: self.chilled + rhs.chilled,
            frozen: self.frozen + rhs.frozen,
        }
    }
}

impl AddAssign for Load {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

/// A receiving window on the horizon, half-open `[start, end)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeWindow {
    pub start: Minutes,
    pub end: Minutes,
}

impl TimeWindow {
    /// Builds a window from its bounds.
    pub const fn new(start: Minutes, end: Minutes) -> Self {
        Self { start, end }
    }

    /// True when `point` lies in `[start, end)`.
    pub fn contains(&self, point: Minutes) -> bool {
        self.start <= point && point < self.end
    }

    /// The length of the window.
    pub fn duration(&self) -> Minutes {
        self.end - self.start
    }
}

/// A driver working interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shift {
    pub start: Minutes,
    pub end: Minutes,
}

impl Shift {
    /// Builds a shift from its bounds.
    pub const fn new(start: Minutes, end: Minutes) -> Self {
        Self { start, end }
    }

    /// True when every minute of `[start, end]` lies inside this shift.
    pub fn covers(&self, start: Minutes, end: Minutes) -> bool {
        self.start <= start && end <= self.end
    }
}

/// True when every minute of `[start, end]` is covered by at least one shift.
///
/// Used for depot dwells (loading, driver hand-overs) that may legally span
/// two shifts. Returns `false` for an empty shift list.
fn union_covers(shifts: &[Shift], start: Minutes, end: Minutes) -> bool {
    !shifts.is_empty()
        && (start.minutes()..=end.minutes()).all(|minute| {
            shifts
                .iter()
                .any(|shift| shift.start.minutes() <= minute && minute <= shift.end.minutes())
        })
}

/// A distribution centre.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Depot {
    pub id: DepotId,
    pub name: String,
    pub location: LatLon,
}

/// A supermarket to deliver to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub id: StoreId,
    pub chain: Chain,
    pub name: String,
    pub location: LatLon,
    /// Demand of this store's single delivery in the M0 model.
    pub demand: Load,
    /// Time spent on site (check-in, unloading).
    pub service: Minutes,
    /// Receiving windows; service must sit inside one of them.
    pub receiving: Vec<TimeWindow>,
}

/// A truck, with one capacity value per temperature compartment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vehicle {
    pub id: VehicleId,
    pub name: String,
    pub depot: DepotId,
    pub capacity: Load,
    pub max_duration: Minutes,
    /// Planning placeholder for average speed; real road durations replace
    /// this in M3 of the roadmap (see `docs/PLAN.md`).
    pub avg_speed_kmh: f64,
}

/// A driver with one or more working shifts on the horizon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Driver {
    pub id: DriverId,
    pub name: String,
    pub shifts: Vec<Shift>,
}

/// Global cost policy, in Australian cents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostModel {
    /// Per-route fixed cost (vehicle standing charge, depot overhead).
    pub fixed_per_route_cents: i64,
    pub per_km_cents: i64,
    pub per_hour_cents: i64,
    /// Lateness penalty for soft windows; reserved for M2 — M0 windows are
    /// hard.
    pub late_per_min_cents: i64,
}

/// One node on a route: a depot visit or a store delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Node {
    Depot(DepotId),
    Store(StoreId),
}

/// A stop with planned times.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedStop {
    pub node: Node,
    pub arrive: Minutes,
    pub depart: Minutes,
}

/// One planned vehicle trip: a time-ordered sequence of stops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutePlan {
    pub vehicle: VehicleId,
    /// Drivers working this route, in any order; coverage rules live in
    /// [`Solution::validate`].
    pub drivers: Vec<DriverId>,
    pub steps: Vec<PlannedStop>,
    pub distance_km: f64,
    pub duration: Minutes,
    pub cost_cents: i64,
}

/// A complete (or partial) plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Solution {
    pub routes: Vec<RoutePlan>,
    /// Stores the planner could not place within every constraint.
    pub unassigned: Vec<StoreId>,
    pub total_distance_km: f64,
    pub total_duration: Minutes,
    pub total_cost_cents: i64,
}

impl Solution {
    /// Checks the plan against the structural and safety rules below.
    /// Returns every violation found, or `Ok(())` when the plan is valid.
    ///
    /// 1. Every route starts at its vehicle's home depot and ends at a depot.
    /// 2. Timestamps are non-decreasing; `duration` matches the spanned time;
    ///    `duration` respects the vehicle limit; a vehicle is used once.
    /// 3. Cumulative load never exceeds the vehicle capacity.
    /// 4. Store service sits inside one of the store's receiving windows
    ///    (arrival is treated as the start of service; waiting time is folded
    ///    into the preceding leg).
    /// 5. Every driving leg sits inside a single shift of one of the route's
    ///    drivers.
    /// 6. Store service sits inside a single shift; depot dwells may span
    ///    shifts (driver hand-overs at relay depots).
    /// 7. Every store is served exactly once or listed as unassigned.
    ///
    /// Aggregate totals (`total_*`) are informational in M0 and are not
    /// re-derived here.
    pub fn validate(&self, problem: &Problem) -> Result<(), Vec<Violation>> {
        let mut violations = Vec::new();
        let mut served: Vec<u32> = vec![0; problem.stores.len()];
        let mut vehicle_used: Vec<bool> = vec![false; problem.vehicles.len()];

        for (route_index, route) in self.routes.iter().enumerate() {
            let Some(vehicle) = problem.vehicle(route.vehicle) else {
                violations.push(Violation::UnknownVehicle {
                    route: route_index,
                    vehicle: route.vehicle,
                });
                continue;
            };
            if vehicle_used[route.vehicle.0 as usize] {
                violations.push(Violation::VehicleUsedTwice {
                    vehicle: route.vehicle,
                });
            } else {
                vehicle_used[route.vehicle.0 as usize] = true;
            }

            for &driver in &route.drivers {
                if problem.driver(driver).is_none() {
                    violations.push(Violation::UnknownDriver { driver });
                }
            }
            let has_drivers = !route.drivers.is_empty();
            if !has_drivers {
                violations.push(Violation::NoDriverAssigned { route: route_index });
            }
            let shifts: Vec<Shift> = route
                .drivers
                .iter()
                .filter_map(|&driver| problem.driver(driver))
                .flat_map(|driver| driver.shifts.iter().copied())
                .collect();

            let (Some(first), Some(last)) = (route.steps.first(), route.steps.last()) else {
                violations.push(Violation::EmptyRoute { route: route_index });
                continue;
            };

            if first.node != Node::Depot(vehicle.depot) {
                violations.push(Violation::RouteNotStartingAtHomeDepot { route: route_index });
            }
            if !matches!(last.node, Node::Depot(_)) {
                violations.push(Violation::RouteNotEndingAtDepot { route: route_index });
            }
            if route.duration != last.depart - first.depart {
                violations.push(Violation::DurationMismatch { route: route_index });
            }
            if route.duration > vehicle.max_duration {
                violations.push(Violation::MaxDurationExceeded {
                    route: route_index,
                    duration: route.duration,
                    limit: vehicle.max_duration,
                });
            }

            let mut previous_depart: Option<Minutes> = None;
            for (step_index, step) in route.steps.iter().enumerate() {
                let ordered = step.depart >= step.arrive
                    && previous_depart.is_none_or(|previous| step.arrive >= previous);
                if !ordered {
                    violations.push(Violation::TimestampsOutOfOrder {
                        route: route_index,
                        step: step_index,
                    });
                }
                previous_depart = Some(step.depart);
            }

            let mut load = Load::ZERO;
            let mut load_reported = false;
            for (step_index, step) in route.steps.iter().enumerate() {
                match step.node {
                    Node::Store(store_id) => {
                        let Some(store) = problem.store(store_id) else {
                            violations.push(Violation::UnknownStore { store: store_id });
                            continue;
                        };
                        load += store.demand;
                        if !load_reported && !load.fits_within(vehicle.capacity) {
                            violations.push(Violation::LoadExceeded {
                                route: route_index,
                                step: step_index,
                            });
                            load_reported = true;
                        }
                        served[store_id.0 as usize] += 1;
                        let window_ok = store
                            .receiving
                            .iter()
                            .any(|window| window.start <= step.arrive && step.depart <= window.end);
                        if !window_ok {
                            violations.push(Violation::WindowMissed {
                                route: route_index,
                                store: store_id,
                            });
                        }
                        if has_drivers
                            && !shifts
                                .iter()
                                .any(|shift| shift.covers(step.arrive, step.depart))
                        {
                            violations.push(Violation::ShiftNotCovered {
                                route: route_index,
                                step: step_index,
                            });
                        }
                    }
                    Node::Depot(depot_id) => {
                        if problem.depot(depot_id).is_none() {
                            violations.push(Violation::UnknownDepot { depot: depot_id });
                        }
                        if has_drivers
                            && step.depart > step.arrive
                            && !union_covers(&shifts, step.arrive, step.depart)
                        {
                            violations.push(Violation::ShiftNotCovered {
                                route: route_index,
                                step: step_index,
                            });
                        }
                    }
                }
            }

            for (leg_index, pair) in route.steps.windows(2).enumerate() {
                let (from, to) = (pair[0], pair[1]);
                if has_drivers
                    && !shifts
                        .iter()
                        .any(|shift| shift.covers(from.depart, to.arrive))
                {
                    violations.push(Violation::ShiftNotCovered {
                        route: route_index,
                        step: leg_index,
                    });
                }
            }
        }

        for &store in &self.unassigned {
            if problem.store(store).is_none() {
                violations.push(Violation::UnknownStore { store });
                continue;
            }
            if served[store.0 as usize] > 0 {
                violations.push(Violation::ServedAndUnassigned { store });
            }
        }
        for (index, &count) in served.iter().enumerate() {
            let store = StoreId(index as u32);
            if count > 1 {
                violations.push(Violation::DuplicateStore { store, count });
            } else if count == 0 && !self.unassigned.contains(&store) {
                violations.push(Violation::MissingStore { store });
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

/// The problem instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub name: String,
    pub depots: Vec<Depot>,
    pub stores: Vec<Store>,
    pub vehicles: Vec<Vehicle>,
    pub drivers: Vec<Driver>,
    pub cost: CostModel,
}

impl Problem {
    /// Looks up a depot by id (bounds-checked).
    pub fn depot(&self, id: DepotId) -> Option<&Depot> {
        self.depots.get(id.0 as usize)
    }

    /// Looks up a store by id (bounds-checked).
    pub fn store(&self, id: StoreId) -> Option<&Store> {
        self.stores.get(id.0 as usize)
    }

    /// Looks up a vehicle by id (bounds-checked).
    pub fn vehicle(&self, id: VehicleId) -> Option<&Vehicle> {
        self.vehicles.get(id.0 as usize)
    }

    /// Looks up a driver by id (bounds-checked).
    pub fn driver(&self, id: DriverId) -> Option<&Driver> {
        self.drivers.get(id.0 as usize)
    }

    /// The coordinate of a route node, when it exists.
    pub fn node_location(&self, node: Node) -> Option<LatLon> {
        match node {
            Node::Depot(id) => self.depot(id).map(|depot| depot.location),
            Node::Store(id) => self.store(id).map(|store| store.location),
        }
    }
}
