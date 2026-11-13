//! The problem and solution model, plus the validation rules that define a
//! well-formed plan.

use std::fmt;
use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize};

use crate::error::Violation;
use crate::geo::LatLon;
use crate::ids::{DepotId, DriverId, StoreId, VehicleId};
use crate::time::Minutes;

/// Delivery stop category or grocery banner. `General` represents arbitrary
/// stops without chain-specific defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    #[default]
    #[serde(alias = "custom")]
    General,
    Coles,
    Woolworths,
    Aldi,
    Iga,
}

impl fmt::Display for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::General => "General",
            Self::Coles => "Coles",
            Self::Woolworths => "Woolworths",
            Self::Aldi => "ALDI",
            Self::Iga => "IGA",
        })
    }
}

/// Chain-specific receiving rules used when a store does not specify its own
/// receiving windows. Each Australian grocery chain has typical receiving
/// patterns (e.g. Coles and Woolworths often receive overnight, ALDI often
/// receives during trading hours).
///
/// Stores can override these defaults with their own `receiving` windows; if a
/// store has no explicit windows, the chain defaults are used.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainRules {
    /// Default receiving windows for Coles stores (minutes from horizon start).
    pub coles: Vec<TimeWindow>,
    /// Default receiving windows for Woolworths stores.
    pub woolworths: Vec<TimeWindow>,
    /// Default receiving windows for ALDI stores.
    pub aldi: Vec<TimeWindow>,
    /// Default receiving windows for IGA stores.
    pub iga: Vec<TimeWindow>,
    /// Empties each banner's stores hand back per visit, used by
    /// [`crate::ledger`]. Illustrative fixture data; defaults to "collect
    /// nothing".
    #[serde(default)]
    pub returnables: ReturnablesDefaults,
}

impl ChainRules {
    /// Returns the default receiving windows for `chain`, or `None` if no
    /// defaults are configured for that chain.
    pub fn windows_for(&self, chain: Chain) -> Option<&Vec<TimeWindow>> {
        match chain {
            Chain::General => None,
            Chain::Coles => Some(&self.coles),
            Chain::Woolworths => Some(&self.woolworths),
            Chain::Aldi => Some(&self.aldi),
            Chain::Iga => Some(&self.iga),
        }
    }

    /// Returns the effective receiving windows for `store`: the store's own
    /// windows if non-empty, otherwise the chain defaults.
    pub fn windows_for_store<'a>(&'a self, store: &'a Store) -> Option<&'a Vec<TimeWindow>> {
        if !store.receiving.is_empty() {
            return Some(&store.receiving);
        }
        self.windows_for(store.chain)
    }

    /// Empties (roll cages) a store of `chain` hands back per visit, for the
    /// returnables ledger. Illustrative fixture data; zeros mean "collect
    /// nothing".
    pub fn returnables_for(&self, chain: Chain) -> Load {
        self.returnables.for_chain(chain)
    }
}

/// Empties (roll cages) each banner's stores hand back on a visit, used by
/// [`crate::ledger`] to balance issued cages against collections.
///
/// These are illustrative fixture defaults, not researched operator data;
/// `Load::ZERO` (the default) means the banner collects nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnablesDefaults {
    pub coles: Load,
    pub woolworths: Load,
    pub aldi: Load,
    pub iga: Load,
}

impl ReturnablesDefaults {
    /// The collection fixture for `chain`.
    pub fn for_chain(&self, chain: Chain) -> Load {
        match chain {
            Chain::General => Load::ZERO,
            Chain::Coles => self.coles,
            Chain::Woolworths => self.woolworths,
            Chain::Aldi => self.aldi,
            Chain::Iga => self.iga,
        }
    }
}

