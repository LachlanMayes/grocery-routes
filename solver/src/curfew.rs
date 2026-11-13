//! Curfew enforcement: re-schedule a solved plan with time-dependent,
//! curfew-aware leg times.
//!
//! The planner builds routes on a static matrix. When `Problem::curfews`
//! names road classes (OSM `highway` values), a plan is feasible only if each
//! traversal fits between curfew windows. Enforcement re-simulates every route
//! leg with a caller-supplied [`LegTimes`] provider (the CLI wires
//! `routing::timed` to the OSM graph), keeping the stop order and fleet
//! assignment fixed. A route whose waits no longer fit its receiving windows,
//! shift or duration budget shrinks: the offending store moves to
//! `Solution::unassigned` with a recorded [`CurfewDrop`].
//!
//! This is deliberately a feasibility restore, not a curfew-aware optimiser.
//! Waiting is the only remedy applied; route quality under curfews is out of
//! scope, and stores dropped here are not re-inserted elsewhere.

use std::fmt;

use domain::{DriverId, Load, Minutes, Node, PlannedStop, Problem, RoutePlan, Solution, StoreId};

use crate::greedy::{
    ScheduleStart, add_load, next_service_start, route_cost, schedule_start, spans_overlap,
};
use crate::matrix::DistanceMatrix;

/// Supplies time-dependent leg times, e.g. from `routing::timed`.
pub trait LegTimes {
    /// The leg from `from` to `to` departing at `depart`, or `None` when no
    /// curfew-feasible path exists for that departure.
    fn leg(&mut self, from: Node, to: Node, depart: Minutes) -> Option<routing::TimedLeg>;
}

/// A [`LegTimes`] implementation that reproduces the static solution's own
/// matrix times (no curfews). Mostly useful for tests and diagnostics.
#[derive(Clone, Debug)]
pub struct StaticLegs {
    matrix: DistanceMatrix,
    speed_kmh: f64,
}

impl StaticLegs {
    /// Builds a static provider for `problem`'s first vehicle speed.
    #[must_use]
    pub fn new(problem: &Problem) -> Self {
        let speed_kmh = problem
            .vehicles
            .first()
            .map_or(45.0, |vehicle| vehicle.avg_speed_kmh);
        Self {
            matrix: DistanceMatrix::new(problem),
            speed_kmh,
        }
    }
}

impl LegTimes for StaticLegs {
    fn leg(&mut self, from: Node, to: Node, depart: Minutes) -> Option<routing::TimedLeg> {
        let distance_km = self.matrix.distance_km(from, to)?;
        let drive = self
            .matrix
            .travel_minutes_between(from, to, self.speed_kmh)?
            .minutes();
        Some(routing::TimedLeg {
            nodes: Vec::new(),
            steps: Vec::new(),
            distance_km,
            drive_minutes: drive,
            wait_minutes: 0,
            arrive: Minutes::from_minutes(depart.minutes() + drive),
        })
    }
}

/// Why a store was removed from its route during curfew enforcement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreachKind {
    /// No curfew-feasible path existed for a leg.
    NoCurfewFeasiblePath,
    /// Waiting pushed a store's service outside its receiving windows.
    ReceivingWindowMissed,
    /// Waiting pushed a leg or service past the shift end.
    ShiftEndExceeded,
    /// Waiting pushed the route past the vehicle's maximum duration.
    MaxDurationExceeded,
    /// The adjusted span would overlap another route of the same driver.
    DriverConflict,
    /// The route's shape cannot be re-scheduled (missing vehicle/store).
    InvalidRoute,
}

impl fmt::Display for BreachKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoCurfewFeasiblePath => "no curfew-feasible path",
            Self::ReceivingWindowMissed => "receiving window missed after curfew wait",
            Self::ShiftEndExceeded => "shift end exceeded after curfew wait",
            Self::MaxDurationExceeded => "vehicle maximum duration exceeded after curfew wait",
            Self::DriverConflict => "adjusted span would overlap another route of the driver",
            Self::InvalidRoute => "route shape cannot be re-scheduled",
        })
    }
}

/// One store removed from a route during enforcement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurfewDrop {
    pub store: StoreId,
    pub route: usize,
    pub kind: BreachKind,
}

