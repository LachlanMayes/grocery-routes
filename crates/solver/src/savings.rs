//! Clarke–Wright savings algorithm for initial route construction (M1).
//!
//! Builds a set of routes by greedily merging routes that share a depot,
//! ordered by the distance savings of the merge. This typically produces
//! better starting solutions than sequential insertion, giving the local
//! search a stronger baseline.

use std::cmp::Reverse;

use domain::{
    DepotId, Load, Minutes, Node, Problem, RoutePlan, Shift, Solution, StoreId, VehicleId,
};

use crate::greedy::{ActiveRoute, Scheduled, plan_with_matrix, schedule_route_simple};
use crate::matrix::{DistanceMatrix, travel_minutes};
use domain::PlannedStop;

/// Runs Clarke–Wright savings and then polishes with local search.
pub(crate) fn savings_plus_ls(problem: &Problem) -> Solution {
    let matrix = DistanceMatrix::new(problem);
    let driver_indices = driver_order(problem);

    // Track driver busy spans to prevent double-booking.
    let mut driver_busy: Vec<Vec<(Minutes, Minutes)>> = vec![Vec::new(); problem.drivers.len()];

    // Phase 1: build initial routes by savings
    let mut routes = build_savings_routes(problem, &matrix, &driver_indices, &mut driver_busy);

    // Phase 2: local search (2-opt, relocate, exchange, cross-exchange)
    local_search(
        problem,
        &matrix,
        &driver_indices,
        &mut routes,
        &mut driver_busy,
    );

    build_solution(problem, &matrix, routes)
}

/// Builds initial routes using Clarke–Wright savings.
fn build_savings_routes(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    driver_busy: &mut [Vec<(Minutes, Minutes)>],
) -> Vec<ActiveRoute> {
    let vehicle_indices: Vec<usize> = (0..problem.vehicles.len()).collect();

    // Each vehicle starts with its own route: depot -> store -> depot.
    // We'll merge them greedily.
    let mut active = init_singleton_routes(
        problem,
        &matrix,
        driver_indices,
        &vehicle_indices,
        driver_busy,
    );

    // Compute all pairwise savings using the first depot (single-depot CW).
    // For multi-depot, we only merge routes that originate from the same depot.
    let primary_depot = problem.depots.first().map(|d| d.id).unwrap_or(DepotId(0));
    let mut savings = Vec::new();
    for i in 0..problem.stores.len() {
        for j in (i + 1)..problem.stores.len() {
            let si = &problem.stores[i];
            let sj = &problem.stores[j];
            if si.demand.total() == 0 || sj.demand.total() == 0 {
                continue;
            }
            let d_ij = matrix
                .distance_km(Node::Store(si.id), Node::Store(sj.id))
                .unwrap_or(f64::INFINITY);
            let d_0i = matrix
                .distance_km(Node::Depot(primary_depot), Node::Store(si.id))
                .unwrap_or(f64::INFINITY);
            let d_0j = matrix
                .distance_km(Node::Depot(primary_depot), Node::Store(sj.id))
                .unwrap_or(f64::INFINITY);
            if d_0i.is_infinite() || d_0j.is_infinite() {
                continue;
            }
            let saving = d_0i + d_0j - d_ij;
            if saving > 0.0 {
                savings.push((Reverse(saving as i64), i, j));
            }
        }
    }
    savings.sort_unstable();

    // Union-Find to track which stores are in which route (by vehicle index).
    let mut uf = UnionFind::new(problem.stores.len());

    for (_, i, j) in savings {
        let ri = uf.find(i);
        let rj = uf.find(j);
        if ri == rj {
            continue;
        }

        // Find the active route containing ri and rj.
        let (route_i_opt, pos_i) = find_route_and_pos(&active, ri);
        let (route_j_opt, pos_j) = find_route_and_pos(&active, rj);

        // Must be different routes, both at ends (so we can concatenate).
        let (Some(route_i_idx), Some(route_j_idx)) = (route_i_opt, route_j_opt) else {
            continue;
        };
        if route_i_idx == route_j_idx {
            continue;
        }
        if !is_at_end(pos_i, &active[route_i_idx]) || !is_at_end(pos_j, &active[route_j_idx]) {
            continue;
        }

        // Try to merge: concatenate the two routes in the orientation that fits.
        if let Some(merged) = try_merge_routes(
            problem,
            &matrix,
            driver_indices,
            &active,
            driver_busy,
            route_i_idx,
            pos_i,
            route_j_idx,
            pos_j,
        ) {
            // Remove old driver bookings for the two routes being merged.
            let old_driver_i = active[route_i_idx].scheduled.driver.0 as usize;
            let old_span_i = active[route_i_idx].scheduled.span;
            if let Some(spans) = driver_busy.get_mut(old_driver_i)
                && let Some(pos) = spans.iter().position(|&s| s == old_span_i)
            {
                spans.remove(pos);
            }
            let old_driver_j = active[route_j_idx].scheduled.driver.0 as usize;
            let old_span_j = active[route_j_idx].scheduled.span;
            if let Some(spans) = driver_busy.get_mut(old_driver_j)
                && let Some(pos) = spans.iter().position(|&s| s == old_span_j)
            {
                spans.remove(pos);
            }

            // Add the new merged route's booking.
            let new_driver = merged.driver.0 as usize;
            let new_span = merged.span;
            if let Some(spans) = driver_busy.get_mut(new_driver) {
                spans.push(new_span);
            }

            // Replace route_i with merged, remove route_j.
            let vehicle_index = active[route_i_idx].vehicle_index;
            active[route_i_idx] = ActiveRoute {
                vehicle_index,
                scheduled: merged,
            };
            active.remove(route_j_idx);
            uf.union(i, j);
        }
    }

    active
}

