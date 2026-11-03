//! Deterministic greedy insertion heuristic (M0).
//!
//! Stores are inserted one at a time, in a fixed order, into the cheapest
//! feasible position of an existing route or into a fresh route with an
//! unused vehicle. Every candidate is fully re-scheduled so that feasibility
//! follows the rules in [`domain::Solution::validate`] exactly.
//!
//! # M0 driver model
//!
//! Each route is driven by exactly one driver using exactly one of that
//! driver's shifts: for a candidate sequence the solver tries drivers in id
//! order and each driver's shifts in declaration order, and picks the first
//! pair that schedules feasibly. The domain model has no vehicle/driver
//! pairing, so any driver may drive any vehicle. Overnight relays spanning
//! two shifts or two drivers are M5 work and are deliberately not supported.
//!
//! # Driver occupancy
//!
//! A driver is booked for a route's whole span (first departure to last
//! arrival) while routes are built; a candidate whose span would overlap an
//! existing booking of the same driver is rejected, mirroring the occupancy
//! rule in [`domain::Solution::validate`]. When a store is inserted into an
//! existing route, that route's own booking is exempt from the check.

use std::cmp::Reverse;

use domain::{
    CostModel, DriverId, Load, Minutes, Node, PlannedStop, Problem, RoutePlan, Shift, Solution,
    Store, StoreId, Vehicle, VehicleId,
};

use crate::matrix::{DistanceMatrix, travel_minutes};

/// A route sequence scheduled onto one (driver, shift) pair, with the derived
/// metrics the solution model expects.
#[derive(Debug)]
pub struct Scheduled {
    pub driver: DriverId,
    /// The driver's work span for this route: first departure to last arrival.
    pub span: (Minutes, Minutes),
    pub steps: Vec<PlannedStop>,
    pub distance_km: f64,
    pub duration: Minutes,
    pub cost_cents: i64,
}

/// A committed route: the vehicle's position in `Problem::vehicles` and its
/// current schedule.
#[derive(Debug)]
pub struct ActiveRoute {
    pub vehicle_index: usize,
    pub scheduled: Scheduled,
}

/// One feasible way of placing a store, with the tie-break key
/// `(incremental cost, vehicle id, insertion index)`.
#[derive(Debug)]
struct Insertion {
    key: (i64, VehicleId, usize),
    /// `Some` when a store is added to an existing route, `None` for a fresh
    /// route.
    route_index: Option<usize>,
    vehicle_index: usize,
    scheduled: Scheduled,
}

/// Empty booking list, used when a driver has no bookings yet.
const NO_SPANS: &[(Minutes, Minutes)] = &[];