/// Summary of one enforcement pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurfewReport {
    /// Routes whose schedule changed (any waiting occurred).
    pub routes_adjusted: usize,
    /// Total curfew waiting time across all routes.
    pub total_wait_minutes: i64,
    /// Stores moved to `unassigned`, in removal order.
    pub dropped: Vec<CurfewDrop>,
}

/// A candidate re-schedule that fits one shift of one of the route's drivers.
#[derive(Clone, Debug)]
struct Attempt {
    route: RoutePlan,
    span: (Minutes, Minutes),
    wait_minutes: i64,
}

type Breach = (BreachKind, Option<StoreId>);

/// Re-schedules `solution`'s routes with `legs`, dropping stores whose waits
/// no longer fit. Returns the feasible plan and a report of what changed.
///
/// The plan keeps every store that remains feasible and moves the rest to
/// `unassigned`; routes that lose all their stores are removed.
#[must_use]
pub fn enforce_curfews(
    problem: &Problem,
    solution: &Solution,
    legs: &mut dyn LegTimes,
) -> (Solution, CurfewReport) {
    let mut routes_out: Vec<RoutePlan> = Vec::with_capacity(solution.routes.len());
    let mut report = CurfewReport::default();
    let mut unassigned = solution.unassigned.clone();

    // Spans of finalised routes, per driver index.
    let mut committed: Vec<Vec<(Minutes, Minutes)>> = vec![Vec::new(); problem.drivers.len()];
    // Original spans of routes not yet processed: (route index, driver, span).
    let mut remaining: Vec<(usize, DriverId, (Minutes, Minutes))> = solution
        .routes
        .iter()
        .enumerate()
        .flat_map(|(index, route)| {
            let span = original_span(route);
            route
                .drivers
                .iter()
                .map(move |&driver| (index, driver, span))
        })
        .collect();

    for (route_index, route) in solution.routes.iter().enumerate() {
        remaining.retain(|&(index, _, _)| index != route_index);
        let Some((attempt, drops)) =
            reschedule_route(problem, route, route_index, legs, &committed, &remaining)
        else {
            continue;
        };
        report.dropped.extend(drops);
        for &driver in &attempt.route.drivers {
            if let Some(spans) = committed.get_mut(driver.0 as usize) {
                spans.push(attempt.span);
            }
        }
        if attempt.wait_minutes > 0 {
            report.routes_adjusted += 1;
        }
        report.total_wait_minutes += attempt.wait_minutes;
        routes_out.push(attempt.route);
    }

    for drop in &report.dropped {
        unassigned.push(drop.store);
    }
    unassigned.sort_unstable();

    let total_distance_km = routes_out.iter().map(|route| route.distance_km).sum();
    let total_duration = routes_out
        .iter()
        .map(|route| route.duration.minutes())
        .sum();
    let total_cost_cents = routes_out.iter().map(|route| route.cost_cents).sum();
    let enforced = Solution {
        routes: routes_out,
        unassigned,
        total_distance_km,
        total_duration: Minutes::from_minutes(total_duration),
        total_cost_cents,
    };
    assert!(
        enforced.validate(problem).is_ok(),
        "curfew enforcement produced an invalid solution"
    );
    (enforced, report)
}

/// Re-schedules one route, dropping stores until it fits the driver
/// commitments; returns the final attempt and every drop it recorded.
fn reschedule_route(
    problem: &Problem,
    route: &RoutePlan,
    route_index: usize,
    legs: &mut dyn LegTimes,
    committed: &[Vec<(Minutes, Minutes)>],
    remaining: &[(usize, DriverId, (Minutes, Minutes))],
) -> Option<(Attempt, Vec<CurfewDrop>)> {
    let mut candidate = route.clone();
    let mut drops: Vec<CurfewDrop> = Vec::new();
    loop {
        if route_stores(&candidate).is_empty() {
            // Every store on this route has been dropped; remove it.
            return None;
        }
        match try_reschedule(problem, &candidate, legs) {
            Ok(attempt) => {
                if driver_conflicts(
                    problem,
                    &candidate.drivers,
                    attempt.span,
                    committed,
                    remaining,
                ) {
                    let store = last_store(&candidate)?;
                    drops.push(CurfewDrop {
                        store,
                        route: route_index,
                        kind: BreachKind::DriverConflict,
                    });
                    drop_store(&mut candidate, store);
                    continue;
                }
                return Some((attempt, drops));
            }
            Err((kind, Some(store))) => {
                drops.push(CurfewDrop {
                    store,
                    route: route_index,
                    kind,
                });
                drop_store(&mut candidate, store);
            }
            Err((kind, None)) => {
                // The whole route is unusable: record its stores.
                for store in route_stores(&candidate) {
                    drops.push(CurfewDrop {
                        store,
                        route: route_index,
                        kind,
                    });
                }
                return None;
            }
        }
    }
}