fn init_singleton_routes(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    vehicle_indices: &[usize],
    driver_busy: &mut [Vec<(Minutes, Minutes)>],
) -> Vec<ActiveRoute> {
    let mut active = Vec::new();
    let mut used = vec![false; problem.vehicles.len()];
    let mut assigned_stores = std::collections::HashSet::new();

    // One store per vehicle initially (capacity permitting).
    for &v_idx in vehicle_indices {
        let vehicle = &problem.vehicles[v_idx];
        let home = Node::Depot(vehicle.depot);

        // Find the first store this vehicle can serve alone.
        for s_idx in 0..problem.stores.len() {
            let store = &problem.stores[s_idx];
            if !store.demand.fits_within(vehicle.capacity) {
                continue;
            }
            if assigned_stores.contains(&store.id) {
                continue;
            }
            let candidate = [home, Node::Store(store.id), home];
            if let Some(scheduled) = schedule_route_simple(
                problem,
                matrix,
                driver_indices,
                vehicle,
                &candidate,
                driver_busy,
                None,
            ) {
                // Add driver booking.
                let driver_idx = scheduled.driver.0 as usize;
                let span = scheduled.span;
                if let Some(spans) = driver_busy.get_mut(driver_idx) {
                    spans.push(span);
                }
                used[v_idx] = true;
                assigned_stores.insert(store.id);
                active.push(ActiveRoute {
                    vehicle_index: v_idx,
                    scheduled,
                });
                break;
            }
        }
    }

    // Remaining vehicles with no store: keep as empty (will be dropped later).
    for &v_idx in vehicle_indices {
        if !used[v_idx] {
            let vehicle = &problem.vehicles[v_idx];
            let home = Node::Depot(vehicle.depot);
            // Try any store (capacity checked in schedule_route)
            for s_idx in 0..problem.stores.len() {
                let store = &problem.stores[s_idx];
                if assigned_stores.contains(&store.id) {
                    continue;
                }
                let candidate = [home, Node::Store(store.id), home];
                if let Some(scheduled) = schedule_route_simple(
                    problem,
                    matrix,
                    driver_indices,
                    vehicle,
                    &candidate,
                    driver_busy,
                    None,
                ) {
                    // Add driver booking.
                    let driver_idx = scheduled.driver.0 as usize;
                    let span = scheduled.span;
                    if let Some(spans) = driver_busy.get_mut(driver_idx) {
                        spans.push(span);
                    }
                    used[v_idx] = true;
                    assigned_stores.insert(store.id);
                    active.push(ActiveRoute {
                        vehicle_index: v_idx,
                        scheduled,
                    });
                    break;
                }
            }
        }
    }

    active
}

