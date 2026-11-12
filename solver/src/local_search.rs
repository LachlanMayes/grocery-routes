//! Local search improvements for the greedy baseline (M2).
//!
//! Applies iterative improvement moves to a greedy solution:
//! - 2-opt: reverse a subsequence within a route
//! - Relocate: move a store from one route to another
//! - Exchange: swap a store between two routes
//! - Cross-exchange: swap subsequences between two routes
//!
//! All moves respect driver occupancy, capacity, time windows, and shift limits
//! by reusing the scheduling logic from the greedy heuristic.

use domain::{Minutes, Node, PlannedStop, Problem, RoutePlan, Solution, StoreId};

use super::greedy::{Legs, Scheduled, plan_with_matrix, schedule_shift_candidate, spans_overlap};
use crate::matrix::DistanceMatrix;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng, seq::SliceRandom};

/// A route in the local search, with its schedule and metadata.
#[derive(Debug, Clone)]
struct LSRoute {
    vehicle_index: usize,
    scheduled: Scheduled,
}

/// A solution in the local search context.
#[derive(Debug, Clone)]
struct LSSolution {
    routes: Vec<LSRoute>,
    unassigned: Vec<StoreId>,
    total_cost_cents: i64,
    /// Driver busy spans: for each driver, list of (start, end) spans they are busy.
    /// Used to enforce driver occupancy constraint during moves.
    driver_busy: Vec<Vec<(Minutes, Minutes)>>,
}

/// Configuration for local search.
#[derive(Debug, Clone)]
pub struct LSConfig {
    /// Maximum iterations without improvement before stopping.
    pub max_no_improve: usize,
    /// Random seed for multi-start (0 = deterministic from problem hash).
    pub seed: u64,
    /// Number of multi-start restarts (0 = single run).
    pub restarts: usize,
    /// Time limit in milliseconds (0 = no limit).
    pub time_limit_ms: u64,
}

impl Default for LSConfig {
    fn default() -> Self {
        Self {
            max_no_improve: 1000,
            seed: 0,
            restarts: 4,
            time_limit_ms: 30_000,
        }
    }
}

/// Runs local search on top of a greedy solution.
///
/// Returns an improved solution and the best cost found.
pub fn improve(
    problem: &Problem,
    matrix: &DistanceMatrix,
    legs: &mut Legs<'_>,
    greedy_solution: Solution,
    config: LSConfig,
) -> Solution {
    // Convert greedy solution to LS internal representation
    let mut current = greedy_to_ls(problem, greedy_solution);
    let mut best = current.clone();

    // Multi-start loop
    let base_seed = if config.seed == 0 {
        hash_problem(problem)
    } else {
        config.seed
    };

    let start_time = std::time::Instant::now();

    for restart in 0..=config.restarts {
        let seed = base_seed.wrapping_add(restart as u64);
        let mut rng = StdRng::seed_from_u64(seed);

        // Perturb the current solution slightly for diversification
        if restart > 0 {
            current = perturb_solution(problem, legs, current, &mut rng);
        }

        // Hill climbing with first-improvement strategy
        let mut no_improve = 0;
        while no_improve < config.max_no_improve {
            if config.time_limit_ms > 0
                && start_time.elapsed().as_millis() > config.time_limit_ms as u128
            {
                break;
            }

            // Try moves in order: 2-opt (fastest), then relocate, exchange, cross-exchange
            let improved = two_opt(problem, legs, &mut current)
                || relocate(problem, legs, &mut current, &mut rng)
                || exchange(problem, legs, &mut current, &mut rng)
                || cross_exchange(problem, legs, &mut current, &mut rng);

            if improved {
                no_improve = 0;
                if current.total_cost_cents < best.total_cost_cents {
                    best = current.clone();
                }
            } else {
                no_improve += 1;
            }
        }
    }

    // Convert back to public Solution
    let solution = ls_to_solution(problem, best);

    // Final validation - ensure the solution satisfies all constraints
    if let Err(violations) = solution.validate(problem) {
        // Log violations but fall back to greedy if local search produced invalid solution
        eprintln!("Local search produced invalid solution, falling back to greedy:");
        for v in &violations {
            eprintln!("  - {v}");
        }
        // Fall back to greedy solution using the same matrix
        plan_with_matrix(problem, matrix.clone())
    } else {
        solution
    }
}