/// The solution span a route is booked for: first departure to last departure.
fn original_span(route: &RoutePlan) -> (Minutes, Minutes) {
    match (route.steps.first(), route.steps.last()) {
        (Some(first), Some(last)) => (first.depart, last.depart),
        _ => (Minutes::ZERO, Minutes::ZERO),
    }
}

/// True when `span` would clash with an already-finalised route span or a
/// not-yet-processed route span of any of `drivers`.
///
/// "Clash" means the same three things [`Solution::validate`] checks for a
/// driver: overlapping bookings, less than the fatigue rules' minimum rest
/// between consecutive spans, and exceeding the rolling 24-hour or 7-day
/// work caps. Without the fatigue checks, a curfew wait that ate a driver's
/// rest would slip past enforcement and then trip validate's assert.
fn driver_conflicts(
    problem: &Problem,
    drivers: &[DriverId],
    span: (Minutes, Minutes),
    committed: &[Vec<(Minutes, Minutes)>],
    remaining: &[(usize, DriverId, (Minutes, Minutes))],
) -> bool {
    drivers.iter().any(|driver| {
        let mut spans: Vec<(Minutes, Minutes)> = committed
            .get(driver.0 as usize)
            .into_iter()
            .flatten()
            .copied()
            .chain(
                remaining
                    .iter()
                    .filter(|&&(_, other_driver, _)| other_driver == *driver)
                    .map(|&(_, _, other_span)| other_span),
            )
            .collect();
        if spans.iter().any(|&other| spans_overlap(other, span)) {
            return true;
        }
        let Some(driver_record) = problem.driver(*driver) else {
            return false;
        };
        spans.push(span);
        spans.sort_unstable();
        let rules = driver_record.fatigue_rules;
        if spans
            .windows(2)
            .any(|pair| pair[1].0.minutes() - pair[0].1.minutes() < rules.min_rest_minutes())
        {
            return true;
        }
        if domain::max_work_in_fixed_window(&spans, 24 * 60) > rules.max_work_24h_minutes() {
            return true;
        }
        if domain::max_work_in_fixed_window(&spans, 7 * 24 * 60) > rules.max_work_7d_minutes() {
            return true;
        }
        false
    })
}

/// Stores on a route, in visit order.
fn route_stores(route: &RoutePlan) -> Vec<StoreId> {
    route
        .steps
        .iter()
        .filter_map(|step| match step.node {
            Node::Store(id) => Some(id),
            Node::Depot(_) => None,
        })
        .collect()
}

fn last_store(route: &RoutePlan) -> Option<StoreId> {
    route_stores(route).pop()
}

/// Removes one store's stop from the route.
fn drop_store(route: &mut RoutePlan, store: StoreId) {
    route.steps.retain(|step| step.node != Node::Store(store));
}

/// Tries to re-schedule `route` with `legs`, one shift at a time.
///
/// Tries every driver listed on the route and each of their shifts, in
/// declaration order; returns the first feasible schedule or the first
/// failure encountered (deterministic because the order is fixed).
fn try_reschedule(
    problem: &Problem,
    route: &RoutePlan,
    legs: &mut dyn LegTimes,
) -> Result<Attempt, Breach> {
    let (vehicle, home, end, store_nodes) = route_shape(problem, route)?;
    let mut first_error: Option<Breach> = None;
    for &driver in &route.drivers {
        let Some(driver_record) = problem.driver(driver) else {
            continue;
        };
        for &shift in &driver_record.shifts {
            match simulate_shift(problem, vehicle, shift, home, end, &store_nodes, legs) {
                Ok(mut attempt) => {
                    // The re-scheduled route keeps its vehicle and driver
                    // list; only its times and metrics change.
                    attempt.route.vehicle = route.vehicle;
                    attempt.route.drivers = route.drivers.clone();
                    return Ok(attempt);
                }
                Err(breach) => {
                    if first_error.is_none() {
                        first_error = Some(breach);
                    }
                }
            }
        }
    }
    Err(first_error.unwrap_or((BreachKind::InvalidRoute, None)))
}