fn find_route_and_pos(active: &[ActiveRoute], store_idx: usize) -> (Option<usize>, usize) {
    let target = StoreId(store_idx as u32);
    for (r_idx, route) in active.iter().enumerate() {
        for (pos, step) in route.scheduled.steps.iter().enumerate() {
            if matches!(step.node, Node::Store(id) if id == target) {
                return (Some(r_idx), pos);
            }
        }
    }
    (None, 0)
}

fn is_at_end(pos: usize, route: &ActiveRoute) -> bool {
    // Route steps: depot, store..., depot. Interior positions are 1..len-2.
    pos == 1 || pos == route.scheduled.steps.len() - 2
}

fn try_merge_routes(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    active: &[ActiveRoute],
    driver_busy: &[Vec<(Minutes, Minutes)>],
    ri: usize,
    pos_i: usize,
    rj: usize,
    pos_j: usize,
) -> Option<Scheduled> {
    let route_i = &active[ri];
    let route_j = &active[rj];
    let vehicle = &problem.vehicles[route_i.vehicle_index];

    // Extract the two store sequences (interior nodes).
    let seq_i: Vec<Node> = route_i
        .scheduled
        .steps
        .iter()
        .skip(1)
        .take(route_i.scheduled.steps.len() - 2)
        .map(|s| s.node)
        .collect();
    let seq_j: Vec<Node> = route_j
        .scheduled
        .steps
        .iter()
        .skip(1)
        .take(route_j.scheduled.steps.len() - 2)
        .map(|s| s.node)
        .collect();

    // Exempt both routes' current bookings since we're merging them.
    let exempt_i = Some((route_i.scheduled.driver.0 as usize, route_i.scheduled.span));
    let exempt_j = Some((route_j.scheduled.driver.0 as usize, route_j.scheduled.span));

    // Four orientations: i→j, j→i, i_rev→j, j_rev→i
    let orientations = [
        (seq_i.clone(), seq_j.clone()),
        (seq_j.clone(), seq_i.clone()),
        (seq_i.iter().rev().cloned().collect(), seq_j.clone()),
        (seq_j.iter().rev().cloned().collect(), seq_i.clone()),
    ];

    for (first, second) in orientations {
        let mut combined = vec![Node::Depot(vehicle.depot)];
        combined.extend(first);
        combined.extend(second);
        combined.push(Node::Depot(vehicle.depot));

        // Try with both exemptions - a merged route could be assigned to either driver.
        // We need to check that the new schedule doesn't conflict with other drivers'
        // bookings, but it can replace either route_i or route_j's booking.
        if let Some(scheduled) = schedule_route_simple(
            problem,
            matrix,
            driver_indices,
            vehicle,
            &combined,
            driver_busy,
            exempt_i,
        ) {
            return Some(scheduled);
        }
        if let Some(scheduled) = schedule_route_simple(
            problem,
            matrix,
            driver_indices,
            vehicle,
            &combined,
            driver_busy,
            exempt_j,
        ) {
            return Some(scheduled);
        }
    }
    None
}

/// Union-Find for savings merges.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }
    fn union(&mut self, x: usize, y: usize) {
        let rx = self.find(x);
        let ry = self.find(y);
        if rx != ry {
            self.parent[ry] = rx;
        }
    }
}

/// Re-export the driver order helper.
fn driver_order(problem: &Problem) -> Vec<usize> {
    let mut order: Vec<usize> = (0..problem.drivers.len()).collect();
    order.sort_by_key(|&index| (problem.drivers[index].id, index));
    order
}