/// Converts a public Solution to internal LS representation.
fn greedy_to_ls(problem: &Problem, solution: Solution) -> LSSolution {
    let mut total_cost = 0;
    let mut driver_busy: Vec<Vec<(Minutes, Minutes)>> = vec![Vec::new(); problem.drivers.len()];
    let routes = solution
        .routes
        .into_iter()
        .map(|route| {
            let _vehicle = &problem
                .vehicles
                .iter()
                .find(|v| v.id == route.vehicle)
                .unwrap();
            let vehicle_index = problem
                .vehicles
                .iter()
                .position(|v| v.id == route.vehicle)
                .unwrap();
            total_cost += route.cost_cents;
            let driver = route
                .drivers
                .first()
                .copied()
                .expect("route must have at least one driver");
            let span = (route.steps[0].depart, route.steps.last().unwrap().arrive);
            driver_busy[driver.0 as usize].push(span);
            LSRoute {
                vehicle_index,
                scheduled: Scheduled {
                    driver,
                    span,
                    steps: route.steps,
                    distance_km: route.distance_km,
                    duration: route.duration,
                    cost_cents: route.cost_cents,
                },
            }
        })
        .collect();

    LSSolution {
        routes,
        unassigned: solution.unassigned,
        total_cost_cents: total_cost,
        driver_busy,
    }
}

/// Recomputes the busy spans of all drivers from the current routes.
///
/// Local-search moves alter routes, so expensive bookkeeping is centralised
/// here and always produces a fresh, non-stale view of occupancy.
fn recompute_driver_busy(problem: &Problem, routes: &[LSRoute]) -> Vec<Vec<(Minutes, Minutes)>> {
    let mut busy = vec![Vec::new(); problem.drivers.len()];
    for route in routes {
        let Some(spans) = busy.get_mut(route.scheduled.driver.0 as usize) else {
            continue;
        };
        spans.push(route.scheduled.span);
    }
    busy
}

/// True when the two candidate routes (with possibly new schedules) share a
/// driver whose work spans overlap.
fn candidates_double_book_same_driver(a: &Scheduled, b: &Scheduled) -> bool {
    a.driver == b.driver && spans_overlap(a.span, b.span)
}

/// Extracts the node sequence from a scheduled route.
fn nodes_from_steps(steps: &[PlannedStop]) -> Vec<Node> {
    steps.iter().map(|s| s.node).collect()
}

fn ls_to_solution(problem: &Problem, ls: LSSolution) -> Solution {
    let routes: Vec<RoutePlan> = ls
        .routes
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

    let total_distance_km = routes.iter().map(|r| r.distance_km).sum();
    let total_duration = routes.iter().map(|r| r.duration.minutes()).sum();
    let total_cost_cents = routes.iter().map(|r| r.cost_cents).sum();

    let mut unassigned = ls.unassigned;
    unassigned.sort_unstable();

    Solution {
        routes,
        unassigned,
        total_distance_km,
        total_duration: Minutes::from_minutes(total_duration),
        total_cost_cents,
    }
}

/// Hash a problem to a u64 for deterministic seeding.
fn hash_problem(problem: &Problem) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    problem.name.hash(&mut hasher);
    problem.stores.len().hash(&mut hasher);
    problem.vehicles.len().hash(&mut hasher);
    problem.drivers.len().hash(&mut hasher);
    for store in &problem.stores {
        store.id.0.hash(&mut hasher);
        store.location.lat.to_bits().hash(&mut hasher);
        store.location.lon.to_bits().hash(&mut hasher);
    }
    for vehicle in &problem.vehicles {
        vehicle.id.0.hash(&mut hasher);
        vehicle.capacity.ambient.hash(&mut hasher);
        vehicle.capacity.chilled.hash(&mut hasher);
        vehicle.capacity.frozen.hash(&mut hasher);
    }
    hasher.finish()
}