/// Extracts `(vehicle, home, end, store nodes)` from a route plan, or the
/// `InvalidRoute` breach when the vehicle is unknown or the plan does not
/// start and end at depots (mid-route depots fail later, in `simulate_shift`).
fn route_shape<'a>(
    problem: &'a Problem,
    route: &RoutePlan,
) -> Result<(&'a domain::Vehicle, Node, Node, Vec<Node>), Breach> {
    let Some(vehicle) = problem.vehicle(route.vehicle) else {
        return Err((BreachKind::InvalidRoute, None));
    };
    let sequence: Vec<Node> = route.steps.iter().map(|step| step.node).collect();
    let Some((&home, rest)) = sequence.split_first() else {
        return Err((BreachKind::InvalidRoute, None));
    };
    let Some((&end, store_nodes)) = rest.split_last() else {
        return Err((BreachKind::InvalidRoute, None));
    };
    if !matches!(home, Node::Depot(_)) || !matches!(end, Node::Depot(_)) {
        return Err((BreachKind::InvalidRoute, None));
    }
    Ok((vehicle, home, end, store_nodes.to_vec()))
}

/// Simulates the whole route against one shift, asking `legs` for every leg.
fn simulate_shift(
    problem: &Problem,
    vehicle: &domain::Vehicle,
    shift: domain::Shift,
    home: Node,
    end: Node,
    store_nodes: &[Node],
    legs: &mut dyn LegTimes,
) -> Result<Attempt, Breach> {
    let ScheduleStart {
        mut steps,
        mut time,
        mut previous,
        mut load,
        mut distance_km,
    } = schedule_start(home, shift, store_nodes.len() + 2);
    let mut wait_minutes = 0i64;

    for &node in store_nodes {
        let visit = store_visit(problem, vehicle, shift, (previous, time), node, load, legs)?;
        distance_km += visit.distance_km;
        wait_minutes += visit.wait_minutes;
        time = visit.stop.depart;
        steps.push(visit.stop);
        previous = node;
        load = visit.load;
    }

    let final_store = route_last_store(store_nodes);
    let Some(leg) = legs.leg(previous, end, time) else {
        return Err((BreachKind::NoCurfewFeasiblePath, final_store));
    };
    distance_km += leg.distance_km;
    wait_minutes += leg.wait_minutes;
    let arrival = time + Minutes::from_minutes(leg.total_minutes());
    if leg.arrive > shift.end || arrival > shift.end {
        return Err((BreachKind::ShiftEndExceeded, final_store));
    }
    let duration = arrival - shift.start;
    if duration > vehicle.max_duration {
        return Err((BreachKind::MaxDurationExceeded, final_store));
    }
    steps.push(PlannedStop {
        node: end,
        arrive: arrival,
        depart: arrival,
    });

    Ok(Attempt {
        route: RoutePlan {
            vehicle: vehicle.id,
            // The caller restores the original driver list.
            drivers: Vec::new(),
            steps,
            distance_km,
            duration,
            cost_cents: route_cost(problem.cost, distance_km, duration),
        },
        span: (shift.start, arrival),
        wait_minutes,
    })
}

/// One store's scheduled stop plus the leg it took to reach it.
struct StoreVisit {
    stop: PlannedStop,
    load: Load,
    distance_km: f64,
    wait_minutes: i64,
}

/// Schedules one store visit after `previous` at `time`, or the breach that
/// makes it infeasible. `load` is the load carried before the visit.
fn store_visit(
    problem: &Problem,
    vehicle: &domain::Vehicle,
    shift: domain::Shift,
    (previous, time): (Node, Minutes),
    node: Node,
    load: Load,
    legs: &mut dyn LegTimes,
) -> Result<StoreVisit, Breach> {
    let Node::Store(store_id) = node else {
        return Err((BreachKind::InvalidRoute, None));
    };
    let Some(store) = problem.store(store_id) else {
        return Err((BreachKind::InvalidRoute, Some(store_id)));
    };
    let Some(leg) = legs.leg(previous, node, time) else {
        return Err((BreachKind::NoCurfewFeasiblePath, Some(store_id)));
    };
    let physical = time + Minutes::from_minutes(leg.total_minutes());
    if leg.arrive > shift.end || physical > shift.end {
        return Err((BreachKind::ShiftEndExceeded, Some(store_id)));
    }
    let Some(service_start) = next_service_start(problem, store, physical) else {
        return Err((BreachKind::ReceivingWindowMissed, Some(store_id)));
    };
    let depart = service_start + store.service;
    if depart > shift.end {
        return Err((BreachKind::ShiftEndExceeded, Some(store_id)));
    }
    let Some(next_load) = add_load(load, store.demand) else {
        return Err((BreachKind::InvalidRoute, Some(store_id)));
    };
    if !next_load.fits_within(vehicle.capacity) {
        return Err((BreachKind::InvalidRoute, Some(store_id)));
    }
    Ok(StoreVisit {
        stop: PlannedStop {
            node,
            arrive: service_start,
            depart,
        },
        load: next_load,
        distance_km: leg.distance_km,
        wait_minutes: leg.wait_minutes,
    })
}