/// Local search improvements: 2-opt, relocate, exchange, cross-exchange.
fn local_search(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    routes: &mut Vec<ActiveRoute>,
    driver_busy: &mut [Vec<(Minutes, Minutes)>],
) {
    let mut improved = true;
    while improved {
        improved = false;
        // 2-opt within each route
        for route in routes.iter_mut() {
            if two_opt(problem, matrix, driver_indices, route, driver_busy) {
                improved = true;
            }
        }
        // Relocate: move a store from one route to another
        if relocate(problem, matrix, driver_indices, routes, driver_busy) {
            improved = true;
            continue;
        }
        // Exchange: swap a store between two routes
        if exchange(problem, matrix, driver_indices, routes, driver_busy) {
            improved = true;
            continue;
        }
        // Cross-exchange: swap a subsequence
        if cross_exchange(problem, matrix, driver_indices, routes, driver_busy) {
            improved = true;
        }
    }
}

fn two_opt(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    route: &mut ActiveRoute,
    driver_busy: &mut [Vec<(Minutes, Minutes)>],
) -> bool {
    let vehicle = &problem.vehicles[route.vehicle_index];
    let steps = &route.scheduled.steps;
    if steps.len() < 5 {
        return false; // depot + 2 stores + depot
    }
    let nodes: Vec<Node> = steps.iter().map(|s| s.node).collect();
    let mut best_nodes = nodes.clone();
    let mut best_cost = route.scheduled.cost_cents;

    // Exempt this route's current booking since we're modifying it.
    let exempt = Some((route.scheduled.driver.0 as usize, route.scheduled.span));

    // Try all pairs of edges to reverse (i, j) with i < j, not adjacent.
    for i in 1..nodes.len() - 2 {
        for j in i + 1..nodes.len() - 1 {
            if j == i + 1 {
                continue;
            }
            let mut candidate = nodes.clone();
            candidate[i..=j].reverse();
            if let Some(scheduled) = schedule_route_simple(
                problem,
                matrix,
                driver_indices,
                vehicle,
                &candidate,
                driver_busy,
                exempt,
            ) {
                if scheduled.cost_cents < best_cost {
                    best_cost = scheduled.cost_cents;
                    best_nodes = candidate;
                }
            }
        }
    }
    if best_cost < route.scheduled.cost_cents {
        if let Some(scheduled) = schedule_route_simple(
            problem,
            matrix,
            driver_indices,
            vehicle,
            &best_nodes,
            driver_busy,
            exempt,
        ) {
            // Update driver busy: remove old span, add new span.
            let old_driver = route.scheduled.driver.0 as usize;
            let old_span = route.scheduled.span;
            if let Some(spans) = driver_busy.get_mut(old_driver)
                && let Some(pos) = spans.iter().position(|&s| s == old_span)
            {
                spans.remove(pos);
            }
            let new_driver = scheduled.driver.0 as usize;
            let new_span = scheduled.span;
            if let Some(spans) = driver_busy.get_mut(new_driver) {
                spans.push(new_span);
            }
            route.scheduled = scheduled;
            return true;
        }
    }
    false
}