/// Curfew rules restrict when trucks may traverse certain road segments.
/// Each entry forbids travel during a time window on a specific road class.
///
/// `start`/`end` are **minutes of the local day** (midnight = 0) that the
/// window applies, recurring on every weekday selected by `days_of_week`.
/// A rule with `start > end` wraps through midnight; `start == end` is an
/// empty window that never applies. `days_of_week` selects the local day the
/// window *starts* on, so a `Friday 22:00 → 06:00` rule covers Saturday
/// morning when only Friday is selected.
///
/// Enforcement requires an absolute anchor ([`Problem::horizon_start`]) to
/// resolve each traversal instant to a local day and time; without an anchor
/// the fields are inert import/export data (the same rule applies to
/// [`CurfewRules`] as a whole).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurfewRule {
    /// Road class this curfew applies to (an OSM `highway` value, e.g.
    /// `"residential"`, `"primary"`).
    pub road_class: String,
    /// Minute of the local day the curfew starts (0 = midnight).
    pub start: Minutes,
    /// Minute of the local day the curfew ends (exclusive).
    pub end: Minutes,
    /// Days of week the window starts on (0=Sunday, 6=Saturday).
    /// `None` = every day.
    pub days_of_week: Option<[bool; 7]>,
}

impl CurfewRule {
    /// True when this rule curtails `road_class` at `minute_of_day` on local
    /// weekday `day` (0 = Sunday).
    #[must_use]
    pub fn applies_at(&self, road_class: &str, minute_of_day: i64, day: usize) -> bool {
        if self.road_class != road_class {
            return false;
        }
        let day = day % 7;
        let selected = |index: usize| self.days_of_week.is_none_or(|days| days[index % 7]);
        let minute = minute_of_day.rem_euclid(1440);
        if self.start.minutes() < self.end.minutes() {
            // Same-day window.
            selected(day) && self.start.minutes() <= minute && minute < self.end.minutes()
        } else if self.start.minutes() > self.end.minutes() {
            // Wrap-midnight window: the evening part belongs to the start
            // day, the morning part to the day after it.
            (selected(day) && minute >= self.start.minutes())
                || (selected((day + 6) % 7) && minute < self.end.minutes())
        } else {
            false
        }
    }

    /// Minutes from `minute_of_day` until this rule next releases, assuming
    /// it currently applies.
    fn minutes_until_release(&self, minute_of_day: i64) -> i64 {
        let minute = minute_of_day.rem_euclid(1440);
        if self.start.minutes() < self.end.minutes() {
            self.end.minutes() - minute
        } else if minute >= self.start.minutes() {
            // Evening part: release at `end` on the next day.
            (1440 - minute) + self.end.minutes()
        } else {
            // Morning part: release at `end` today.
            self.end.minutes() - minute
        }
    }
}

/// Curfew rules for the road network. Enforcement resolves each traversal
/// instant to a local weekday/time via [`crate::clock`] and evaluates
/// [`CurfewRule::applies_at`]; see `routing::timed` for the time-aware router
/// that consumes these rules.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurfewRules {
    pub rules: Vec<CurfewRule>,
}

impl CurfewRules {
    /// True when `road_class` is under curfew at `time` on `day_of_week`
    /// (0=Sunday). Without an anchor, `time` is taken as minutes from the
    /// horizon start of a single-day horizon.
    #[must_use]
    pub fn is_curtailed(&self, road_class: &str, time: Minutes, day_of_week: usize) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.applies_at(road_class, time.minutes(), day_of_week))
    }

    /// Minutes to wait before `road_class` may be traversed again, given the
    /// current local `minute_of_day` and weekday. Returns `0` when traversal
    /// is already allowed; otherwise the wait lands on the next window
    /// boundary, which the caller must re-check (and re-localise) because
    /// another rule may still apply.
    #[must_use]
    pub fn next_allowed_delta(&self, road_class: &str, minute_of_day: i64, day: usize) -> i64 {
        self.rules
            .iter()
            .filter(|rule| rule.applies_at(road_class, minute_of_day, day))
            .map(|rule| rule.minutes_until_release(minute_of_day))
            .min()
            .unwrap_or(0)
            .max(0)
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

    /// Adds `rhs` compartment-wise, saturating at [`u32::MAX`].
    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self {
            ambient: self.ambient.saturating_add(rhs.ambient),
            chilled: self.chilled.saturating_add(rhs.chilled),
            frozen: self.frozen.saturating_add(rhs.frozen),
        }
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
/// Maximum work (minutes) that intersects any window of `window_minutes`.
///
/// For each candidate window, the work is the sum of overlaps with every
/// span, not the full duration of spans that happen to start inside it. The
/// best window can always be moved until it starts at a span start or a
/// span end minus the window length, so only those anchors are checked.
///
/// Public so callers that re-schedule routes (e.g. curfew enforcement) can
/// apply the same fatigue arithmetic [`Solution::validate`] uses.
#[must_use]
pub fn max_work_in_fixed_window(spans: &[(Minutes, Minutes)], window_minutes: i64) -> i64 {
    if spans.is_empty() {
        return 0;
    }
    let anchors: Vec<i64> = spans
        .iter()
        .flat_map(|&(start, end)| {
            [
                start.minutes(),
                start.minutes() - window_minutes,
                end.minutes(),
                end.minutes() - window_minutes,
            ]
        })
        .collect();
    anchors
        .iter()
        .map(|&anchor| {
            let end = anchor + window_minutes;
            spans
                .iter()
                .map(|&(start, finish)| {
                    let lo = start.minutes().max(anchor);
                    let hi = finish.minutes().min(end);
                    hi.saturating_sub(lo).max(0)
                })
                .sum()
        })
        .max()
        .unwrap_or(0)
}

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

/// A distribution centre or route start/end depot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Depot {
    pub id: DepotId,
    pub name: String,
    pub location: LatLon,
    /// Optional street address or place description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// IANA timezone name (e.g. "Australia/Sydney"). Stored for API
    /// import/export and future local-time planning; it is not applied by the
    /// solver or validator yet.
    #[serde(default)]
    pub timezone: Option<String>,
}

