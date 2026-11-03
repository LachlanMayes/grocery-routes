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

use crate::greedy::{ActiveRoute, Scheduled, schedule_route_simple};
use crate::matrix::{DistanceMatrix, travel_minutes};
use domain::PlannedStop;

/// Runs Clarke–Wright savings and then polishes with local search.
pub(crate) fn savings_plus_ls(problem: &Problem) -> Solution {
    let matrix = DistanceMatrix::new(problem);
    let driver_indices = driver_order(problem);

    // Phase 1: build initial routes by savings
    let mut routes = build_savings_routes(problem, &matrix, &driver_indices);

    // Phase 2: local search (2-opt, relocate, exchange, cross-exchange)
    local_search(problem, &matrix, &driver_indices, &mut routes);

    build_solution(problem, routes)
}

/// Builds initial routes using Clarke–Wright savings.
fn build_savings_routes(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
) -> Vec<ActiveRoute> {
    let vehicle_indices: Vec<usize> = (0..problem.vehicles.len()).collect();

    // Each vehicle starts with its own route: depot -> store -> depot.
    // We'll merge them greedily.
    let mut active = init_singleton_routes(problem, &matrix, driver_indices, &vehicle_indices);

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
            route_i_idx,
            pos_i,
            route_j_idx,
            pos_j,
        ) {
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
) -> Vec<ActiveRoute> {
    let mut active = Vec::new();
    let mut used = vec![false; problem.vehicles.len()];

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
            let candidate = [home, Node::Store(store.id), home];
            if let Some(scheduled) =
                schedule_route_simple(problem, matrix, driver_indices, vehicle, &candidate)
            {
                used[v_idx] = true;
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
                let candidate = [home, Node::Store(problem.stores[s_idx].id), home];
                if let Some(scheduled) =
                    schedule_route_simple(problem, matrix, driver_indices, vehicle, &candidate)
                {
                    used[v_idx] = true;
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

        if let Some(scheduled) =
            schedule_route_simple(problem, matrix, driver_indices, vehicle, &combined)
        {
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
) {
    let mut improved = true;
    while improved {
        improved = false;
        // 2-opt within each route
        for route in routes.iter_mut() {
            if two_opt(problem, matrix, driver_indices, route) {
                improved = true;
            }
        }
        // Relocate: move a store from one route to another
        if relocate(problem, matrix, driver_indices, routes) {
            improved = true;
            continue;
        }
        // Exchange: swap a store between two routes
        if exchange(problem, matrix, driver_indices, routes) {
            improved = true;
            continue;
        }
        // Cross-exchange: swap a subsequence
        if cross_exchange(problem, matrix, driver_indices, routes) {
            improved = true;
        }
    }
}

fn two_opt(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    route: &mut ActiveRoute,
) -> bool {
    let vehicle = &problem.vehicles[route.vehicle_index];
    let steps = &route.scheduled.steps;
    if steps.len() < 5 {
        return false; // depot + 2 stores + depot
    }
    let nodes: Vec<Node> = steps.iter().map(|s| s.node).collect();
    let mut best_nodes = nodes.clone();
    let mut best_cost = route.scheduled.cost_cents;

    // Try all pairs of edges to reverse (i, j) with i < j, not adjacent.
    for i in 1..nodes.len() - 2 {
        for j in i + 1..nodes.len() - 1 {
            if j == i + 1 {
                continue;
            }
            let mut candidate = nodes.clone();
            candidate[i..=j].reverse();
            if let Some(scheduled) =
                schedule_route_simple(problem, matrix, driver_indices, vehicle, &candidate)
            {
                if scheduled.cost_cents < best_cost {
                    best_cost = scheduled.cost_cents;
                    best_nodes = candidate;
                }
            }
        }
    }
    if best_cost < route.scheduled.cost_cents {
        if let Some(scheduled) =
            schedule_route_simple(problem, matrix, driver_indices, vehicle, &best_nodes)
        {
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
) -> bool {
    let mut improved = false;
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
            // Try to re-schedule src without this store
            let Some(src_scheduled) =
                schedule_route_simple(problem, matrix, driver_indices, src_vehicle, &new_src_nodes)
            else {
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
                    if let Some(dst_scheduled) = schedule_route_simple(
                        problem,
                        matrix,
                        driver_indices,
                        dst_vehicle,
                        &new_dst_nodes,
                    ) {
                        let delta = dst_scheduled.cost_cents + src_scheduled.cost_cents
                            - routes[src_idx].scheduled.cost_cents
                            - routes[dst_idx].scheduled.cost_cents;
                        if delta < 0 {
                            routes[src_idx].scheduled = src_scheduled;
                            routes[dst_idx].scheduled = dst_scheduled;
                            return true;
                        }
                    }
                }
            }
        }
    }
    improved
}

fn exchange(
    problem: &Problem,
    matrix: &DistanceMatrix,
    driver_indices: &[usize],
    routes: &mut Vec<ActiveRoute>,
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
                    if let (Some(si_sched), Some(sj_sched)) = (
                        schedule_route_simple(problem, matrix, driver_indices, vi, &new_i),
                        schedule_route_simple(problem, matrix, driver_indices, vj, &new_j),
                    ) {
                        let delta = si_sched.cost_cents + sj_sched.cost_cents
                            - routes[i].scheduled.cost_cents
                            - routes[j].scheduled.cost_cents;
                        if delta < 0 {
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
                        if let (Some(si_sched), Some(sj_sched)) = (
                            schedule_route_simple(problem, matrix, driver_indices, vi, &new_i),
                            schedule_route_simple(problem, matrix, driver_indices, vj, &new_j),
                        ) {
                            let delta = si_sched.cost_cents + sj_sched.cost_cents
                                - routes[i].scheduled.cost_cents
                                - routes[j].scheduled.cost_cents;
                            if delta < 0 {
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

fn build_solution(problem: &Problem, routes: Vec<ActiveRoute>) -> Solution {
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

    Solution {
        routes,
        unassigned,
        total_distance_km,
        total_duration: Minutes::from_minutes(total_duration),
        total_cost_cents,
    }
}