fn relocate(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    routes: &mut Vec<ActiveRoute>,
    driver_busy: &mut [Vec<(Minutes, Minutes)>],
) -> bool {
    // For each route, try removing each interior store and inserting it into another route.
    for src_idx in 0..routes.len() {
        let src_vehicle = &problem.vehicles[routes[src_idx].vehicle_index];
        let src_steps = routes[src_idx].scheduled.steps.clone();
        if src_steps.len() <= 3 {
            continue; // only depot → store → depot
        }
        for store_pos in 1..src_steps.len() - 1 {
            let Node::Store(store_id) = src_steps[store_pos].node else {
                continue;
            };
            // Remove from src
            let mut new_src_nodes: Vec<Node> =
                src_steps.iter().map(|s: &PlannedStop| s.node).collect();
            new_src_nodes.remove(store_pos);
            if new_src_nodes.len() < 3 {
                continue;
            }
            // Try to re-schedule src without this store (exempt its current booking)
            let src_exempt = Some((
                routes[src_idx].scheduled.driver.0 as usize,
                routes[src_idx].scheduled.span,
            ));
            let Some(src_scheduled) = schedule_route_simple(
                problem,
                matrix,
                driver_indices,
                src_vehicle,
                &new_src_nodes,
                driver_busy,
                src_exempt,
            ) else {
                continue;
            };

            // Try to insert into each other route
            for dst_idx in 0..routes.len() {
                if dst_idx == src_idx {
                    continue;
                }
                let dst_vehicle = &problem.vehicles[routes[dst_idx].vehicle_index];
                let dst_steps = &routes[dst_idx].scheduled.steps;
                for insert_pos in 1..dst_steps.len() - 1 {
                    let mut new_dst_nodes: Vec<Node> = dst_steps.iter().map(|s| s.node).collect();
                    new_dst_nodes.insert(insert_pos, Node::Store(store_id));
                    // Exempt destination route's current booking
                    let dst_exempt = Some((
                        routes[dst_idx].scheduled.driver.0 as usize,
                        routes[dst_idx].scheduled.span,
                    ));
                    if let Some(dst_scheduled) = schedule_route_simple(
                        problem,
                        matrix,
                        driver_indices,
                        dst_vehicle,
                        &new_dst_nodes,
                        driver_busy,
                        dst_exempt,
                    ) {
                        let delta = dst_scheduled.cost_cents + src_scheduled.cost_cents
                            - routes[src_idx].scheduled.cost_cents
                            - routes[dst_idx].scheduled.cost_cents;
                        if delta < 0 {
                            // Update driver busy for src route
                            let old_src_driver = routes[src_idx].scheduled.driver.0 as usize;
                            let old_src_span = routes[src_idx].scheduled.span;
                            if let Some(spans) = driver_busy.get_mut(old_src_driver)
                                && let Some(pos) = spans.iter().position(|&s| s == old_src_span)
                            {
                                spans.remove(pos);
                            }
                            let new_src_driver = src_scheduled.driver.0 as usize;
                            let new_src_span = src_scheduled.span;
                            if let Some(spans) = driver_busy.get_mut(new_src_driver) {
                                spans.push(new_src_span);
                            }

                            // Update driver busy for dst route
                            let old_dst_driver = routes[dst_idx].scheduled.driver.0 as usize;
                            let old_dst_span = routes[dst_idx].scheduled.span;
                            if let Some(spans) = driver_busy.get_mut(old_dst_driver)
                                && let Some(pos) = spans.iter().position(|&s| s == old_dst_span)
                            {
                                spans.remove(pos);
                            }
                            let new_dst_driver = dst_scheduled.driver.0 as usize;
                            let new_dst_span = dst_scheduled.span;
                            if let Some(spans) = driver_busy.get_mut(new_dst_driver) {
                                spans.push(new_dst_span);
                            }

                            routes[src_idx].scheduled = src_scheduled;
                            routes[dst_idx].scheduled = dst_scheduled;
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn exchange(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    routes: &mut Vec<ActiveRoute>,
    driver_busy: &mut [Vec<(Minutes, Minutes)>],
) -> bool {
    for i in 0..routes.len() {
        for j in (i + 1)..routes.len() {
            let vi = &problem.vehicles[routes[i].vehicle_index];
            let vj = &problem.vehicles[routes[j].vehicle_index];
            let steps_i = &routes[i].scheduled.steps;
            let steps_j = &routes[j].scheduled.steps;
            if steps_i.len() <= 3 || steps_j.len() <= 3 {
                continue;
            }
            for pos_i in 1..steps_i.len() - 1 {
                let Node::Store(si) = steps_i[pos_i].node else {
                    continue;
                };
                for pos_j in 1..steps_j.len() - 1 {
                    let Node::Store(sj) = steps_j[pos_j].node else {
                        continue;
                    };
                    // Swap si and sj
                    let mut new_i: Vec<Node> = steps_i.iter().map(|s| s.node).collect();
                    let mut new_j: Vec<Node> = steps_j.iter().map(|s| s.node).collect();
                    new_i[pos_i] = Node::Store(sj);
                    new_j[pos_j] = Node::Store(si);
                    // Exempt both routes' current bookings
                    let exempt_i = Some((
                        routes[i].scheduled.driver.0 as usize,
                        routes[i].scheduled.span,
                    ));
                    let exempt_j = Some((
                        routes[j].scheduled.driver.0 as usize,
                        routes[j].scheduled.span,
                    ));
                    if let (Some(si_sched), Some(sj_sched)) = (
                        schedule_route_simple(
                            problem,
                            matrix,
                            driver_indices,
                            vi,
                            &new_i,
                            driver_busy,
                            exempt_i,
                        ),
                        schedule_route_simple(
                            problem,
                            matrix,
                            driver_indices,
                            vj,
                            &new_j,
                            driver_busy,
                            exempt_j,
                        ),
                    ) {
                        let delta = si_sched.cost_cents + sj_sched.cost_cents
                            - routes[i].scheduled.cost_cents
                            - routes[j].scheduled.cost_cents;
                        if delta < 0 {
                            // Update driver busy for route i
                            let old_driver_i = routes[i].scheduled.driver.0 as usize;
                            let old_span_i = routes[i].scheduled.span;
                            if let Some(spans) = driver_busy.get_mut(old_driver_i)
                                && let Some(pos) = spans.iter().position(|&s| s == old_span_i)
                            {
                                spans.remove(pos);
                            }
                            let new_driver_i = si_sched.driver.0 as usize;
                            let new_span_i = si_sched.span;
                            if let Some(spans) = driver_busy.get_mut(new_driver_i) {
                                spans.push(new_span_i);
                            }

                            // Update driver busy for route j
                            let old_driver_j = routes[j].scheduled.driver.0 as usize;
                            let old_span_j = routes[j].scheduled.span;
                            if let Some(spans) = driver_busy.get_mut(old_driver_j)
                                && let Some(pos) = spans.iter().position(|&s| s == old_span_j)
                            {
                                spans.remove(pos);
                            }
                            let new_driver_j = sj_sched.driver.0 as usize;
                            let new_span_j = sj_sched.span;
                            if let Some(spans) = driver_busy.get_mut(new_driver_j) {
                                spans.push(new_span_j);
                            }

                            routes[i].scheduled = si_sched;
                            routes[j].scheduled = sj_sched;
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn cross_exchange(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    routes: &mut Vec<ActiveRoute>,
    driver_busy: &mut [Vec<(Minutes, Minutes)>],
) -> bool {
    // Swap contiguous subsequences (length 1..2 for now) between two routes.
    for i in 0..routes.len() {
        for j in (i + 1)..routes.len() {
            let vi = &problem.vehicles[routes[i].vehicle_index];
            let vj = &problem.vehicles[routes[j].vehicle_index];
            let steps_i = &routes[i].scheduled.steps;
            let steps_j = &routes[j].scheduled.steps;
            if steps_i.len() <= 4 || steps_j.len() <= 4 {
                continue;
            }
            // Length 1 and 2 subsequences
            for len in 1..=2 {
                for start_i in 1..=steps_i.len() - 1 - len {
                    let sub_i: Vec<Node> = steps_i[start_i..start_i + len]
                        .iter()
                        .map(|s| s.node)
                        .collect();
                    for start_j in 1..=steps_j.len() - 1 - len {
                        let sub_j: Vec<Node> = steps_j[start_j..start_j + len]
                            .iter()
                            .map(|s| s.node)
                            .collect();
                        let mut new_i: Vec<Node> = steps_i.iter().map(|s| s.node).collect();
                        let mut new_j: Vec<Node> = steps_j.iter().map(|s| s.node).collect();
                        new_i.splice(start_i..start_i + len, sub_j.clone());
                        new_j.splice(start_j..start_j + len, sub_i.clone());
                        // Exempt both routes' current bookings
                        let exempt_i = Some((
                            routes[i].scheduled.driver.0 as usize,
                            routes[i].scheduled.span,
                        ));
                        let exempt_j = Some((
                            routes[j].scheduled.driver.0 as usize,
                            routes[j].scheduled.span,
                        ));
                        if let (Some(si_sched), Some(sj_sched)) = (
                            schedule_route_simple(
                                problem,
                                matrix,
                                driver_indices,
                                vi,
                                &new_i,
                                driver_busy,
                                exempt_i,
                            ),
                            schedule_route_simple(
                                problem,
                                matrix,
                                driver_indices,
                                vj,
                                &new_j,
                                driver_busy,
                                exempt_j,
                            ),
                        ) {
                            let delta = si_sched.cost_cents + sj_sched.cost_cents
                                - routes[i].scheduled.cost_cents
                                - routes[j].scheduled.cost_cents;
                            if delta < 0 {
                                // Update driver busy for route i
                                let old_driver_i = routes[i].scheduled.driver.0 as usize;
                                let old_span_i = routes[i].scheduled.span;
                                if let Some(spans) = driver_busy.get_mut(old_driver_i)
                                    && let Some(pos) = spans.iter().position(|&s| s == old_span_i)
                                {
                                    spans.remove(pos);
                                }
                                let new_driver_i = si_sched.driver.0 as usize;
                                let new_span_i = si_sched.span;
                                if let Some(spans) = driver_busy.get_mut(new_driver_i) {
                                    spans.push(new_span_i);
                                }

                                // Update driver busy for route j
                                let old_driver_j = routes[j].scheduled.driver.0 as usize;
                                let old_span_j = routes[j].scheduled.span;
                                if let Some(spans) = driver_busy.get_mut(old_driver_j)
                                    && let Some(pos) = spans.iter().position(|&s| s == old_span_j)
                                {
                                    spans.remove(pos);
                                }
                                let new_driver_j = sj_sched.driver.0 as usize;
                                let new_span_j = sj_sched.span;
                                if let Some(spans) = driver_busy.get_mut(new_driver_j) {
                                    spans.push(new_span_j);
                                }

                                routes[i].scheduled = si_sched;
                                routes[j].scheduled = sj_sched;
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

fn build_solution(
    problem: &Problem,
    matrix: &DistanceMatrix,
    routes: Vec<ActiveRoute>,
) -> Solution {
    let mut served_stores = std::collections::HashSet::new();
    let routes: Vec<RoutePlan> = routes
        .into_iter()
        .map(|route| {
            let vehicle = &problem.vehicles[route.vehicle_index];
            for step in &route.scheduled.steps {
                if let Node::Store(id) = step.node {
                    served_stores.insert(id);
                }
            }
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

    let total_distance_km = routes.iter().map(|r| r.distance_km).sum();
    let total_duration = routes.iter().map(|r| r.duration.minutes()).sum();
    let total_cost_cents = routes.iter().map(|r| r.cost_cents).sum();

    let mut unassigned: Vec<StoreId> = problem
        .stores
        .iter()
        .filter(|s| !served_stores.contains(&s.id))
        .map(|s| s.id)
        .collect();
    unassigned.sort_unstable();

    let solution = Solution {
        routes,
        unassigned,
        total_distance_km,
        total_duration: Minutes::from_minutes(total_duration),
        total_cost_cents,
    };

    if let Err(violations) = solution.validate(problem) {
        eprintln!("Savings produced invalid solution, falling back to greedy:");
        for v in &violations {
            eprintln!("  - {v}");
        }
        return plan_with_matrix(problem, matrix.clone());
    }
    solution
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        CostModel, Depot, DepotId, Driver, DriverId, LatLon, Load, Minutes, Problem, Shift, Store,
        StoreId, TimeWindow, Vehicle, VehicleId,
    };

    fn small_problem() -> Problem {
        let depot = Depot {
            id: DepotId(0),
            name: "Depot".into(),
            location: LatLon::new(-33.87, 151.21),
        };

        let stores = vec![
            Store {
                id: StoreId(0),
                chain: domain::Chain::Coles,
                name: "Store 1".into(),
                location: LatLon::new(-33.92, 151.23),
                demand: Load::new(10, 0, 0),
                service: Minutes::from_minutes(5),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(480),
                    Minutes::from_minutes(720),
                )],
            },
            Store {
                id: StoreId(1),
                chain: domain::Chain::Coles,
                name: "Store 2".into(),
                location: LatLon::new(-33.80, 151.15),
                demand: Load::new(10, 0, 0),
                service: Minutes::from_minutes(5),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(300),
                    Minutes::from_minutes(600),
                )],
            },
            Store {
                id: StoreId(2),
                chain: domain::Chain::Coles,
                name: "Store 3".into(),
                location: LatLon::new(-33.75, 151.30),
                demand: Load::new(10, 0, 0),
                service: Minutes::from_minutes(5),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(540),
                    Minutes::from_minutes(900),
                )],
            },
        ];

        let vehicles = vec![
            Vehicle {
                id: VehicleId(0),
                name: "Vehicle 1".into(),
                depot: DepotId(0),
                capacity: Load::new(100, 0, 0),
                max_duration: Minutes::from_minutes(600),
                avg_speed_kmh: 60.0,
            },
            Vehicle {
                id: VehicleId(1),
                name: "Vehicle 2".into(),
                depot: DepotId(0),
                capacity: Load::new(100, 0, 0),
                max_duration: Minutes::from_minutes(480),
                avg_speed_kmh: 75.0,
            },
        ];

        let drivers = vec![
            Driver {
                id: DriverId(0),
                name: "Driver 1".into(),
                shifts: vec![Shift::new(
                    Minutes::from_minutes(300),
                    Minutes::from_minutes(1200),
                )],
            },
            Driver {
                id: DriverId(1),
                name: "Driver 2".into(),
                shifts: vec![Shift::new(
                    Minutes::from_minutes(300),
                    Minutes::from_minutes(1200),
                )],
            },
        ];

        Problem {
            name: "small".into(),
            depots: vec![depot],
            stores,
            vehicles,
            drivers,
            cost: CostModel {
                fixed_per_route_cents: 0,
                per_km_cents: 100,
                per_hour_cents: 0,
                late_per_min_cents: 0,
            },
        }
    }

    #[test]
    fn savings_plus_ls_returns_valid_solution() {
        let problem = small_problem();
        let solution = savings_plus_ls(&problem);
        assert!(
            solution.validate(&problem).is_ok(),
            "solution should pass validation"
        );
    }

    #[test]
    fn savings_plus_ls_handles_small_problem() {
        let problem = small_problem();
        let solution = savings_plus_ls(&problem);
        // Should serve at least some stores and produce a valid solution
        let served_count: usize = solution
            .routes
            .iter()
            .flat_map(|r| &r.steps)
            .filter(|s| matches!(s.node, domain::Node::Store(_)))
            .count();
        assert!(served_count > 0, "at least one store should be served");
        // Total served + unassigned should equal total stores
        assert_eq!(
            served_count + solution.unassigned.len(),
            problem.stores.len(),
            "all stores accounted for"
        );
    }

    #[test]
    fn savings_plus_ls_no_duplicate_stores() {
        let problem = small_problem();
        let solution = savings_plus_ls(&problem);
        let mut seen = std::collections::HashSet::new();
        for route in &solution.routes {
            for step in &route.steps {
                if let domain::Node::Store(id) = step.node {
                    assert!(seen.insert(id), "store {id:?} appears twice in solution");
                }
            }
        }
        // Also check unassigned doesn't duplicate served
        for id in &solution.unassigned {
            assert!(
                !seen.contains(id),
                "unassigned store {id:?} also appears in routes"
            );
        }
    }
}