/// A stop / store / place to visit and deliver to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub id: StoreId,
    pub chain: Chain,
    pub name: String,
    pub location: LatLon,
    /// Optional street address or place description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Demand of this store's single delivery in the M0 model.
    pub demand: Load,
    /// Time spent on site (check-in, unloading).
    pub service: Minutes,
    /// Receiving windows; service must sit inside one of them.
    pub receiving: Vec<TimeWindow>,
    /// IANA timezone name (e.g. "Australia/Sydney"). Stored for API
    /// import/export and future local-time planning; it is not applied by the
    /// solver or validator yet. Omitting it means all times are horizon
    /// minutes.
    #[serde(default)]
    pub timezone: Option<String>,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
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

            // Book drivers for this route's span so overlaps and NHVR fatigue
            // rules are checked after all routes have been collected.
            let mut listed: Vec<DriverId> = Vec::new();
            for &driver in &route.drivers {
                if problem.driver(driver).is_some() && !listed.contains(&driver) {
                    listed.push(driver);
                }
            }
            if listed.len() == 1 {
                driver_spans[listed[0].0 as usize].push((first.depart, last.depart));
            } else if listed.len() > 1 {
                attribute_relay_spans(
                    problem,
                    &listed,
                    first.depart,
                    last.depart,
                    &mut driver_spans,
                );
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
                        load = load.saturating_add(store.demand);
                        if !load_reported && !load.fits_within(vehicle.capacity) {
                            violations.push(Violation::LoadExceeded {
                                route: route_index,
                                step: step_index,
                            });
                            load_reported = true;
                        }
                        served[store_id.0 as usize] += 1;
                        let effective_windows = problem.chain_rules.windows_for_store(store);
                        let window_ok = effective_windows.is_some_and(|windows| {
                            windows.iter().any(|window| {
                                window.start <= step.arrive && step.depart <= window.end
                            })
                        });
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
            let day_minutes: i64 = 24 * 60;
            let work_in_window = max_work_in_fixed_window(spans, day_minutes);
            if work_in_window > max_work_24h {
                violations.push(Violation::DriverWorkHoursExceeded {
                    driver: driver_id,
                    work_hours: work_in_window as f64 / 60.0,
                    limit_hours: max_work_24h as f64 / 60.0,
                });
            }

            // Check rolling 7-day work limit.
            let week_minutes = 7 * day_minutes;
            let work_in_window = max_work_in_fixed_window(spans, week_minutes);
            if work_in_window > max_work_7d {
                violations.push(Violation::DriverWeeklyHoursExceeded {
                    driver: driver_id,
                    work_hours: work_in_window as f64 / 60.0,
                    limit_hours: max_work_7d as f64 / 60.0,
                });
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Problem {
    pub name: String,
    pub depots: Vec<Depot>,
    pub stores: Vec<Store>,
    pub vehicles: Vec<Vehicle>,
    pub drivers: Vec<Driver>,
    pub cost: CostModel,
    /// Chain-specific default receiving windows. Stores without explicit
    /// receiving windows inherit from their chain's defaults.
    #[serde(default)]
    pub chain_rules: ChainRules,
    /// Truck curfew rules for certain road classes.
    ///
    /// Enforced by `solve --enforce-curfews`: `routing::timed` computes each
    /// traversal's presence interval against the rules (a truck waits at the
    /// previous node until its traversal is clear) and
    /// `solver::enforce_curfews` re-schedules the plan, dropping stores whose
    /// waits no longer fit their receiving windows. Enforcement requires
    /// [`Problem::horizon_start`] (to resolve local days/times) and an OSM
    /// graph (edges carry the road class); without an anchor the rules are
    /// inert import/export data.
    #[serde(default)]
    pub curfews: CurfewRules,
    /// Absolute instant corresponding to horizon minute zero (RFC 3339).
    ///
    /// When set, `Depot::timezone`/`Store::timezone` IANA names resolve to
    /// local civil times via [`crate::clock`]; when `None` every timestamp is
    /// horizon-relative and timezones stay inert data. `None` also keeps
    /// existing fixtures valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon_start: Option<jiff::Timestamp>,
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

    /// Validates problem-level configuration that no solution can repair.
    ///
    /// Currently: `Depot::timezone`/`Store::timezone` must be known IANA
    /// names when set. This deliberately lives here and **not** in
    /// [`Solution::validate`] — solvers assert that their output validates,
    /// and a bad timezone in the input is not something a solver can fix.
    /// Report these violations at the boundary (CLI/API) before solving.
    #[must_use]
    pub fn validate_timezones(&self) -> Vec<Violation> {
        let mut violations = Vec::new();
        for name in self
            .depots
            .iter()
            .filter_map(|depot| depot.timezone.as_ref())
            .chain(
                self.stores
                    .iter()
                    .filter_map(|store| store.timezone.as_ref()),
            )
        {
            if !crate::clock::is_valid_timezone(name) {
                violations.push(Violation::InvalidTimezone {
                    timezone: name.clone(),
                });
            }
        }
        violations
    }
}

/// Attributes slices of a multi-driver relay route `[route_start, route_end]`
/// across `listed` drivers using their shift boundaries so each driver is
/// charged only for the portion they cover rather than the full relay span.
fn attribute_relay_spans(
    problem: &Problem,
    listed: &[DriverId],
    route_start: Minutes,
    route_end: Minutes,
    driver_spans: &mut [Vec<(Minutes, Minutes)>],
) {
    if route_end <= route_start {
        return;
    }

    let mut boundaries = vec![route_start, route_end];
    for &driver_id in listed {
        let Some(driver) = problem.driver(driver_id) else {
            continue;
        };
        for shift in &driver.shifts {
            if shift.start > route_start && shift.start < route_end {
                boundaries.push(shift.start);
            }
            if shift.end > route_start && shift.end < route_end {
                boundaries.push(shift.end);
            }
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut per_driver: Vec<Vec<(Minutes, Minutes)>> = vec![Vec::new(); listed.len()];
    let mut active_idx: Option<usize> = None;

    for pair in boundaries.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b <= a {
            continue;
        }
        let covering: Vec<usize> = listed
            .iter()
            .enumerate()
            .filter(|&(_, &driver_id)| {
                problem
                    .driver(driver_id)
                    .is_some_and(|d| d.shifts.iter().any(|s| s.covers(a, b)))
            })
            .map(|(i, _)| i)
            .collect();

        if covering.is_empty() {
            active_idx = None;
            continue;
        }

        let slice_len = b.minutes() - a.minutes();
        let fits_24h = |idx: usize| -> bool {
            let Some(driver) = problem.driver(listed[idx]) else {
                return false;
            };
            let current_contig = per_driver[idx]
                .last()
                .filter(|(_, end)| *end == a)
                .map_or(0, |(start, end)| end.minutes() - start.minutes());
            current_contig + slice_len <= driver.fatigue_rules.max_work_24h_minutes()
        };

        let chosen = if covering.len() == 1 {
            covering[0]
        } else if let Some(prev) =
            active_idx.filter(|prev| covering.contains(prev) && fits_24h(*prev))
        {
            prev
        } else {
            *covering
                .iter()
                .find(|&&idx| fits_24h(idx))
                .unwrap_or(&covering[0])
        };

        if let Some(last) = per_driver[chosen].last_mut().filter(|(_, end)| *end == a) {
            last.1 = b;
        } else {
            per_driver[chosen].push((a, b));
        }
        active_idx = Some(chosen);
    }

    for (idx, spans) in per_driver.into_iter().enumerate() {
        driver_spans[listed[idx].0 as usize].extend(spans);
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
                address: None,
                timezone: None,
            }],
            stores: vec![Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "Store".into(),
                location: LatLon::new(-33.92, 151.23),
                address: None,
                timezone: None,
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
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
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
    fn fatigue_work_intersecting_24h_window_is_not_overcounted() {
        // A 700-minute span ending at minute 700 contributes only its 700
        // minutes to the [0, 1440) window, even if the following span starts
        // just inside that window. Actual work is 710 minutes, below the
        // Standard 12-hour limit.
        let problem = fatigue_test_problem(FatigueRules::Standard);
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
                            arrive: Minutes::from_minutes(700),
                            depart: Minutes::from_minutes(700),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(700),
                    cost_cents: 0,
                },
                RoutePlan {
                    vehicle: VehicleId(1),
                    drivers: vec![DriverId(0)],
                    steps: vec![
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(1430),
                            depart: Minutes::from_minutes(1430),
                        },
                        PlannedStop {
                            node: Node::Depot(DepotId(0)),
                            arrive: Minutes::from_minutes(1490),
                            depart: Minutes::from_minutes(1490),
                        },
                    ],
                    distance_km: 0.0,
                    duration: Minutes::from_minutes(60),
                    cost_cents: 0,
                },
            ],
            unassigned: vec![StoreId(0)],
            total_distance_km: 0.0,
            total_duration: Minutes::from_minutes(760),
            total_cost_cents: 0,
        };
        assert!(solution.validate(&problem).is_ok());
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

    #[test]
    fn chain_rules_windows_for_each_chain() {
        let rules = ChainRules {
            coles: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
            woolworths: vec![TimeWindow::new(
                Minutes::from_minutes(720),
                Minutes::from_minutes(960),
            )],
            aldi: vec![TimeWindow::new(
                Minutes::from_minutes(900),
                Minutes::from_minutes(1200),
            )],
            iga: Vec::new(),
            returnables: ReturnablesDefaults {
                coles: Load::new(2, 2, 1),
                ..ReturnablesDefaults::default()
            },
        };
        assert_eq!(
            rules.returnables_for(Chain::Coles),
            Load::new(2, 2, 1),
            "chain returnables lookup"
        );
        assert_eq!(
            rules.returnables_for(Chain::Iga),
            Load::ZERO,
            "unset banners collect nothing"
        );

        assert_eq!(
            rules
                .windows_for(Chain::Coles)
                .map(|w| w.len())
                .unwrap_or(0),
            1
        );
        assert_eq!(
            rules
                .windows_for(Chain::Woolworths)
                .map(|w| w.len())
                .unwrap_or(0),
            1
        );
        assert_eq!(
            rules.windows_for(Chain::Aldi).map(|w| w.len()).unwrap_or(0),
            1
        );
        assert_eq!(
            rules.windows_for(Chain::Iga).map(|w| w.len()).unwrap_or(0),
            0
        );
    }

    #[test]
    fn chain_rules_windows_for_store_uses_store_windows_first() {
        let rules = ChainRules {
            coles: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
            woolworths: vec![],
            aldi: vec![],
            iga: vec![],
            returnables: ReturnablesDefaults::default(),
        };

        // Store with explicit windows uses those, not chain defaults
        let store = Store {
            id: StoreId(0),
            chain: Chain::Coles,
            name: "Test Store".into(),
            location: LatLon::new(-33.87, 151.21),
            address: None,
            demand: Load::ZERO,
            service: Minutes::ZERO,
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(700),
                Minutes::from_minutes(800),
            )],
            timezone: None,
        };
        assert_eq!(
            rules
                .windows_for_store(&store)
                .map(|w| w.len())
                .unwrap_or(0),
            1
        );
        assert_eq!(
            rules.windows_for_store(&store).unwrap()[0].start,
            Minutes::from_minutes(700)
        );
    }

    #[test]
    fn chain_rules_windows_for_store_falls_back_to_chain_defaults() {
        let rules = ChainRules {
            coles: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
            woolworths: vec![],
            aldi: vec![],
            iga: vec![],
            returnables: ReturnablesDefaults::default(),
        };

        // Store with no receiving windows uses chain defaults
        let store = Store {
            id: StoreId(0),
            chain: Chain::Coles,
            name: "Test Store".into(),
            location: LatLon::new(-33.87, 151.21),
            address: None,
            demand: Load::ZERO,
            service: Minutes::ZERO,
            receiving: vec![],
            timezone: None,
        };
        assert_eq!(
            rules
                .windows_for_store(&store)
                .map(|w| w.len())
                .unwrap_or(0),
            1
        );
        assert_eq!(
            rules.windows_for_store(&store).unwrap()[0].start,
            Minutes::ZERO
        );
        assert_eq!(
            rules.windows_for_store(&store).unwrap()[0].end,
            Minutes::from_minutes(600)
        );
    }

    #[test]
    fn chain_rules_empty_profile_has_no_windows() {
        let rules = ChainRules::default();
        assert_eq!(
            rules
                .windows_for(Chain::Coles)
                .map(|w| w.len())
                .unwrap_or(0),
            0
        );
        assert_eq!(
            rules
                .windows_for(Chain::Woolworths)
                .map(|w| w.len())
                .unwrap_or(0),
            0
        );
        assert_eq!(
            rules.windows_for(Chain::Aldi).map(|w| w.len()).unwrap_or(0),
            0
        );
        assert_eq!(
            rules.windows_for(Chain::Iga).map(|w| w.len()).unwrap_or(0),
            0
        );
    }

    #[test]
    fn curfew_rules_block_curtailed_edges() {
        let rules = CurfewRules {
            rules: vec![
                CurfewRule {
                    road_class: "residential".to_string(),
                    start: Minutes::from_minutes(600), // 10:00
                    end: Minutes::from_minutes(900),   // 15:00
                    days_of_week: None,                // all days
                },
                CurfewRule {
                    road_class: "arterial".to_string(),
                    start: Minutes::from_minutes(1080), // 18:00
                    end: Minutes::from_minutes(1200),   // 20:00
                    // Monday-Friday (0 = Sunday, 6 = Saturday).
                    days_of_week: Some([false, true, true, true, true, true, false]),
                },
            ],
        };

        // Residential road at 10:00 is curtailed
        assert!(rules.is_curtailed("residential", Minutes::from_minutes(600), 0));

        // Residential road at 09:00 is NOT curtailed (before window)
        assert!(!rules.is_curtailed("residential", Minutes::from_minutes(540), 0));

        // Residential road at 15:00 is NOT curtailed (after window)
        assert!(!rules.is_curtailed("residential", Minutes::from_minutes(900), 0));

        // Arterial road at 18:00 on Monday IS curtailed
        assert!(rules.is_curtailed("arterial", Minutes::from_minutes(1080), 1));

        // Arterial road at 18:00 on Saturday is NOT curtailed (not a weekday)
        assert!(!rules.is_curtailed("arterial", Minutes::from_minutes(1080), 6));

        // Unknown road class is never curtailed
        assert!(!rules.is_curtailed("motorway", Minutes::from_minutes(600), 0));
    }

    #[test]
    fn curfew_rules_empty_has_no_restrictions() {
        let rules = CurfewRules::default();
        assert!(!rules.is_curtailed("residential", Minutes::ZERO, 0));
        assert!(!rules.is_curtailed("arterial", Minutes::from_minutes(1440), 0));
    }

    #[test]
    fn curfew_window_wrapping_midnight_covers_the_next_morning() {
        // Friday 22:00 -> 06:00, Friday only (day 5).
        let rules = CurfewRules {
            rules: vec![CurfewRule {
                road_class: "residential".to_string(),
                start: Minutes::from_minutes(22 * 60),
                end: Minutes::from_minutes(6 * 60),
                days_of_week: Some([false, false, false, false, false, true, false]),
            }],
        };
        let friday = 5;
        let saturday = 6;

        // Friday evening: covered.
        assert!(rules.is_curtailed("residential", Minutes::from_minutes(22 * 60 + 30), friday));
        // Saturday 02:00: still covered (morning part of Friday's window).
        assert!(rules.is_curtailed("residential", Minutes::from_minutes(120), saturday));
        // Saturday morning after release: not covered.
        assert!(!rules.is_curtailed("residential", Minutes::from_minutes(360), saturday));
        // Saturday evening: Saturday is not selected.
        assert!(!rules.is_curtailed("residential", Minutes::from_minutes(22 * 60 + 30), saturday));
        // Sunday 02:00: Friday's window has long released.
        assert!(!rules.is_curtailed("residential", Minutes::from_minutes(120), 0));
        // Friday 05:00: the morning part would belong to Thursday,
        // which is not selected.
        assert!(!rules.is_curtailed("residential", Minutes::from_minutes(300), friday));
    }

    #[test]
    fn next_allowed_delta_lands_on_window_boundaries() {
        let rules = CurfewRules {
            rules: vec![CurfewRule {
                road_class: "residential".to_string(),
                start: Minutes::from_minutes(22 * 60),
                end: Minutes::from_minutes(6 * 60),
                days_of_week: None,
            }],
        };
        // Not curtailed: no wait.
        assert_eq!(rules.next_allowed_delta("residential", 12 * 60, 3), 0);
        // Friday 22:30: wait until 06:00 tomorrow = 90 + 360 = 450 minutes.
        assert_eq!(
            rules.next_allowed_delta("residential", 22 * 60 + 30, 5),
            450
        );
        // Saturday 02:00: wait until 06:00 = 240 minutes.
        assert_eq!(rules.next_allowed_delta("residential", 120, 6), 240);
    }

    #[test]
    fn next_allowed_delta_steps_past_overlapping_windows() {
        // Two back-to-back windows: 10:00-12:00 and 11:30-13:00.
        let rules = CurfewRules {
            rules: vec![
                CurfewRule {
                    road_class: "primary".to_string(),
                    start: Minutes::from_minutes(600),
                    end: Minutes::from_minutes(720),
                    days_of_week: None,
                },
                CurfewRule {
                    road_class: "primary".to_string(),
                    start: Minutes::from_minutes(690),
                    end: Minutes::from_minutes(780),
                    days_of_week: None,
                },
            ],
        };
        // At 10:00 the first release is 12:00 (120 minutes); the caller must
        // re-check and then wait to 13:00.
        let first = rules.next_allowed_delta("primary", 600, 1);
        assert_eq!(first, 120);
        assert!(rules.is_curtailed("primary", Minutes::from_minutes(600 + first), 1));
        let second = rules.next_allowed_delta("primary", 600 + first, 1);
        assert_eq!(second, 60);
        assert!(!rules.is_curtailed("primary", Minutes::from_minutes(600 + first + second), 1));
    }

    #[test]
    fn empty_curfew_window_never_applies() {
        let rules = CurfewRules {
            rules: vec![CurfewRule {
                road_class: "residential".to_string(),
                start: Minutes::from_minutes(600),
                end: Minutes::from_minutes(600),
                days_of_week: None,
            }],
        };
        assert!(!rules.is_curtailed("residential", Minutes::from_minutes(600), 1));
        assert_eq!(rules.next_allowed_delta("residential", 600, 1), 0);
    }

    #[test]
    fn general_chain_defaults_and_optional_addresses_roundtrip() {
        assert_eq!(Chain::default(), Chain::General);
        assert_eq!(Chain::General.to_string(), "General");
        let parsed: Chain = serde_json::from_str("\"custom\"").unwrap();
        assert_eq!(parsed, Chain::General);
        assert!(ChainRules::default().windows_for(Chain::General).is_none());
        assert_eq!(
            ReturnablesDefaults::default().for_chain(Chain::General),
            Load::ZERO
        );

        let depot = Depot {
            id: DepotId(0),
            name: "HQ".into(),
            location: LatLon::new(-33.8688, 151.2093),
            address: Some("100 George St, Sydney NSW 2000".into()),
            timezone: None,
        };
        let encoded = serde_json::to_string(&depot).unwrap();
        assert!(encoded.contains("100 George St"));
        let decoded: Depot = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, depot);

        let legacy_json = r#"{"id":0,"name":"HQ","location":{"lat":-33.87,"lon":151.21}}"#;
        let legacy: Depot = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy.address, None);
    }
}