/// True when two work spans overlap. Spans may touch (one ends exactly when
/// the next starts) but must not otherwise share any instant.
fn spans_overlap(a: (Minutes, Minutes), b: (Minutes, Minutes)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// Builds a plan for `problem` with the M0 greedy insertion heuristic.
///
/// Stores that no candidate can schedule feasibly are returned in
/// [`Solution::unassigned`].
pub(crate) fn plan(problem: &Problem) -> Solution {
    let matrix = DistanceMatrix::new(problem);
    let driver_indices = driver_order(problem);
    let mut routes: Vec<ActiveRoute> = Vec::new();
    let mut used = vec![false; problem.vehicles.len()];
    let mut driver_busy: Vec<Vec<(Minutes, Minutes)>> = vec![Vec::new(); problem.drivers.len()];
    let mut unassigned: Vec<StoreId> = Vec::new();

    for store_index in processing_order(problem) {
        match best_insertion(
            problem,
            &matrix,
            &driver_indices,
            &routes,
            &used,
            &driver_busy,
            store_index,
        ) {
            Some(insertion) => commit(&mut routes, &mut used, &mut driver_busy, insertion),
            None => unassigned.push(problem.stores[store_index].id),
        }
    }

    build_solution(problem, routes, unassigned)
}

/// The order in which stores are offered to the greedy: earliest receiving
/// window end first (a store with several windows is judged by its tightest
/// one), then largest total demand, then id. The id tie-break keeps the order
/// total and therefore deterministic.
fn processing_order(problem: &Problem) -> Vec<usize> {
    let mut order: Vec<usize> = (0..problem.stores.len()).collect();
    order.sort_by_key(|&index| {
        let store = &problem.stores[index];
        let deadline = store
            .receiving
            .iter()
            .map(|window| window.end.minutes())
            .min();
        (
            deadline.unwrap_or(i64::MAX),
            Reverse(store.demand.total()),
            store.id,
        )
    });
    order
}

/// Driver positions in id order, with the vector index as a stable tie-break.
fn driver_order(problem: &Problem) -> Vec<usize> {
    let mut order: Vec<usize> = (0..problem.drivers.len()).collect();
    order.sort_by_key(|&index| (problem.drivers[index].id, index));
    order
}

/// Evaluates every feasible placement of one store and returns the cheapest,
/// ties broken by `(cost, vehicle id, insertion index)`.
fn best_insertion(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    routes: &[ActiveRoute],
    used: &[bool],
    driver_busy: &[Vec<(Minutes, Minutes)>],
    store_index: usize,
) -> Option<Insertion> {
    let store = &problem.stores[store_index];
    let node = Node::Store(store.id);
    let mut best: Option<Insertion> = None;

    for (route_index, route) in routes.iter().enumerate() {
        let vehicle = &problem.vehicles[route.vehicle_index];
        // The route's own booking must not block its own growth.
        let exempt = Some((route.scheduled.driver.0 as usize, route.scheduled.span));
        let sequence: Vec<Node> = route.scheduled.steps.iter().map(|stop| stop.node).collect();
        // Interior positions only: the route must keep its home depot first
        // and last.
        for position in 1..sequence.len() {
            let mut candidate = sequence.clone();
            candidate.insert(position, node);
            let Some(scheduled) = schedule_route(
                problem,
                matrix,
                driver_indices,
                vehicle,
                &candidate,
                driver_busy,
                exempt,
            ) else {
                continue;
            };
            let incremental = scheduled
                .cost_cents
                .saturating_sub(route.scheduled.cost_cents);
            consider(
                &mut best,
                Insertion {
                    key: (incremental, vehicle.id, position),
                    route_index: Some(route_index),
                    vehicle_index: route.vehicle_index,
                    scheduled,
                },
            );
        }
    }

    for (vehicle_index, vehicle) in problem.vehicles.iter().enumerate() {
        if used[vehicle_index] {
            continue;
        }
        let home = Node::Depot(vehicle.depot);
        let candidate = [home, node, home];
        let Some(scheduled) = schedule_route(
            problem,
            matrix,
            driver_indices,
            vehicle,
            &candidate,
            driver_busy,
            None,
        ) else {
            continue;
        };
        consider(
            &mut best,
            Insertion {
                key: (scheduled.cost_cents, vehicle.id, 1),
                route_index: None,
                vehicle_index,
                scheduled,
            },
        );
    }

    best
}

/// Keeps the candidate with the smaller tie-break key.
fn consider(best: &mut Option<Insertion>, candidate: Insertion) {
    if best
        .as_ref()
        .is_none_or(|current| candidate.key < current.key)
    {
        *best = Some(candidate);
    }
}

/// Writes an evaluated insertion back into the route list and moves the
/// driver's booking from the replaced span to the new one.
fn commit(
    routes: &mut Vec<ActiveRoute>,
    used: &mut [bool],
    driver_busy: &mut [Vec<(Minutes, Minutes)>],
    insertion: Insertion,
) {
    let Insertion {
        route_index,
        vehicle_index,
        scheduled,
        ..
    } = insertion;
    let new_driver = scheduled.driver.0 as usize;
    let new_span = scheduled.span;

    match route_index {
        Some(index) => {
            let old_driver = routes[index].scheduled.driver.0 as usize;
            let old_span = routes[index].scheduled.span;
            if let Some(spans) = driver_busy.get_mut(old_driver)
                && let Some(position) = spans.iter().position(|&span| span == old_span)
            {
                spans.remove(position);
            }
            routes[index] = ActiveRoute {
                vehicle_index,
                scheduled,
            };
        }
        None => {
            used[vehicle_index] = true;
            routes.push(ActiveRoute {
                vehicle_index,
                scheduled,
            });
        }
    }

    if let Some(spans) = driver_busy.get_mut(new_driver) {
        spans.push(new_span);
    }
}

/// Tries each driver, in id order, and each of that driver's shifts, in
/// declaration order, and returns the first schedule that is feasible and
/// does not overlap an existing booking of the same driver.
pub fn schedule_route(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    vehicle: &Vehicle,
    sequence: &[Node],
    driver_busy: &[Vec<(Minutes, Minutes)>],
    exempt: Option<(usize, (Minutes, Minutes))>,
) -> Option<Scheduled> {
    for &driver_index in driver_indices {
        let driver = problem.drivers.get(driver_index)?;
        let busy = driver_busy
            .get(driver.id.0 as usize)
            .map_or(NO_SPANS, Vec::as_slice);
        for &shift in &driver.shifts {
            let Some(scheduled) =
                schedule_shift(problem, matrix, vehicle, driver.id, shift, sequence)
            else {
                continue;
            };
            let blocked = busy.iter().any(|&span| {
                spans_overlap(span, scheduled.span) && Some((driver.id.0 as usize, span)) != exempt
            });
            if !blocked {
                return Some(scheduled);
            }
        }
    }
    None
}

/// Schedules `sequence` (home depot, stores, home depot) onto one shift.
///
/// The truck leaves the home depot at `shift.start`; a store's service starts
/// at `max(previous departure + travel, window start)`, so early arrivals wait
/// and the wait is folded into the preceding leg (the interpretation
/// [`Solution::validate`] documents). The route is feasible only when the
/// whole run fits the single shift, every cumulative load fits the vehicle
/// and the spanned time respects `max_duration`.
fn schedule_shift(
    problem: &Problem,
    matrix: &DistanceMatrix,
    vehicle: &Vehicle,
    driver: DriverId,
    shift: Shift,
    sequence: &[Node],
) -> Option<Scheduled> {
    let (&home, rest) = sequence.split_first()?;
    if !matches!(home, Node::Depot(_)) {
        return None;
    }
    let (&last, store_nodes) = rest.split_last()?;
    if last != home || store_nodes.is_empty() {
        return None;
    }
    if store_nodes
        .iter()
        .any(|node| !matches!(node, Node::Store(_)))
    {
        return None;
    }

    let mut steps = Vec::with_capacity(sequence.len());
    steps.push(PlannedStop {
        node: home,
        arrive: shift.start,
        depart: shift.start,
    });

    let mut time = shift.start;
    let mut previous = home;
    let mut load = Load::ZERO;
    let mut distance_km = 0.0;

    for &node in store_nodes {
        let Node::Store(store_id) = node else {
            return None;
        };
        let store = problem.store(store_id)?;
        let leg_km = matrix.distance_km(previous, node)?;
        distance_km += leg_km;

        let arrival = time + travel_minutes(leg_km, vehicle.avg_speed_kmh);
        let service_start = next_service_start(store, arrival)?;
        let depart = service_start + store.service;

        load = add_load(load, store.demand)?;
        if !load.fits_within(vehicle.capacity) {
            return None;
        }

        steps.push(PlannedStop {
            node,
            arrive: service_start,
            depart,
        });
        time = depart;
        previous = node;
    }

    // Return leg to the home depot.
    let leg_km = matrix.distance_km(previous, home)?;
    distance_km += leg_km;
    let arrival = time + travel_minutes(leg_km, vehicle.avg_speed_kmh);
    if arrival > shift.end {
        return None;
    }
    steps.push(PlannedStop {
        node: home,
        arrive: arrival,
        depart: arrival,
    });

    let duration = arrival - shift.start;
    if duration > vehicle.max_duration {
        return None;
    }

    Some(Scheduled {
        driver,
        span: (shift.start, arrival),
        steps,
        distance_km,
        duration,
        cost_cents: route_cost(problem.cost, distance_km, duration),
    })
}

/// Simplified scheduling for construction heuristics (no occupancy checks).
/// Tries each driver, in id order, and each of that driver's shifts, in
/// declaration order, and returns the first feasible schedule.
#[allow(dead_code)]
pub fn schedule_route_simple(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    vehicle: &Vehicle,
    sequence: &[Node],
) -> Option<Scheduled> {
    for &driver_index in driver_indices {
        let driver = problem.drivers.get(driver_index)?;
        for &shift in &driver.shifts {
            if let Some(scheduled) =
                schedule_shift(problem, matrix, vehicle, driver.id, shift, sequence)
            {
                return Some(scheduled);
            }
        }
    }
    None
}

/// The earliest minute a store's service can start after arriving at
/// `arrival`, or `None` when no receiving window can hold the full service.
///
/// With several windows the earliest feasible one (by start) wins: it also
/// yields the earliest departure, which keeps every downstream timestamp as
/// small as possible.
fn next_service_start(store: &Store, arrival: Minutes) -> Option<Minutes> {
    store
        .receiving
        .iter()
        .filter_map(|window| {
            let start = arrival.max(window.start);
            (start + store.service <= window.end).then_some(start)
        })
        .min()
}

/// Adds two loads, or `None` on overflow (which no capacity can absorb).
fn add_load(load: Load, demand: Load) -> Option<Load> {
    Some(Load {
        ambient: load.ambient.checked_add(demand.ambient)?,
        chilled: load.chilled.checked_add(demand.chilled)?,
        frozen: load.frozen.checked_add(demand.frozen)?,
    })
}

/// Route cost in cents: fixed charge plus distance and duration components,
/// each rounded to the nearest cent.
fn route_cost(model: CostModel, distance_km: f64, duration: Minutes) -> i64 {
    let distance_cents = (distance_km * model.per_km_cents as f64).round() as i64;
    let duration_cents = (duration.hours() * model.per_hour_cents as f64).round() as i64;
    model
        .fixed_per_route_cents
        .saturating_add(distance_cents)
        .saturating_add(duration_cents)
}

/// Converts the internal routes into the public [`Solution`], deriving the
/// aggregate totals from the routes themselves.
fn build_solution(
    problem: &Problem,
    routes: Vec<ActiveRoute>,
    mut unassigned: Vec<StoreId>,
) -> Solution {
    let routes: Vec<RoutePlan> = routes
        .into_iter()
        .map(|route| {
            let vehicle = &problem.vehicles[route.vehicle_index];
            RoutePlan {
                vehicle: vehicle.id,
                drivers: vec![route.scheduled.driver],
                steps: route.scheduled.steps,
                distance_km: route.scheduled.distance_km,
                duration: route.scheduled.duration,
                cost_cents: route.scheduled.cost_cents,
            }
        })
        .collect();

    let total_distance_km = routes.iter().map(|route| route.distance_km).sum();
    let total_duration = routes.iter().map(|route| route.duration.minutes()).sum();
    let total_cost_cents = routes.iter().map(|route| route.cost_cents).sum();

    unassigned.sort_unstable();
    Solution {
        routes,
        unassigned,
        total_distance_km,
        total_duration: Minutes::from_minutes(total_duration),
        total_cost_cents,
    }
}

#[cfg(test)]
mod tests {
    use super::route_cost;
    use domain::{CostModel, Minutes};

    #[test]
    fn route_cost_matches_hand_computed_value() {
        let model = CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 90,
            per_hour_cents: 4_500,
            late_per_min_cents: 0,
        };
        // 100 km at 90c = 9,000; 90 minutes at 45.00/h = 6,750; fixed 15,000.
        assert_eq!(route_cost(model, 100.0, Minutes::from_minutes(90)), 30_750);
    }
}
