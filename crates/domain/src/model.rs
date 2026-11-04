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
    /// Uses saturating arithmetic to avoid overflow (returns u64 for headroom).
    pub fn total(self) -> u64 {
        (self.ambient as u64)
            .saturating_add(self.chilled as u64)
            .saturating_add(self.frozen as u64)
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

/// True when every minute of `[start, end)` is covered by at least one shift.
///
/// Used for depot dwells (loading, driver hand-overs) that may legally span
/// two shifts. Shifts are half-open `[start, end)`. Returns `false` for an
/// empty shift list.
fn union_covers(shifts: &[Shift], start: Minutes, end: Minutes) -> bool {
    if shifts.is_empty() {
        return false;
    }
    // Build merged intervals from shifts (half-open [start, end))
    let mut intervals: Vec<(i64, i64)> = shifts
        .iter()
        .map(|s| (s.start.minutes(), s.end.minutes()))
        .collect();
    intervals.sort_unstable_by_key(|&(s, _)| s);
    // Merge overlapping/adjacent intervals
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (s, e) in intervals {
        if let Some(last) = merged.last_mut()
            && s <= last.1
        {
            // Overlaps or touches — merge
            last.1 = last.1.max(e);
            continue;
        }
        merged.push((s, e));
    }
    // Check if [start, end) is fully covered by merged intervals
    let (qs, qe) = (start.minutes(), end.minutes());
    for (s, e) in merged {
        if s <= qs && qe <= e {
            return true;
        }
    }
    false
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

/// Fatigue management rules for heavy vehicle drivers (NHVR-standard).
///
/// Standard Hours are the default for Australian heavy vehicle drivers:
/// - Max 12 hours work in any 24-hour period
/// - Min 7 hours continuous rest between shifts
/// - Max 144 hours work in any 7-day period
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FatigueRules {
    /// NHVR Standard Hours (default for most heavy vehicle drivers).
    #[default]
    Standard,
    /// NHVR Basic Fatigue Rules (requires accreditation).
    /// Max 14 hours work per shift, min 6 hours rest, max 144 hours per 7 days.
    Basic,
    /// NHVR Advanced Fatigue Rules (requires accreditation and additional controls).
    /// Max 14 hours work per shift, min 5 hours rest, max 144 hours per 7 days.
    Advanced,
}

impl FatigueRules {
    /// Maximum work minutes in any 24-hour period.
    pub fn max_work_24h_minutes(self) -> i64 {
        match self {
            Self::Standard => 12 * 60,
            Self::Basic | Self::Advanced => 14 * 60,
        }
    }

    /// Minimum continuous rest minutes between shifts.
    pub fn min_rest_minutes(self) -> i64 {
        match self {
            Self::Standard => 7 * 60,
            Self::Basic => 6 * 60,
            Self::Advanced => 5 * 60,
        }
    }

    /// Maximum work minutes in any 7-day period.
    pub fn max_work_7d_minutes(self) -> i64 {
        // All rule sets cap at 144 hours per 7 days.
        144 * 60
    }
}

/// A driver with one or more working shifts on the horizon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Driver {
    pub id: DriverId,
    pub name: String,
    pub shifts: Vec<Shift>,
    /// Fatigue management rules for this driver. Defaults to Standard Hours.
    #[serde(default)]
    pub fatigue_rules: FatigueRules,
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
    /// 7. No driver is scheduled on two routes with overlapping work spans.
    ///    A driver listed on a route is booked for the route's whole span
    ///    (first departure to last arrival); spans may touch end-to-start but
    ///    must not overlap. Relay-precise attribution is M5 work.
    /// 8. Every store is served exactly once or listed as unassigned.
    ///
    /// Aggregate totals (`total_*`) are informational in M0 and are not
    /// re-derived here.
    pub fn validate(&self, problem: &Problem) -> Result<(), Vec<Violation>> {
        let mut violations = Vec::new();
        let mut served: Vec<u32> = vec![0; problem.stores.len()];
        let mut vehicle_used: Vec<bool> = vec![false; problem.vehicles.len()];
        let mut driver_spans: Vec<Vec<(Minutes, Minutes)>> =
            vec![Vec::new(); problem.drivers.len()];

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

            // Book every listed driver for this route's span so overlaps are
            // caught after all routes have been checked.
            let mut listed: Vec<DriverId> = Vec::new();
            for &driver in &route.drivers {
                if problem.driver(driver).is_some() && !listed.contains(&driver) {
                    listed.push(driver);
                    driver_spans[driver.0 as usize].push((first.depart, last.depart));
                }
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

        for (index, spans) in driver_spans.iter_mut().enumerate() {
            spans.sort_unstable();
            let overlapping = spans
                .windows(2)
                .any(|pair| pair[1].0 < pair[0].1 && pair[0].0 < pair[1].1);
            if overlapping {
                violations.push(Violation::DriverDoubleBooked {
                    driver: DriverId(index as u32),
                });
            }

            // NHVR fatigue rule checks.
            let driver_id = DriverId(index as u32);
            let Some(driver) = problem.driver(driver_id) else {
                continue;
            };
            let rules = driver.fatigue_rules;
            let max_work_24h = rules.max_work_24h_minutes();
            let min_rest = rules.min_rest_minutes();
            let max_work_7d = rules.max_work_7d_minutes();

            // Check minimum rest between consecutive shifts.
            for pair in spans.windows(2) {
                let rest = pair[1].0.minutes() - pair[0].1.minutes();
                if rest < min_rest {
                    violations.push(Violation::DriverInsufficientRest {
                        driver: driver_id,
                        rest_hours: rest as f64 / 60.0,
                        min_rest_hours: min_rest as f64 / 60.0,
                    });
                }
            }

            // Check rolling 24-hour work limit.
            let day_minutes = 24 * 60;
            for i in 0..spans.len() {
                let window_start = spans[i].0;
                let window_end = window_start + Minutes::from_minutes(day_minutes);
                let work_in_window: i64 = spans
                    .iter()
                    .filter(|(s, e)| *s >= window_start && *s < window_end)
                    .map(|(s, e)| e.minutes() - s.minutes())
                    .sum();
                if work_in_window > max_work_24h {
                    violations.push(Violation::DriverWorkHoursExceeded {
                        driver: driver_id,
                        work_hours: work_in_window as f64 / 60.0,
                        limit_hours: max_work_24h as f64 / 60.0,
                    });
                    break; // Report once per driver per rule
                }
            }

            // Check rolling 7-day work limit.
            let week_minutes = 7 * day_minutes;
            for i in 0..spans.len() {
                let window_start = spans[i].0;
                let window_end = window_start + Minutes::from_minutes(week_minutes);
                let work_in_window: i64 = spans
                    .iter()
                    .filter(|(s, e)| *s >= window_start && *s < window_end)
                    .map(|(s, e)| e.minutes() - s.minutes())
                    .sum();
                if work_in_window > max_work_7d {
                    violations.push(Violation::DriverWeeklyHoursExceeded {
                        driver: driver_id,
                        work_hours: work_in_window as f64 / 60.0,
                        limit_hours: max_work_7d as f64 / 60.0,
                    });
                    break;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatigue_rules_standard_limits() {
        let rules = FatigueRules::Standard;
        assert_eq!(rules.max_work_24h_minutes(), 12 * 60);
        assert_eq!(rules.min_rest_minutes(), 7 * 60);
        assert_eq!(rules.max_work_7d_minutes(), 144 * 60);
    }

    #[test]
    fn fatigue_rules_basic_limits() {
        let rules = FatigueRules::Basic;
        assert_eq!(rules.max_work_24h_minutes(), 14 * 60);
        assert_eq!(rules.min_rest_minutes(), 6 * 60);
        assert_eq!(rules.max_work_7d_minutes(), 144 * 60);
    }

    #[test]
    fn fatigue_rules_advanced_limits() {
        let rules = FatigueRules::Advanced;
        assert_eq!(rules.max_work_24h_minutes(), 14 * 60);
        assert_eq!(rules.min_rest_minutes(), 5 * 60);
        assert_eq!(rules.max_work_7d_minutes(), 144 * 60);
    }

    #[test]
    fn fatigue_rules_default_is_standard() {
        let rules = FatigueRules::default();
        assert_eq!(rules, FatigueRules::Standard);
    }

    /// Builds a minimal problem with one depot, one store, one vehicle, one driver.
    fn fatigue_test_problem(fatigue_rules: FatigueRules) -> Problem {
        Problem {
            name: "fatigue test".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Depot".into(),
                location: LatLon::new(-33.87, 151.21),
            }],
            stores: vec![Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "Store".into(),
                location: LatLon::new(-33.92, 151.23),
                demand: Load::ZERO,
                service: Minutes::ZERO,
                receiving: vec![TimeWindow::new(
                    Minutes::ZERO,
                    Minutes::from_minutes(10_000),
                )],
            }],
            vehicles: vec![
                Vehicle {
                    id: VehicleId(0),
                    name: "Truck 1".into(),
                    depot: DepotId(0),
                    capacity: Load::new(8, 8, 4),
                    max_duration: Minutes::from_minutes(10_000),
                    avg_speed_kmh: 60.0,
                },
                Vehicle {
                    id: VehicleId(1),
                    name: "Truck 2".into(),
                    depot: DepotId(0),
                    capacity: Load::new(8, 8, 4),
                    max_duration: Minutes::from_minutes(10_000),
                    avg_speed_kmh: 60.0,
                },
            ],
            drivers: vec![Driver {
                id: DriverId(0),
                name: "Driver".into(),
                shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(10_000))],
                fatigue_rules,
            }],
            cost: CostModel {
                fixed_per_route_cents: 0,
                per_km_cents: 0,
                per_hour_cents: 0,
                late_per_min_cents: 0,
            },
        }
    }

    #[test]
    fn fatigue_normal_shifts_pass_validation() {
        let problem = fatigue_test_problem(FatigueRules::Standard);
        // Two 5-hour shifts with 7 hours rest between them (exactly at minimum)
        // Total work in 24h: 10 hours < 12 hour limit
        let solution = Solution {
            routes: vec![
                RoutePlan {
                    vehicle: VehicleId(0),
                    drivers: vec![DriverId(0)],
                    steps: vec![
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::ZERO,
                            depart: Minutes::ZERO,
                        },
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(300),
                            depart: Minutes::from_minutes(300),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(300),
                    cost_cents: 0,
                },
                RoutePlan {
                    vehicle: VehicleId(1),
                    drivers: vec![DriverId(0)],
                    steps: vec![
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(720),
                            depart: Minutes::from_minutes(720),
                        },
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(1020),
                            depart: Minutes::from_minutes(1020),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(300),
                    cost_cents: 0,
                },
            ],
            unassigned: vec![StoreId(0)],
            total_distance_km: 0.0,
            total_duration: Minutes::from_minutes(600),
            total_cost_cents: 0,
        };
        assert!(solution.validate(&problem).is_ok());
    }

    #[test]
    fn fatigue_insufficient_rest_between_shifts() {
        let problem = fatigue_test_problem(FatigueRules::Standard);
        // Shift 1 ends at 480, shift 2 starts at 800 → only 320 min rest (< 420 min = 7h)
        let solution = Solution {
            routes: vec![
                RoutePlan {
                    vehicle: VehicleId(0),
                    drivers: vec![DriverId(0)],
                    steps: vec![
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::ZERO,
                            depart: Minutes::ZERO,
                        },
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(480),
                            depart: Minutes::from_minutes(480),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(480),
                    cost_cents: 0,
                },
                RoutePlan {
                    vehicle: VehicleId(0),
                    drivers: vec![DriverId(0)],
                    steps: vec![
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(800),
                            depart: Minutes::from_minutes(800),
                        },
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(1000),
                            depart: Minutes::from_minutes(1000),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(200),
                    cost_cents: 0,
                },
            ],
            unassigned: vec![StoreId(0)],
            total_distance_km: 0.0,
            total_duration: Minutes::from_minutes(680),
            total_cost_cents: 0,
        };
        let violations = solution.validate(&problem).unwrap_err();
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, Violation::DriverInsufficientRest { .. })),
            "expected DriverInsufficientRest violation, got {:?}",
            violations
        );
    }

    #[test]
    fn fatigue_work_hours_exceeded_in_24h() {
        let problem = fatigue_test_problem(FatigueRules::Standard);
        // Two shifts within 24h totaling 13 hours (780 min > 720 min = 12h limit)
        let solution = Solution {
            routes: vec![
                RoutePlan {
                    vehicle: VehicleId(0),
                    drivers: vec![DriverId(0)],
                    steps: vec![
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::ZERO,
                            depart: Minutes::ZERO,
                        },
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(480),
                            depart: Minutes::from_minutes(480),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(480),
                    cost_cents: 0,
                },
                RoutePlan {
                    vehicle: VehicleId(1),
                    drivers: vec![DriverId(0)],
                    steps: vec![
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(550),
                            depart: Minutes::from_minutes(550),
                        },
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(850),
                            depart: Minutes::from_minutes(850),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(300),
                    cost_cents: 0,
                },
            ],
            unassigned: vec![StoreId(0)],
            total_distance_km: 0.0,
            total_duration: Minutes::from_minutes(780),
            total_cost_cents: 0,
        };
        let violations = solution.validate(&problem).unwrap_err();
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, Violation::DriverWorkHoursExceeded { .. })),
            "expected DriverWorkHoursExceeded violation, got {:?}",
            violations
        );
    }

    #[test]
    fn fatigue_basic_rules_allow_more_work() {
        // Same shift pattern as work_hours_exceeded_in_24h but with Basic rules
        // (14h = 840 min limit vs Standard 12h = 720 min)
        let problem = fatigue_test_problem(FatigueRules::Basic);
        let solution = Solution {
            routes: vec![
                RoutePlan {
                    vehicle: VehicleId(0),
                    drivers: vec![DriverId(0)],
                    steps: vec![
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::ZERO,
                            depart: Minutes::ZERO,
                        },
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(480),
                            depart: Minutes::from_minutes(480),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(480),
                    cost_cents: 0,
                },
                RoutePlan {
                    vehicle: VehicleId(1),
                    drivers: vec![DriverId(0)],
                    steps: vec![
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(850),
                            depart: Minutes::from_minutes(850),
                        },
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(1150),
                            depart: Minutes::from_minutes(1150),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(300),
                    cost_cents: 0,
                },
            ],
            unassigned: vec![StoreId(0)],
            total_distance_km: 0.0,
            total_duration: Minutes::from_minutes(780),
            total_cost_cents: 0,
        };
        // 780 min < 840 min Basic limit → should pass
        let result = solution.validate(&problem);
        if let Err(ref violations) = result {
            eprintln!("Violations: {:?}", violations);
        }
        assert!(result.is_ok());
    }

    #[test]
    fn fatigue_driver_with_no_routes_passes() {
        let problem = fatigue_test_problem(FatigueRules::Standard);
        let solution = Solution {
            routes: vec![],
            unassigned: vec![StoreId(0)],
            total_distance_km: 0.0,
            total_duration: Minutes::ZERO,
            total_cost_cents: 0,
        };
        assert!(solution.validate(&problem).is_ok());
    }
}