/// Perturbs a solution by randomly relocating a few stores.
fn perturb_solution(
    problem: &Problem,
    legs: &mut Legs<'_>,
    mut solution: LSSolution,
    rng: &mut StdRng,
) -> LSSolution {
    if solution.routes.is_empty() {
        return solution;
    }

    // Randomly select up to 3 stores to relocate
    let num_perturb = solution.routes.len().clamp(1, 3);
    let mut all_stores: Vec<(usize, usize, StoreId)> = Vec::new(); // (route_idx, pos_in_route, store_id)

    for (r_idx, route) in solution.routes.iter().enumerate() {
        for (pos, step) in route.scheduled.steps.iter().enumerate() {
            if let Node::Store(id) = step.node {
                all_stores.push((r_idx, pos, id));
            }
        }
    }

    if all_stores.is_empty() {
        return solution;
    }

    for _ in 0..num_perturb {
        if all_stores.is_empty() {
            break;
        }
        let idx = rng.random_range(0..all_stores.len());
        let (route_idx, pos, store_id) = all_stores.swap_remove(idx);

        // Try to move this store to a different route
        let other_routes: Vec<usize> = (0..solution.routes.len())
            .filter(|&i| i != route_idx)
            .collect();
        if other_routes.is_empty() {
            break;
        }
        let new_route_idx = other_routes[rng.random_range(0..other_routes.len())];

        // Try inserting at a random position in the new route
        let dest_route = &solution.routes[new_route_idx];
        if dest_route.scheduled.steps.len() >= 3 {
            let insert_pos = rng.random_range(1..dest_route.scheduled.steps.len() - 1);
            if try_relocate_direct(
                problem,
                legs,
                &mut solution,
                route_idx,
                pos,
                new_route_idx,
                insert_pos,
            ) {
                // Successfully moved, update all_stores
                all_stores.retain(|(_, _, id)| *id != store_id);
            }
        }
    }
    solution
}

/// Helper to try a direct relocation during perturbation.
fn try_relocate_direct(
    problem: &Problem,
    legs: &mut Legs<'_>,
    solution: &mut LSSolution,
    src_idx: usize,
    src_pos: usize,
    dst_idx: usize,
    insert_pos: usize,
) -> bool {
    let src_route = &solution.routes[src_idx];
    let src_vehicle = &problem.vehicles[src_route.vehicle_index];
    let src_driver = src_route.scheduled.driver;

    // Remove from source
    let mut new_src_steps = src_route.scheduled.steps.clone();
    new_src_steps.remove(src_pos);
    if new_src_steps.len() < 3 {
        return false;
    }

    let new_src_nodes = nodes_from_steps(&new_src_steps);

    let Some(src_scheduled) = schedule_shift_candidate(
        problem,
        legs,
        src_vehicle,
        src_driver,
        &new_src_nodes,
        &solution.driver_busy,
        Some((src_driver.0 as usize, src_route.scheduled.span)),
    ) else {
        return false;
    };

    let dst_route = &solution.routes[dst_idx];
    let dst_vehicle = &problem.vehicles[dst_route.vehicle_index];
    let dst_driver = dst_route.scheduled.driver;
    let mut new_dst_steps = dst_route.scheduled.steps.clone();
    new_dst_steps.insert(
        insert_pos,
        PlannedStop {
            node: Node::Store(
                store_id_from_steps(&solution.routes[src_idx].scheduled.steps, src_pos).unwrap(),
            ),
            arrive: Minutes::ZERO,
            depart: Minutes::ZERO,
        },
    );

    let new_dst_nodes = nodes_from_steps(&new_dst_steps);

    let Some(dst_scheduled) = schedule_shift_candidate(
        problem,
        legs,
        dst_vehicle,
        dst_driver,
        &new_dst_nodes,
        &solution.driver_busy,
        Some((dst_driver.0 as usize, dst_route.scheduled.span)),
    ) else {
        return false;
    };

    let delta = src_scheduled.cost_cents + dst_scheduled.cost_cents
        - solution.routes[src_idx].scheduled.cost_cents
        - solution.routes[dst_idx].scheduled.cost_cents;

    if delta < 0 && !candidates_double_book_same_driver(&src_scheduled, &dst_scheduled) {
        solution.routes[src_idx].scheduled = src_scheduled;
        solution.routes[dst_idx].scheduled = dst_scheduled;
        solution.total_cost_cents += delta;
        solution.driver_busy = recompute_driver_busy(problem, &solution.routes);
        return true;
    }
    false
}