fn route_last_store(store_nodes: &[Node]) -> Option<StoreId> {
    store_nodes.iter().rev().find_map(|node| match node {
        Node::Store(id) => Some(*id),
        Node::Depot(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solve_greedy;
    use domain::{
        Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, FatigueRules, LatLon,
        Load, Shift, Store, TimeWindow, Vehicle, VehicleId,
    };

    /// Shared two-store fixture (used by curfew and curfew-aware tests).
    /// Marked as a fixture block: the `Problem` payload shapes intentionally
    /// mirror the crate's other test fixtures (see the code-quality skill on
    /// test duplication).
    // jscpd:ignore-start
    #[cfg(test)]
    pub(crate) fn problem_with_two_stores(
        store_a_window: (i64, i64),
        store_b_window: (i64, i64),
    ) -> Problem {
        let store =
            |id: u32, name: &'static str, lat: f64, demand: Load, (start, end): (i64, i64)| Store {
                id: StoreId(id),
                chain: Chain::Coles,
                name: name.into(),
                location: LatLon::new(lat, if name == "A" { 151.30 } else { 151.10 }),
                address: None,
                demand,
                service: Minutes::from_minutes(10),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(start),
                    Minutes::from_minutes(end),
                )],
                timezone: None,
            };
        Problem {
            name: "curfew fixture".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "DC".into(),
                location: LatLon::new(-33.87, 151.21),
                address: None,
                timezone: None,
            }],
            stores: vec![
                store(0, "A", -33.80, Load::new(2, 2, 1), store_a_window),
                store(1, "B", -33.95, Load::new(1, 1, 1), store_b_window),
            ],
            vehicles: vec![Vehicle {
                id: VehicleId(0),
                name: "Rigid".into(),
                depot: DepotId(0),
                capacity: Load::new(8, 8, 4),
                max_duration: Minutes::from_minutes(600),
                avg_speed_kmh: 60.0,
            }],
            drivers: vec![Driver {
                id: DriverId(0),
                name: "Sam".into(),
                shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(600))],
                fatigue_rules: FatigueRules::default(),
            }],
            cost: CostModel::default(),
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
        }
    }
    // jscpd:ignore-end

    /// A provider that delegates to the static matrix for `base`, then adds
    /// `extra_wait` of curfew waiting to every leg.
    struct WaitProvider {
        base: StaticLegs,
        extra_wait: i64,
    }

    impl LegTimes for WaitProvider {
        fn leg(&mut self, from: Node, to: Node, depart: Minutes) -> Option<routing::TimedLeg> {
            let mut leg = self.base.leg(from, to, depart)?;
            leg.wait_minutes = self.extra_wait;
            leg.arrive =
                Minutes::from_minutes(depart.minutes() + leg.drive_minutes + leg.wait_minutes);
            Some(leg)
        }
    }

    #[test]
    fn static_provider_leaves_the_plan_unchanged() {
        let problem = problem_with_two_stores((0, 600), (0, 600));
        let solution = solve_greedy(&problem);
        let mut provider = StaticLegs::new(&problem);
        let (enforced, report) = enforce_curfews(&problem, &solution, &mut provider);
        assert_eq!(report.dropped, Vec::new());
        assert_eq!(report.total_wait_minutes, 0);
        assert_eq!(enforced, solution);
    }

    #[test]
    fn curfew_waits_shift_the_schedule_and_validate() {
        let problem = problem_with_two_stores((0, 600), (0, 600));
        let solution = solve_greedy(&problem);
        let mut provider = WaitProvider {
            base: StaticLegs::new(&problem),
            extra_wait: 30,
        };
        let (enforced, report) = enforce_curfews(&problem, &solution, &mut provider);
        assert_eq!(report.dropped, Vec::new());
        assert!(report.total_wait_minutes > 0);
        assert_eq!(report.routes_adjusted, 1);
        assert_eq!(enforced.validate(&problem), Ok(()));
        // Every departure is at least 30 minutes later than the original on
        // the first leg; the first store's service starts later too.
        let original_depart = solution.routes[0].steps[0].depart;
        let enforced_depart = enforced.routes[0].steps[0].depart;
        assert_eq!(original_depart, enforced_depart); // depot departure is shift start
        assert!(
            enforced.routes[0].steps[1].arrive > solution.routes[0].steps[1].arrive,
            "first store should be reached later"
        );
    }

    #[test]
    fn a_wait_that_breaks_a_receiving_window_drops_the_store() {
        let mut problem = problem_with_two_stores((0, 600), (0, 600));
        // Store 0 must be served within the first hour of the horizon.
        problem.stores[0].receiving =
            vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(60))];
        let solution = solve_greedy(&problem);
        assert!(
            solution.unassigned.is_empty(),
            "fixture must start feasible: {solution:?}"
        );
        let mut provider = WaitProvider {
            base: StaticLegs::new(&problem),
            extra_wait: 100,
        };
        let (enforced, report) = enforce_curfews(&problem, &solution, &mut provider);
        assert_eq!(enforced.validate(&problem), Ok(()));
        assert!(
            enforced.unassigned.contains(&StoreId(0)),
            "store 0 should be dropped: {:?}",
            report.dropped
        );
        assert!(
            report
                .dropped
                .iter()
                .any(|drop| drop.store == StoreId(0)
                    && drop.kind == BreachKind::ReceivingWindowMissed),
            "expected a ReceivingWindowMissed breach for store 0: {:?}",
            report.dropped
        );
        assert!(
            !enforced.unassigned.contains(&StoreId(1)),
            "store 1 should survive the waits: {:?}",
            report.dropped
        );
    }

    #[test]
    fn driver_conflicts_check_overlap_rest_and_work_caps() {
        let problem = problem_with_two_stores((0, 600), (0, 600));
        // The pre-existing spans themselves respect the 420-minute rest
        // rule (they come from a valid plan in real use).
        let committed = vec![vec![(Minutes::from_minutes(0), Minutes::from_minutes(100))]];
        let remaining = vec![(
            1usize,
            DriverId(0),
            (Minutes::from_minutes(520), Minutes::from_minutes(620)),
        )];
        // Overlaps the committed span.
        assert!(driver_conflicts(
            &problem,
            &[DriverId(0)],
            (Minutes::from_minutes(50), Minutes::from_minutes(150)),
            &committed,
            &remaining,
        ));
        // Overlaps the remaining span.
        assert!(driver_conflicts(
            &problem,
            &[DriverId(0)],
            (Minutes::from_minutes(550), Minutes::from_minutes(650)),
            &committed,
            &remaining,
        ));
        // No overlap, but only 50 minutes of rest after the committed span
        // (Standard Hours require 420).
        assert!(driver_conflicts(
            &problem,
            &[DriverId(0)],
            (Minutes::from_minutes(150), Minutes::from_minutes(200)),
            &committed,
            &remaining,
        ));
        // Clear of both with enough rest: 940 minutes after the first span
        // and exactly the 420-minute minimum after the second.
        assert!(!driver_conflicts(
            &problem,
            &[DriverId(0)],
            (Minutes::from_minutes(1040), Minutes::from_minutes(1140)),
            &committed,
            &remaining,
        ));
        // A single span over the 12-hour daily cap conflicts even alone.
        assert!(driver_conflicts(
            &problem,
            &[DriverId(0)],
            (Minutes::ZERO, Minutes::from_minutes(800)),
            &[Vec::new()],
            &[],
        ));
        // A different driver is unaffected (and unknown drivers cannot
        // conflict: the fixture defines only driver 0).
        let committed = vec![
            vec![(Minutes::ZERO, Minutes::from_minutes(100))],
            Vec::new(),
        ];
        assert!(!driver_conflicts(
            &problem,
            &[DriverId(1)],
            (Minutes::from_minutes(50), Minutes::from_minutes(150)),
            &committed,
            &remaining,
        ));
    }
}