/// Extract store ID from route steps at position.
fn store_id_from_steps(steps: &[PlannedStop], pos: usize) -> Option<StoreId> {
    if let Node::Store(id) = steps[pos].node {
        Some(id)
    } else {
        None
    }
}

/// 2-opt: reverse a subsequence within a single route.
fn two_opt(problem: &Problem, legs: &mut Legs<'_>, solution: &mut LSSolution) -> bool {
    for route_idx in 0..solution.routes.len() {
        let route = &solution.routes[route_idx];
        let vehicle_idx = route.vehicle_index;

        if route.scheduled.steps.len() < 5 {
            continue; // Need at least depot, 2 stores, depot
        }

        let steps = &route.scheduled.steps;

        // Try all pairs of edges to reverse
        for i in 1..steps.len() - 2 {
            for j in i + 1..steps.len() - 1 {
                let mut candidate_steps = steps.clone();
                candidate_steps[i..=j].reverse();

                let candidate_nodes = nodes_from_steps(&candidate_steps);
                let vehicle = &problem.vehicles[vehicle_idx];
                let driver = solution.routes[route_idx].scheduled.driver;
                let vehicle_ref = &solution.routes[route_idx];

                if let Some(scheduled) = schedule_shift_candidate(
                    problem,
                    legs,
                    vehicle,
                    driver,
                    &candidate_nodes,
                    &solution.driver_busy,
                    Some((driver.0 as usize, vehicle_ref.scheduled.span)),
                ) {
                    let delta = scheduled.cost_cents - vehicle_ref.scheduled.cost_cents;
                    if delta < 0 {
                        solution.routes[route_idx].scheduled = scheduled;
                        solution.total_cost_cents += delta;
                        solution.driver_busy = recompute_driver_busy(problem, &solution.routes);
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Builds the static leg source, or `None` when the solution cannot use a
/// Relocate: move a store from one route to another.
fn relocate(
    problem: &Problem,
    legs: &mut Legs<'_>,
    solution: &mut LSSolution,
    rng: &mut StdRng,
) -> bool {
    if solution.routes.len() < 2 {
        return false;
    }

    // Collect all movable stores
    let mut candidates = Vec::new();
    for (src_idx, route) in solution.routes.iter().enumerate() {
        for pos in 1..route.scheduled.steps.len() - 1 {
            if let Node::Store(store_id) = route.scheduled.steps[pos].node {
                candidates.push((src_idx, pos, store_id));
            }
        }
    }

    if candidates.is_empty() {
        return false;
    }

    // Shuffle for random exploration
    candidates.shuffle(rng);

    for (src_idx, src_pos, store_id) in candidates {
        let src_route = &solution.routes[src_idx];
        let src_vehicle = &problem.vehicles[src_route.vehicle_index];

        // Try removing from source
        let mut new_src_steps = src_route.scheduled.steps.clone();
        new_src_steps.remove(src_pos);
        if new_src_steps.len() < 3 {
            continue; // Would leave only depot-depot
        }

        let new_src_nodes = nodes_from_steps(&new_src_steps);

        // Try to re-schedule source without this store
        let src_driver = src_route.scheduled.driver;
        let Some(src_scheduled) = schedule_shift_candidate(
            problem,
            legs,
            src_vehicle,
            src_driver,
            &new_src_nodes,
            &solution.driver_busy,
            Some((src_driver.0 as usize, src_route.scheduled.span)),
        ) else {
            continue;
        };

        // Try inserting into each other route
        for (dst_idx, dst_route) in solution.routes.iter().enumerate() {
            if dst_idx == src_idx {
                continue;
            }
            let dst_vehicle = &problem.vehicles[dst_route.vehicle_index];
            let dst_steps = &dst_route.scheduled.steps;
            if dst_steps.len() < 2 {
                continue;
            }

            for insert_pos in 1..dst_steps.len() - 1 {
                let mut new_dst_steps = dst_steps.clone();
                new_dst_steps.insert(
                    insert_pos,
                    PlannedStop {
                        node: Node::Store(store_id),
                        arrive: Minutes::ZERO,
                        depart: Minutes::ZERO,
                    },
                );
                let new_dst_nodes = nodes_from_steps(&new_dst_steps);

                let dst_driver = dst_route.scheduled.driver;
                let Some(dst_scheduled) = schedule_shift_candidate(
                    problem,
                    legs,
                    dst_vehicle,
                    dst_driver,
                    &new_dst_nodes,
                    &solution.driver_busy,
                    Some((dst_driver.0 as usize, dst_route.scheduled.span)),
                ) else {
                    continue;
                };

                let delta = src_scheduled.cost_cents + dst_scheduled.cost_cents
                    - solution.routes[src_idx].scheduled.cost_cents
                    - solution.routes[dst_idx].scheduled.cost_cents;

                if delta < 0 && !candidates_double_book_same_driver(&src_scheduled, &dst_scheduled)
                {
                    // Apply the move
                    solution.routes[src_idx].scheduled = src_scheduled;
                    solution.routes[dst_idx].scheduled = dst_scheduled;
                    solution.total_cost_cents += delta;
                    solution.driver_busy = recompute_driver_busy(problem, &solution.routes);
                    return true;
                }
            }
        }
    }
    false
}

/// Exchange: swap a store between two routes.
fn exchange(
    problem: &Problem,
    legs: &mut Legs<'_>,
    solution: &mut LSSolution,
    rng: &mut StdRng,
) -> bool {
    if solution.routes.len() < 2 {
        return false;
    }

    let mut pairs = Vec::new();
    for i in 0..solution.routes.len() {
        for j in (i + 1)..solution.routes.len() {
            let route_i = &solution.routes[i];
            let route_j = &solution.routes[j];
            if route_i.scheduled.steps.len() <= 3 || route_j.scheduled.steps.len() <= 3 {
                continue;
            }
            for pos_i in 1..route_i.scheduled.steps.len() - 1 {
                let Node::Store(si) = route_i.scheduled.steps[pos_i].node else {
                    continue;
                };
                for pos_j in 1..route_j.scheduled.steps.len() - 1 {
                    let Node::Store(sj) = route_j.scheduled.steps[pos_j].node else {
                        continue;
                    };
                    pairs.push((i, pos_i, si, j, pos_j, sj));
                }
            }
        }
    }

    if pairs.is_empty() {
        return false;
    }

    pairs.shuffle(rng);

    for (i, pos_i, si, j, pos_j, sj) in pairs {
        let route_i = &solution.routes[i];
        let route_j = &solution.routes[j];
        let vi = &problem.vehicles[route_i.vehicle_index];
        let vj = &problem.vehicles[route_j.vehicle_index];

        let mut new_i_steps = route_i.scheduled.steps.clone();
        let mut new_j_steps = route_j.scheduled.steps.clone();
        new_i_steps[pos_i] = PlannedStop {
            node: Node::Store(sj),
            arrive: Minutes::ZERO,
            depart: Minutes::ZERO,
        };
        new_j_steps[pos_j] = PlannedStop {
            node: Node::Store(si),
            arrive: Minutes::ZERO,
            depart: Minutes::ZERO,
        };

        let new_i_nodes = nodes_from_steps(&new_i_steps);
        let new_j_nodes = nodes_from_steps(&new_j_steps);

        let driver_i = route_i.scheduled.driver;
        let driver_j = route_j.scheduled.driver;

        if let (Some(si_sched), Some(sj_sched)) = (
            schedule_shift_candidate(
                problem,
                legs,
                vi,
                driver_i,
                &new_i_nodes,
                &solution.driver_busy,
                Some((driver_i.0 as usize, route_i.scheduled.span)),
            ),
            schedule_shift_candidate(
                problem,
                legs,
                vj,
                driver_j,
                &new_j_nodes,
                &solution.driver_busy,
                Some((driver_j.0 as usize, route_j.scheduled.span)),
            ),
        ) {
            let delta = si_sched.cost_cents + sj_sched.cost_cents
                - route_i.scheduled.cost_cents
                - route_j.scheduled.cost_cents;

            if delta < 0 && !candidates_double_book_same_driver(&si_sched, &sj_sched) {
                solution.routes[i].scheduled = si_sched;
                solution.routes[j].scheduled = sj_sched;
                solution.total_cost_cents += delta;
                solution.driver_busy = recompute_driver_busy(problem, &solution.routes);
                return true;
            }
        }
    }
    false
}

/// Cross-exchange: swap subsequences of length 1..2 between two routes.
fn cross_exchange(
    problem: &Problem,
    legs: &mut Legs<'_>,
    solution: &mut LSSolution,
    _rng: &mut StdRng,
) -> bool {
    if solution.routes.len() < 2 {
        return false;
    }

    for i in 0..solution.routes.len() {
        for j in (i + 1)..solution.routes.len() {
            let route_i = &solution.routes[i];
            let route_j = &solution.routes[j];
            let vi = &problem.vehicles[route_i.vehicle_index];
            let vj = &problem.vehicles[route_j.vehicle_index];
            let steps_i = &route_i.scheduled.steps;
            let steps_j = &route_j.scheduled.steps;
            if steps_i.len() < 3 || steps_j.len() < 3 {
                continue;
            }

            // Convert to node sequences for manipulation
            let nodes_i = nodes_from_steps(steps_i);
            let nodes_j = nodes_from_steps(steps_j);
            if nodes_i.len() < 3 || nodes_j.len() < 3 {
                continue;
            }

            // Length 1 and 2 subsequences
            for len in 1..=2 {
                for start_i in 1..=nodes_i.len() - 1 - len {
                    let sub_i = nodes_i[start_i..start_i + len].to_vec();
                    for start_j in 1..=nodes_j.len() - 1 - len {
                        let sub_j = nodes_j[start_j..start_j + len].to_vec();

                        let mut new_i = nodes_i.clone();
                        let mut new_j = nodes_j.clone();
                        new_i.splice(start_i..start_i + len, sub_j.clone());
                        new_j.splice(start_j..start_j + len, sub_i.clone());

                        let driver_i = route_i.scheduled.driver;
                        let driver_j = route_j.scheduled.driver;

                        if let (Some(si_sched), Some(sj_sched)) = (
                            schedule_shift_candidate(
                                problem,
                                legs,
                                vi,
                                driver_i,
                                &new_i,
                                &solution.driver_busy,
                                Some((driver_i.0 as usize, route_i.scheduled.span)),
                            ),
                            schedule_shift_candidate(
                                problem,
                                legs,
                                vj,
                                driver_j,
                                &new_j,
                                &solution.driver_busy,
                                Some((driver_j.0 as usize, route_j.scheduled.span)),
                            ),
                        ) {
                            let delta = si_sched.cost_cents + sj_sched.cost_cents
                                - route_i.scheduled.cost_cents
                                - route_j.scheduled.cost_cents;

                            if delta < 0
                                && !candidates_double_book_same_driver(&si_sched, &sj_sched)
                            {
                                solution.routes[i].scheduled = si_sched;
                                solution.routes[j].scheduled = sj_sched;
                                solution.total_cost_cents += delta;
                                solution.driver_busy =
                                    recompute_driver_busy(problem, &solution.routes);
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

// `schedule_shift_candidate` now lives in `greedy` (shared with LNS).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::greedy::plan;
    use domain::{
        Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
        LatLon, Load, Minutes, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
    };

    fn simple_problem() -> Problem {
        Problem {
            name: "ls test".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Depot".into(),
                location: LatLon::new(-33.87, 151.21),
                timezone: None,
            }],
            stores: vec![
                Store {
                    id: StoreId(0),
                    chain: Chain::Coles,
                    name: "Store 0".into(),
                    location: LatLon::new(-33.90, 151.20),
                    timezone: None,
                    demand: Load::new(1, 0, 0),
                    service: Minutes::from_minutes(10),
                    receiving: vec![TimeWindow::new(
                        Minutes::from_minutes(300),
                        Minutes::from_minutes(600),
                    )],
                },
                Store {
                    id: StoreId(1),
                    chain: Chain::Coles,
                    name: "Store 1".into(),
                    location: LatLon::new(-33.85, 151.25),
                    timezone: None,
                    demand: Load::new(1, 0, 0),
                    service: Minutes::from_minutes(10),
                    receiving: vec![TimeWindow::new(
                        Minutes::from_minutes(300),
                        Minutes::from_minutes(600),
                    )],
                },
            ],
            vehicles: vec![
                Vehicle {
                    id: VehicleId(0),
                    name: "V1".into(),
                    depot: DepotId(0),
                    capacity: Load::new(10, 10, 10),
                    max_duration: Minutes::from_minutes(480),
                    avg_speed_kmh: 60.0,
                },
                Vehicle {
                    id: VehicleId(1),
                    name: "V2".into(),
                    depot: DepotId(0),
                    capacity: Load::new(10, 10, 10),
                    max_duration: Minutes::from_minutes(480),
                    avg_speed_kmh: 60.0,
                },
            ],
            drivers: vec![
                Driver {
                    id: DriverId(0),
                    name: "D1".into(),
                    shifts: vec![Shift::new(
                        Minutes::from_minutes(300),
                        Minutes::from_minutes(900),
                    )],
                    fatigue_rules: FatigueRules::default(),
                },
                Driver {
                    id: DriverId(1),
                    name: "D2".into(),
                    shifts: vec![Shift::new(
                        Minutes::from_minutes(300),
                        Minutes::from_minutes(900),
                    )],
                    fatigue_rules: FatigueRules::default(),
                },
            ],
            cost: CostModel {
                fixed_per_route_cents: 10_000,
                per_km_cents: 100,
                per_hour_cents: 5_000,
                late_per_min_cents: 0,
            },
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
        }
    }

    #[test]
    fn local_search_does_not_worsen_greedy() {
        let problem = simple_problem();
        let matrix = crate::matrix::DistanceMatrix::new(&problem);
        let greedy = plan(&problem);
        let greedy_cost = greedy.total_cost_cents;

        let config = LSConfig {
            max_no_improve: 100,
            seed: 42,
            restarts: 2,
            time_limit_ms: 5000,
        };

        let mut legs = Legs::Static(&matrix);
        let improved = improve(&problem, &matrix, &mut legs, greedy, config);

        // Should still be valid
        assert_eq!(improved.validate(&problem), Ok(()));
        // Cost should not increase
        assert!(improved.total_cost_cents <= greedy_cost);
    }

    #[test]
    fn ls_config_defaults() {
        let config = LSConfig::default();
        assert_eq!(config.max_no_improve, 1000);
        assert_eq!(config.seed, 0);
        assert_eq!(config.restarts, 4);
        assert_eq!(config.time_limit_ms, 30_000);
    }
}
