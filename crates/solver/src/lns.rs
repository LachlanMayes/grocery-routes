//! Large Neighborhood Search (LNS) and Adaptive LNS (ALNS) for M2.
//!
//! LNS escapes local optima by repeatedly destroying (ruining) a significant
//! portion of the current solution and then repairing (recreating) it.
//! ALNS adaptively selects ruin/recreate operators based on their historical
//! performance.
//!
//! rustfmt::skip_file
// rustfmt::skip_file

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use domain::{DepotId, DriverId, Minutes, Node, Problem, RoutePlan, Solution, StoreId, Vehicle};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng, seq::SliceRandom};

use crate::greedy::{
    Scheduled, plan_with_matrix, schedule_route as greedy_schedule_route,
    schedule_shift as greedy_schedule_shift, spans_overlap,
};
use crate::matrix::DistanceMatrix;

/// A route in the LNS context.
#[derive(Debug, Clone)]
struct LNSRoute {
    vehicle_index: usize,
    scheduled: Scheduled,
}

/// A solution in the LNS context.
#[derive(Debug, Clone)]
struct LNSSolution {
    routes: Vec<LNSRoute>,
    unassigned: Vec<StoreId>,
    total_cost_cents: i64,
    driver_busy: Vec<Vec<(Minutes, Minutes)>>,
}

/// Configuration for LNS/ALNS.
#[derive(Debug, Clone)]
pub struct LNSConfig {
    /// Maximum iterations without global improvement.
    pub max_no_improve: usize,
    /// Number of stores to remove per ruin (min, max).
    pub ruin_size_range: (usize, usize),
    /// Random seed (0 = deterministic from problem hash).
    pub seed: u64,
    /// Time limit in milliseconds (0 = no limit).
    pub time_limit_ms: u64,
    /// Initial temperature for simulated annealing.
    pub initial_temperature: f64,
    /// Cooling rate per iteration.
    pub cooling_rate: f64,
    /// Minimum temperature.
    pub min_temperature: f64,
    /// Whether to use ALNS (adaptive operator selection).
    pub adaptive: bool,
    /// Reaction factor for weight adaptation (0-1).
    pub reaction_factor: f64,
}

impl Default for LNSConfig {
    fn default() -> Self {
        Self {
            max_no_improve: 1000,
            ruin_size_range: (8, 20),
            seed: 0,
            time_limit_ms: 60_000,
            initial_temperature: 2000.0,
            cooling_rate: 0.9997,
            min_temperature: 0.1,
            adaptive: true,
            reaction_factor: 0.15,
        }
    }
}

/// Statistics for an operator.
#[derive(Debug, Clone, Default)]
struct OperatorStats {
    weight: f64,
    score_sum: f64,
    usage_count: usize,
    conflicting_ops: HashSet<&'static str>,
}

/// Result of a ruin operation.
#[derive(Debug, Clone)]
struct RuinResult {
    _removed_stores: Vec<StoreId>,
    modified_solution: LNSSolution,
}

/// Primal integral tracker for anytime evaluation (Finding 11).
#[derive(Debug, Clone, Default)]
struct PrimalIntegralTracker {
    integral: f64,
    last_time: f64,
    last_cost: f64,
    _start_time: f64,
}

impl PrimalIntegralTracker {
    fn new(start_time: f64, initial_cost: f64) -> Self {
        Self {
            integral: 0.0,
            last_time: 0.0,
            last_cost: initial_cost,
            _start_time: start_time,
        }
    }

    fn update(&mut self, elapsed: f64, current_cost: f64) {
        if elapsed > self.last_time {
            let dt = elapsed - self.last_time;
            let avg_cost = (self.last_cost + current_cost) / 2.0;
            self.integral += avg_cost * dt;
            self.last_time = elapsed;
            self.last_cost = current_cost;
        }
    }

    fn normalized_integral(&self, initial_cost: f64, best_cost: f64) -> f64 {
        if initial_cost == best_cost {
            return 0.0;
        }
        self.integral / (initial_cost - best_cost) as f64
    }
}

/// A feasible insertion for the LNS repair phase.
#[derive(Debug, Clone)]
struct LNSInsertion {
    route_idx: usize,
    _pos: usize,
    vehicle_index: usize,
    scheduled: Scheduled,
    cost_cents: i64,
}

/// Runs LNS/ALNS on top of a greedy solution.
pub fn solve_lns(
    problem: &Problem,
    matrix: &DistanceMatrix,
    greedy_solution: Solution,
    config: LNSConfig,
) -> Solution {
    let Some(mut current) = greedy_to_lns(problem, greedy_solution) else {
        eprintln!("LNS: greedy_to_lns failed, falling back to greedy");
        return plan_with_matrix(problem, matrix.clone());
    };
    let mut best = current.clone();
    let mut global_best = current.clone();

    let mut ruin_weights: HashMap<&'static str, OperatorStats> = HashMap::from([
        (
            "random_removal",
            OperatorStats {
                weight: 1.0,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::new(),
            },
        ),
        (
            "worst_removal",
            OperatorStats {
                weight: 1.0,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::new(),
            },
        ),
        (
            "proximity_removal",
            OperatorStats {
                weight: 1.0,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::new(),
            },
        ),
        (
            "shaw_removal",
            OperatorStats {
                weight: 1.5,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::from(["time_window_removal"]),
            },
        ),
        (
            "time_window_removal",
            OperatorStats {
                weight: 1.5,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::from(["shaw_removal"]),
            },
        ),
        (
            "route_removal",
            OperatorStats {
                weight: 0.8,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::from(["cluster_removal"]),
            },
        ),
        (
            "cluster_removal",
            OperatorStats {
                weight: 1.2,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::from(["route_removal"]),
            },
        ),
    ]);
    let mut recreate_weights: HashMap<&'static str, OperatorStats> = HashMap::from([
        (
            "greedy_repair",
            OperatorStats {
                weight: 1.0,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::new(),
            },
        ),
        (
            "regret_repair",
            OperatorStats {
                weight: 1.2,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::new(),
            },
        ),
        (
            "regret3_repair",
            OperatorStats {
                weight: 1.3,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::new(),
            },
        ),
        (
            "regret4_repair",
            OperatorStats {
                weight: 1.1,
                usage_count: 0,
                score_sum: 0.0,
                conflicting_ops: HashSet::new(),
            },
        ),
    ]);

    let base_seed = if config.seed == 0 {
        hash_problem(problem) as u64
    } else {
        config.seed
    };
    let mut rng = StdRng::seed_from_u64(base_seed);

    let start_time = Instant::now();
    let mut temperature = config.initial_temperature;
    let mut no_improve = 0;
    let initial_cost = current.total_cost_cents as f64;
    let mut primal_tracker =
        PrimalIntegralTracker::new(start_time.elapsed().as_secs_f64(), initial_cost);

    while no_improve < config.max_no_improve {
        if config.time_limit_ms > 0
            && start_time.elapsed().as_millis() > config.time_limit_ms as u128
        {
            break;
        }

        // Select ruin and recreate operators
        let ruin_op = if config.adaptive {
            select_operator_weighted(&ruin_weights, &mut rng)
        } else {
            let ops = [
                "random_removal",
                "worst_removal",
                "proximity_removal",
                "shaw_removal",
                "time_window_removal",
                "route_removal",
                "cluster_removal",
            ];
            let idx = rng.random_range(0..ops.len());
            ops[idx]
        };
        let recreate_op = if config.adaptive {
            select_operator_weighted(&recreate_weights, &mut rng)
        } else {
            let ops = [
                "greedy_repair",
                "regret_repair",
                "regret3_repair",
                "regret4_repair",
            ];
            let idx = rng.random_range(0..ops.len());
            ops[idx]
        };

        // Apply ruin
        let ruin_result = match ruin_op {
            "random_removal" => {
                random_removal(problem, matrix, &current, &mut rng, config.ruin_size_range)
            }
            "worst_removal" => {
                worst_removal(problem, matrix, &current, &mut rng, config.ruin_size_range)
            }
            "proximity_removal" => {
                proximity_removal(problem, matrix, &current, &mut rng, config.ruin_size_range)
            }
            "shaw_removal" => {
                shaw_removal(problem, matrix, &current, &mut rng, config.ruin_size_range)
            }
            "time_window_removal" => {
                time_window_removal(problem, matrix, &current, &mut rng, config.ruin_size_range)
            }
            "route_removal" => {
                route_removal(problem, matrix, &current, &mut rng, config.ruin_size_range)
            }
            "cluster_removal" => {
                cluster_removal(problem, matrix, &current, &mut rng, config.ruin_size_range)
            }
            _ => random_removal(problem, matrix, &current, &mut rng, config.ruin_size_range),
        };

        // Apply recreate
        let candidate = match recreate_op {
            "greedy_repair" => greedy_repair(problem, matrix, ruin_result),
            "regret_repair" => regret_repair(problem, matrix, ruin_result, &mut rng),
            "regret3_repair" => regret_k_repair(problem, matrix, ruin_result, &mut rng, 3),
            "regret4_repair" => regret_k_repair(problem, matrix, ruin_result, &mut rng, 4),
            _ => greedy_repair(problem, matrix, ruin_result),
        };

        let current_cost = current.total_cost_cents;
        let candidate_cost = candidate.total_cost_cents;
        let delta = candidate_cost as f64 - current_cost as f64;

        // A corrupted candidate is never accepted, no matter how cheap it
        // looks (dropping stores lowers apparent cost). Skip it entirely.
        let candidate_valid = stores_partition_valid(problem, &candidate);
        let mut accepted = false;
        if !candidate_valid {
            no_improve += 1;
        } else if delta < 0.0 {
            let candidate_clone = candidate.clone();
            current = candidate;
            accepted = true;
            if candidate_cost < best.total_cost_cents {
                best = candidate_clone.clone();
            }
            if candidate_cost < global_best.total_cost_cents {
                global_best = candidate_clone;
                no_improve = 0;
            } else {
                no_improve += 1;
            }
        } else if temperature > config.min_temperature {
            let probability = (-delta / temperature).exp();
            if rng.random::<f64>() < probability {
                current = candidate;
                accepted = true;
            }
            no_improve += 1;
        } else {
            no_improve += 1;
        }

        if config.adaptive && accepted {
            let score = if delta < 0.0 {
                if candidate_cost < global_best.total_cost_cents {
                    10.0
                } else if candidate_cost < best.total_cost_cents {
                    5.0
                } else {
                    1.0
                }
            } else {
                0.1
            };
            if let Some(stats) = ruin_weights.get_mut(ruin_op) {
                stats.score_sum += score;
                stats.usage_count += 1;
            }
            if let Some(stats) = recreate_weights.get_mut(recreate_op) {
                stats.score_sum += score;
                stats.usage_count += 1;
            }
        }

        // Update primal integral tracker (Finding 11)
        let elapsed = start_time.elapsed().as_secs_f64();
        primal_tracker.update(elapsed, current.total_cost_cents as f64);

        temperature = (temperature * config.cooling_rate).max(config.min_temperature);

        if config.adaptive && no_improve % 50 == 0 {
            update_weights(&mut ruin_weights, config.reaction_factor);
            update_weights(&mut recreate_weights, config.reaction_factor);
            apply_conflict_penalties(&mut ruin_weights);
            apply_conflict_penalties(&mut recreate_weights);
        }
    }

    let best_cost = global_best.total_cost_cents as f64;
    let final_integral = primal_tracker.normalized_integral(initial_cost, best_cost);
    eprintln!(
        "LNS final primal integral (normalized): {:.4}",
        final_integral
    );

    let solution = lns_to_solution(problem, global_best);
    if let Err(violations) = solution.validate(problem) {
        eprintln!("LNS produced invalid solution, falling back to greedy:");
        for violation in violations.iter().take(10) {
            eprintln!("  violation: {violation}");
        }
        if violations.len() > 10 {
            eprintln!("  ... and {} more", violations.len() - 10);
        }
        return plan_with_matrix(problem, matrix.clone());
    }
    solution
}

fn select_operator_weighted(
    weights: &HashMap<&'static str, OperatorStats>,
    rng: &mut StdRng,
) -> &'static str {
    let total_weight: f64 = weights.values().map(|s| s.weight).sum();
    let mut r = rng.random::<f64>() * total_weight;
    for (op, stats) in weights {
        r -= stats.weight;
        if r <= 0.0 {
            return *op;
        }
    }
    weights.keys().next().copied().unwrap()
}

fn update_weights(weights: &mut HashMap<&'static str, OperatorStats>, reaction_factor: f64) {
    for stats in weights.values_mut() {
        let avg_score = if stats.usage_count > 0 {
            stats.score_sum / stats.usage_count as f64
        } else {
            0.0
        };
        stats.weight =
            (1.0 - reaction_factor) * stats.weight + reaction_factor * avg_score.max(0.01);
        stats.score_sum = 0.0;
        stats.usage_count = 0;
    }
}

fn apply_conflict_penalties(weights: &mut HashMap<&'static str, OperatorStats>) {
    let keys: Vec<&'static str> = weights.keys().copied().collect();
    let mut adjustments: Vec<(&'static str, f64)> = Vec::new();
    for op1 in &keys {
        let conflicting: Vec<&'static str> = weights
            .get(op1)
            .map(|s| s.conflicting_ops.iter().copied().collect())
            .unwrap_or_default();
        for op2 in &conflicting {
            if let (Some(s1), Some(s2)) = (weights.get(op1), weights.get(op2)) {
                if s1.usage_count > 10 && s2.usage_count > 10 {
                    let avg1 = s1.score_sum / s1.usage_count as f64;
                    let avg2 = s2.score_sum / s2.usage_count as f64;
                    if avg1 < 1.0 && avg2 < 1.0 {
                        adjustments.push((*op1, 0.95));
                        adjustments.push((*op2, 0.95));
                    }
                }
            }
        }
    }
    for (op, factor) in adjustments {
        if let Some(stats) = weights.get_mut(op) {
            stats.weight *= factor;
        }
    }
}

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

/// Removes every occurrence of the given store ids from the solution's routes.
///
/// Matching is by store identity, never by position, so interacting removals
/// cannot shift indices and delete the wrong step (or a route's end depot).
/// Routes left with no store steps are dropped entirely. Returns the ids that
/// were actually removed, in route order.
fn remove_stores_by_id(solution: &mut LNSSolution, selected: &HashSet<StoreId>) -> Vec<StoreId> {
    if selected.is_empty() {
        return Vec::new();
    }
    let mut removed = Vec::new();
    for route in &mut solution.routes {
        let mut steps = std::mem::take(&mut route.scheduled.steps);
        steps.retain(|step| match step.node {
            Node::Store(id) if selected.contains(&id) => {
                removed.push(id);
                false
            }
            _ => true,
        });
        route.scheduled.steps = steps;
    }
    solution.routes.retain(|route| {
        route
            .scheduled
            .steps
            .iter()
            .any(|step| matches!(step.node, Node::Store(_)))
    });
    removed
}

/// True when every store in the problem appears exactly once across the
/// solution's routes and its unassigned list.
///
/// Ruin/recreate candidates are accepted only when this holds, so a corrupted
/// candidate can never displace a valid incumbent in the acceptance loop or
/// in `global_best`.
fn stores_partition_valid(problem: &Problem, solution: &LNSSolution) -> bool {
    let mut seen = vec![false; problem.stores.len()];
    let mut claim = |id: StoreId| -> bool {
        let index = id.0 as usize;
        match seen.get_mut(index) {
            Some(slot) => {
                let was_seen = *slot;
                *slot = true;
                !was_seen
            }
            None => false,
        }
    };
    for route in &solution.routes {
        for step in &route.scheduled.steps {
            if let Node::Store(id) = step.node {
                if !claim(id) {
                    return false;
                }
            }
        }
    }
    for &id in &solution.unassigned {
        if !claim(id) {
            return false;
        }
    }
    seen.iter().all(|&claimed| claimed)
}

/// -----------------------------------------------------------------------
/// Ruin operators
/// -----------------------------------------------------------------------

/// Randomly removes `k` stores from the solution.
fn random_removal(
    problem: &Problem,
    matrix: &DistanceMatrix,
    solution: &LNSSolution,
    rng: &mut StdRng,
    ruin_size_range: (usize, usize),
) -> RuinResult {
    let mut current = solution.clone();
    let num_stores = current
        .routes
        .iter()
        .map(|r| r.scheduled.steps.len().saturating_sub(2))
        .sum::<usize>();
    if num_stores == 0 {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let max_k = ruin_size_range.1.min(num_stores);
    if ruin_size_range.0 > max_k {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let k = rng.random_range(ruin_size_range.0..=max_k);
    let mut all_stores: Vec<(usize, usize, StoreId)> = Vec::new();
    for (r_idx, route) in current.routes.iter().enumerate() {
        let route_len = route.scheduled.steps.len();
        if route_len < 3 {
            continue;
        }
        for pos in 1..route_len - 1 {
            if let Node::Store(id) = route.scheduled.steps[pos].node {
                all_stores.push((r_idx, pos, id));
            }
        }
    }
    all_stores.shuffle(rng);
    let selected: HashSet<StoreId> = all_stores
        .into_iter()
        .take(k)
        .map(|(_, _, id)| id)
        .collect();
    let removed = remove_stores_by_id(&mut current, &selected);
    current.unassigned.extend(&removed);
    re_schedule_routes(problem, matrix, &mut current);
    RuinResult {
        _removed_stores: removed,
        modified_solution: current,
    }
}

/// Removes the `k` stores with highest marginal cost.
fn worst_removal(
    problem: &Problem,
    matrix: &DistanceMatrix,
    solution: &LNSSolution,
    rng: &mut StdRng,
    ruin_size_range: (usize, usize),
) -> RuinResult {
    let mut current = solution.clone();
    let mut store_costs: Vec<(i64, usize, usize, StoreId)> = Vec::new();
    for (r_idx, route) in current.routes.iter().enumerate() {
        let vehicle = &problem.vehicles[route.vehicle_index];
        let driver = route.scheduled.driver;
        let route_len = route.scheduled.steps.len();
        if route_len < 3 {
            continue;
        }
        for pos in 1..route_len - 1 {
            if let Node::Store(store_id) = route.scheduled.steps[pos].node {
                let mut without = route.scheduled.steps.clone();
                without.remove(pos);
                let without_nodes = without.iter().map(|s| s.node).collect::<Vec<_>>();
                let cost_with = route.scheduled.cost_cents;
                let cost_without = if let Some(sched) = schedule_shift_candidate(
                    problem,
                    matrix,
                    vehicle,
                    driver,
                    &without_nodes,
                    &current.driver_busy,
                    Some((driver.0 as usize, route.scheduled.span)),
                ) {
                    sched.cost_cents
                } else {
                    cost_with
                };
                let marginal = cost_with.saturating_sub(cost_without);
                store_costs.push((marginal, r_idx, pos, store_id));
            }
        }
    }
    if store_costs.is_empty() {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    store_costs.sort_by(|a, b| b.0.cmp(&a.0));
    let max_k = ruin_size_range.1.min(store_costs.len());
    if ruin_size_range.0 > max_k {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let k = rng.random_range(ruin_size_range.0..=max_k);
    let selected: HashSet<StoreId> = store_costs
        .into_iter()
        .take(k)
        .map(|(_, _, _, id)| id)
        .collect();
    let removed = remove_stores_by_id(&mut current, &selected);
    current.unassigned.extend(&removed);
    re_schedule_routes(problem, matrix, &mut current);
    RuinResult {
        _removed_stores: removed,
        modified_solution: current,
    }
}

/// Removes a seed store and its nearest neighbors (by distance/time).
fn proximity_removal(
    problem: &Problem,
    matrix: &DistanceMatrix,
    solution: &LNSSolution,
    rng: &mut StdRng,
    ruin_size_range: (usize, usize),
) -> RuinResult {
    let mut current = solution.clone();
    let mut all_stores: Vec<(usize, usize, StoreId)> = Vec::new();
    for (r_idx, route) in current.routes.iter().enumerate() {
        let route_len = route.scheduled.steps.len();
        if route_len < 3 {
            continue;
        }
        for pos in 1..route_len - 1 {
            if let Node::Store(id) = route.scheduled.steps[pos].node {
                all_stores.push((r_idx, pos, id));
            }
        }
    }
    if all_stores.is_empty() {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let max_k = ruin_size_range.1.min(all_stores.len());
    if ruin_size_range.0 > max_k {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let k = rng.random_range(ruin_size_range.0..=max_k);
    let (_, _, seed_store_id) = all_stores[rng.random_range(0..all_stores.len())];
    let mut selected: HashSet<StoreId> = HashSet::from([seed_store_id]);
    let mut neighbors: Vec<(f64, StoreId)> = Vec::new();
    for route in current.routes.iter() {
        let route_len = route.scheduled.steps.len();
        if route_len < 3 {
            continue;
        }
        for step in &route.scheduled.steps[1..route_len - 1] {
            if let Node::Store(id) = step.node {
                if id == seed_store_id {
                    continue;
                }
                if let Some(d) = matrix.distance_km(Node::Store(seed_store_id), Node::Store(id)) {
                    neighbors.push((d, id));
                }
            }
        }
    }
    neighbors.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for (_, store_id) in neighbors.into_iter().take(k.saturating_sub(1)) {
        selected.insert(store_id);
    }
    let removed = remove_stores_by_id(&mut current, &selected);
    current.unassigned.extend(&removed);
    re_schedule_routes(problem, matrix, &mut current);
    RuinResult {
        _removed_stores: removed,
        modified_solution: current,
    }
}

/// Shaw removal — removes stores based on relatedness (time window similarity,
/// location proximity, demand similarity). From Shaw (1998) and Ropke & Pisinger (2006).
fn shaw_removal(
    problem: &Problem,
    matrix: &DistanceMatrix,
    solution: &LNSSolution,
    rng: &mut StdRng,
    ruin_size_range: (usize, usize),
) -> RuinResult {
    let mut current = solution.clone();
    let mut all_stores: Vec<(usize, usize, StoreId)> = Vec::new();
    for (r_idx, route) in current.routes.iter().enumerate() {
        let route_len = route.scheduled.steps.len();
        if route_len < 3 {
            continue;
        }
        for pos in 1..route_len - 1 {
            if let Node::Store(id) = route.scheduled.steps[pos].node {
                all_stores.push((r_idx, pos, id));
            }
        }
    }
    if all_stores.is_empty() {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let max_k = ruin_size_range.1.min(all_stores.len());
    if ruin_size_range.0 > max_k {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let k = rng.random_range(ruin_size_range.0..=max_k);
    let (_, _, seed_store_id) = all_stores[rng.random_range(0..all_stores.len())];
    let mut selected: HashSet<StoreId> = HashSet::from([seed_store_id]);
    let Some(seed_store) = problem.store(seed_store_id) else {
        // Unknown seed store: remove it anyway so it is never lost silently.
        let removed = remove_stores_by_id(&mut current, &selected);
        current.unassigned.extend(&removed);
        re_schedule_routes(problem, matrix, &mut current);
        return RuinResult {
            _removed_stores: removed,
            modified_solution: current,
        };
    };
    let seed_tw_center = seed_store
        .receiving
        .iter()
        .map(|w| (w.start.minutes() + w.end.minutes()) / 2)
        .min()
        .unwrap_or(0);
    let seed_tw_width = seed_store
        .receiving
        .iter()
        .map(|w| (w.end - w.start).minutes())
        .min()
        .unwrap_or(i64::MAX);
    let _seed_loc = seed_store.location;
    let mut relatedness: Vec<(f64, StoreId)> = Vec::new();
    for route in current.routes.iter() {
        let route_len = route.scheduled.steps.len();
        if route_len < 3 {
            continue;
        }
        for step in &route.scheduled.steps[1..route_len - 1] {
            if let Node::Store(id) = step.node {
                if id == seed_store_id {
                    continue;
                }
                let Some(store) = problem.store(id) else {
                    continue;
                };
                let tw_center = store
                    .receiving
                    .iter()
                    .map(|w| (w.start.minutes() + w.end.minutes()) / 2)
                    .min()
                    .unwrap_or(0);
                let tw_width = store
                    .receiving
                    .iter()
                    .map(|w| (w.end - w.start).minutes())
                    .min()
                    .unwrap_or(i64::MAX);
                let tw_overlap =
                    if (seed_tw_center - tw_center).abs() < (seed_tw_width + tw_width) / 2 {
                        1.0
                    } else {
                        0.0
                    };
                let dist = matrix
                    .distance_km(Node::Store(seed_store_id), Node::Store(id))
                    .unwrap_or(1000.0);
                let dist_sim = 1.0 / (1.0 + dist / 100.0);
                let demand_sim = 1.0
                    - (seed_store.demand.total() as i64 - store.demand.total() as i64).abs() as f64
                        / (seed_store.demand.total() as f64 + store.demand.total() as f64).max(1.0);
                let rel = 0.4 * tw_overlap + 0.4 * dist_sim + 0.2 * demand_sim;
                relatedness.push((rel, id));
            }
        }
    }
    relatedness.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (_, store_id) in relatedness.into_iter().take(k.saturating_sub(1)) {
        selected.insert(store_id);
    }
    let removed = remove_stores_by_id(&mut current, &selected);
    current.unassigned.extend(&removed);
    re_schedule_routes(problem, matrix, &mut current);
    RuinResult {
        _removed_stores: removed,
        modified_solution: current,
    }
}

/// Time-window removal — targets stores with tight time windows (hardest to schedule).
fn time_window_removal(
    problem: &Problem,
    matrix: &DistanceMatrix,
    solution: &LNSSolution,
    rng: &mut StdRng,
    ruin_size_range: (usize, usize),
) -> RuinResult {
    let mut current = solution.clone();
    let mut store_tw_scores: Vec<(i64, usize, usize, StoreId)> = Vec::new();
    for (r_idx, route) in current.routes.iter().enumerate() {
        let route_len = route.scheduled.steps.len();
        if route_len < 3 {
            continue;
        }
        for pos in 1..route_len - 1 {
            if let Node::Store(id) = route.scheduled.steps[pos].node {
                if let Some(store) = problem.store(id) {
                    let min_width = store
                        .receiving
                        .iter()
                        .map(|w| (w.end - w.start).minutes())
                        .min()
                        .unwrap_or(i64::MAX);
                    let earliest_start = store
                        .receiving
                        .iter()
                        .map(|w| w.start.minutes())
                        .min()
                        .unwrap_or(i64::MAX);
                    let score =
                        i64::MAX.saturating_sub(min_width).min(i64::MAX / 2) + earliest_start / 10;
                    store_tw_scores.push((score, r_idx, pos, id));
                }
            }
        }
    }
    if store_tw_scores.is_empty() {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let max_k = ruin_size_range.1.min(store_tw_scores.len());
    if ruin_size_range.0 > max_k {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let k = rng.random_range(ruin_size_range.0..=max_k);
    store_tw_scores.sort_by(|a, b| b.0.cmp(&a.0));
    let selected: HashSet<StoreId> = store_tw_scores
        .into_iter()
        .take(k)
        .map(|(_, _, _, id)| id)
        .collect();
    let removed = remove_stores_by_id(&mut current, &selected);
    current.unassigned.extend(&removed);
    re_schedule_routes(problem, matrix, &mut current);
    RuinResult {
        _removed_stores: removed,
        modified_solution: current,
    }
}

/// Route removal — removes entire routes to allow major restructuring.
fn route_removal(
    problem: &Problem,
    matrix: &DistanceMatrix,
    solution: &LNSSolution,
    rng: &mut StdRng,
    ruin_size_range: (usize, usize),
) -> RuinResult {
    let mut current = solution.clone();
    if current.routes.is_empty() {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let max_routes = (ruin_size_range.1 / 3).max(1).min(current.routes.len());
    let num_routes = rng.random_range(1..=max_routes);
    let mut route_indices: Vec<usize> = (0..current.routes.len()).collect();
    route_indices.shuffle(rng);
    let mut selected: HashSet<StoreId> = HashSet::new();
    for i in 0..num_routes {
        let route_idx = route_indices[i];
        if let Some(route) = current.routes.get(route_idx) {
            selected.extend(
                route
                    .scheduled
                    .steps
                    .iter()
                    .filter_map(|step| match step.node {
                        Node::Store(id) => Some(id),
                        _ => None,
                    }),
            );
        }
    }
    let removed = remove_stores_by_id(&mut current, &selected);
    current.unassigned.extend(&removed);
    re_schedule_routes(problem, matrix, &mut current);
    RuinResult {
        _removed_stores: removed,
        modified_solution: current,
    }
}

/// Cluster-based removal — removes stores from spatially/temporally clustered routes
/// (Finding 20: Cluster-Based Combined Routing from CBCR heuristic).
fn cluster_removal(
    problem: &Problem,
    matrix: &DistanceMatrix,
    solution: &LNSSolution,
    rng: &mut StdRng,
    ruin_size_range: (usize, usize),
) -> RuinResult {
    let mut current = solution.clone();
    if current.routes.is_empty() {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let mut route_clusters: HashMap<(DepotId, i64), Vec<usize>> = HashMap::new();
    for (r_idx, route) in current.routes.iter().enumerate() {
        let vehicle = &problem.vehicles[route.vehicle_index];
        let depot = vehicle.depot;
        let avg_tw: i64 = route
            .scheduled
            .steps
            .iter()
            .filter_map(|s| {
                if let Node::Store(id) = s.node {
                    problem.store(id).map(|st| {
                        st.receiving
                            .iter()
                            .map(|w| (w.start.minutes() + w.end.minutes()) / 2)
                            .min()
                            .unwrap_or(0)
                    })
                } else {
                    None
                }
            })
            .sum::<i64>()
            / route.scheduled.steps.len().max(1) as i64;
        let cluster_key = (depot, avg_tw / 60);
        route_clusters.entry(cluster_key).or_default().push(r_idx);
    }
    if route_clusters.is_empty() {
        return RuinResult {
            _removed_stores: vec![],
            modified_solution: current,
        };
    }
    let cluster_keys: Vec<(DepotId, i64)> = route_clusters.keys().copied().collect();
    let chosen_cluster = cluster_keys[rng.random_range(0..cluster_keys.len())];
    let route_indices = route_clusters
        .get(&chosen_cluster)
        .cloned()
        .unwrap_or_default();
    let max_k = ruin_size_range.1.min(route_indices.len() * 3);
    let min_k = ruin_size_range.0.min(max_k);
    let k = rng.random_range(min_k..=max_k);
    let mut all_stores_in_cluster = Vec::new();
    for &r_idx in &route_indices {
        if r_idx >= current.routes.len() {
            continue;
        }
        let route = &current.routes[r_idx];
        for pos in 1..route.scheduled.steps.len().saturating_sub(1) {
            if let Node::Store(id) = route.scheduled.steps[pos].node {
                all_stores_in_cluster.push(id);
            }
        }
    }
    all_stores_in_cluster.shuffle(rng);
    let selected: HashSet<StoreId> = all_stores_in_cluster.into_iter().take(k).collect();
    let removed = remove_stores_by_id(&mut current, &selected);
    current.unassigned.extend(&removed);
    re_schedule_routes(problem, matrix, &mut current);
    RuinResult {
        _removed_stores: removed,
        modified_solution: current,
    }
}

/// -----------------------------------------------------------------------
/// Repair operators
/// -----------------------------------------------------------------------

/// Repairs by greedily inserting unassigned stores (sorted by time-window tightness).
fn greedy_repair(
    problem: &Problem,
    matrix: &DistanceMatrix,
    ruin_result: RuinResult,
) -> LNSSolution {
    let mut current = ruin_result.modified_solution;
    let unassigned = current.unassigned.clone();
    current.unassigned.clear();

    let mut to_insert: Vec<StoreId> = unassigned;
    to_insert.sort_by(|a, b| {
        let store_a = problem.store(*a);
        let store_b = problem.store(*b);
        let tw_a = store_a.map(|s| {
            let min_width = s
                .receiving
                .iter()
                .map(|w| (w.end - w.start).minutes())
                .min()
                .unwrap_or(i64::MAX);
            let earliest = s
                .receiving
                .iter()
                .map(|w| w.start.minutes())
                .min()
                .unwrap_or(i64::MAX);
            (min_width, earliest)
        });
        let tw_b = store_b.map(|s| {
            let min_width = s
                .receiving
                .iter()
                .map(|w| (w.end - w.start).minutes())
                .min()
                .unwrap_or(i64::MAX);
            let earliest = s
                .receiving
                .iter()
                .map(|w| w.start.minutes())
                .min()
                .unwrap_or(i64::MAX);
            (min_width, earliest)
        });
        tw_a.cmp(&tw_b)
    });

    for store_id in to_insert {
        if let Some(insertion) = best_insertion_lns(problem, matrix, &current, store_id) {
            apply_insertion(&mut current, problem, matrix, insertion);
        } else {
            current.unassigned.push(store_id);
        }
    }
    current
}

/// Repairs using regret-2 heuristic (considers opportunity cost of not inserting).
fn regret_repair(
    problem: &Problem,
    matrix: &DistanceMatrix,
    ruin_result: RuinResult,
    _rng: &mut StdRng,
) -> LNSSolution {
    regret_k_repair(problem, matrix, ruin_result, _rng, 2)
}

/// Generalized regret-k repair — considers k best insertions and picks the one
/// with maximum regret. Uses probabilistic selection proportional to regret
/// value (Finding 4/6).
fn regret_k_repair(
    problem: &Problem,
    matrix: &DistanceMatrix,
    ruin_result: RuinResult,
    rng: &mut StdRng,
    k: usize,
) -> LNSSolution {
    let mut current = ruin_result.modified_solution;
    let mut unassigned = current.unassigned.clone();
    current.unassigned.clear();

    while !unassigned.is_empty() {
        let mut candidates = Vec::new();
        for &store_id in &unassigned {
            let mut insertions = all_insertions_lns(problem, matrix, &current, store_id);
            if insertions.is_empty() {
                continue;
            }
            insertions.sort_by_key(|ins| ins.cost_cents);
            let regret = if insertions.len() >= k {
                insertions[k - 1]
                    .cost_cents
                    .saturating_sub(insertions[0].cost_cents)
            } else if insertions.len() >= 2 {
                insertions[1]
                    .cost_cents
                    .saturating_sub(insertions[0].cost_cents)
            } else {
                insertions[0].cost_cents
            };
            candidates.push((regret, store_id, insertions[0].clone()));
        }
        if candidates.is_empty() {
            current.unassigned.extend(unassigned.drain(..));
            break;
        }
        // Probabilistic selection proportional to regret value (Finding 4/6)
        let total_regret: i64 = candidates.iter().map(|(r, _, _)| *r).sum();
        let mut selected_idx = None;
        if total_regret > 0 {
            let mut r = rng.random_range(0..=total_regret);
            for (idx, (regret, _, _)) in candidates.iter().enumerate() {
                if r <= *regret {
                    selected_idx = Some(idx);
                    break;
                }
                r = r.saturating_sub(*regret);
            }
        }
        if selected_idx.is_none() {
            selected_idx = candidates
                .iter()
                .enumerate()
                .max_by_key(|(_, (r, _, _))| *r)
                .map(|(i, _)| i);
        }
        if let Some(idx) = selected_idx {
            let (_, store_id, insertion) = candidates.swap_remove(idx);
            apply_insertion(&mut current, problem, matrix, insertion);
            unassigned.retain(|id| *id != store_id);
        } else {
            current.unassigned.extend(unassigned.drain(..));
            break;
        }
    }
    current
}

/// -----------------------------------------------------------------------
/// Repair utilities
/// -----------------------------------------------------------------------

/// Re-schedules all routes in the solution after ruin.
fn re_schedule_routes(problem: &Problem, matrix: &DistanceMatrix, solution: &mut LNSSolution) {
    let mut new_driver_busy = vec![Vec::new(); problem.drivers.len()];
    let driver_indices = driver_order(problem);
    let mut new_routes = Vec::new();
    for route in solution.routes.drain(..) {
        let vehicle = &problem.vehicles[route.vehicle_index];
        let seq: Vec<Node> = route.scheduled.steps.iter().map(|s| s.node).collect();
        if let Some(sched) = greedy_schedule_route(
            problem,
            matrix,
            &driver_indices,
            vehicle,
            &seq,
            &new_driver_busy,
            None,
        ) {
            let idx = sched.driver.0 as usize;
            new_driver_busy[idx].push(sched.span);
            new_routes.push(LNSRoute {
                vehicle_index: route.vehicle_index,
                scheduled: sched,
            });
        } else {
            for step in &route.scheduled.steps {
                if let Node::Store(id) = step.node {
                    solution.unassigned.push(id);
                }
            }
        }
    }
    solution.routes = new_routes;
    solution.driver_busy = new_driver_busy;
    solution.total_cost_cents = solution.routes.iter().map(|r| r.scheduled.cost_cents).sum();
}

/// Finds the best insertion for a store (similar to greedy).
fn best_insertion_lns(
    problem: &Problem,
    matrix: &DistanceMatrix,
    solution: &LNSSolution,
    store_id: StoreId,
) -> Option<LNSInsertion> {
    let _store = problem.store(store_id)?;
    let node = Node::Store(store_id);
    let mut best: Option<LNSInsertion> = None;
    let driver_indices = driver_order(problem);
    for (route_idx, route) in solution.routes.iter().enumerate() {
        let vehicle = &problem.vehicles[route.vehicle_index];
        let exempt = Some((route.scheduled.driver.0 as usize, route.scheduled.span));
        let seq: Vec<Node> = route.scheduled.steps.iter().map(|s| s.node).collect();
        let sl = seq.len();
        if sl < 2 {
            continue;
        }
        for pos in 1..sl {
            let mut cand = seq.clone();
            cand.insert(pos, node);
            if let Some(sched) = greedy_schedule_route(
                problem,
                matrix,
                &driver_indices,
                vehicle,
                &cand,
                &solution.driver_busy,
                exempt,
            ) {
                let base_cost = sched.cost_cents;
                // Fleet-aware cost (Finding 10): penalize opening a new vehicle,
                // not using the last existing route.
                let fleet_penalty = if route_idx == solution.routes.len() {
                    5_000
                } else {
                    0
                };
                let cost = base_cost + fleet_penalty;
                let ins = LNSInsertion {
                    route_idx,
                    _pos: pos,
                    vehicle_index: route.vehicle_index,
                    scheduled: sched.clone(),
                    cost_cents: cost,
                };
                if best.as_ref().is_none_or(|b| ins.cost_cents < b.cost_cents) {
                    best = Some(ins);
                }
            }
        }
    }
    let used_vehicles: HashSet<usize> = solution.routes.iter().map(|r| r.vehicle_index).collect();
    for (vehicle_index, vehicle) in problem.vehicles.iter().enumerate() {
        if used_vehicles.contains(&vehicle_index) {
            continue;
        }
        let home = Node::Depot(vehicle.depot);
        let cand = [home, node, home];
        if let Some(sched) = greedy_schedule_route(
            problem,
            matrix,
            &driver_indices,
            vehicle,
            &cand,
            &solution.driver_busy,
            None,
        ) {
            let ins = LNSInsertion {
                route_idx: solution.routes.len(),
                _pos: 1,
                vehicle_index,
                scheduled: sched.clone(),
                // New vehicle for the restricted candidate: penalise using one.
                cost_cents: sched.cost_cents + 5_000,
            };
            if best.as_ref().is_none_or(|b| ins.cost_cents < b.cost_cents) {
                best = Some(ins);
            }
        }
    }
    best
}

/// All feasible insertions for a store (for regret).
fn all_insertions_lns(
    problem: &Problem,
    matrix: &DistanceMatrix,
    solution: &LNSSolution,
    store_id: StoreId,
) -> Vec<LNSInsertion> {
    let mut insertions = Vec::new();
    let Some(_store) = problem.store(store_id) else {
        return insertions;
    };
    let node = Node::Store(store_id);
    let driver_indices = driver_order(problem);
    for (route_idx, route) in solution.routes.iter().enumerate() {
        let vehicle = &problem.vehicles[route.vehicle_index];
        let exempt = Some((route.scheduled.driver.0 as usize, route.scheduled.span));
        let seq: Vec<Node> = route.scheduled.steps.iter().map(|s| s.node).collect();
        let sl = seq.len();
        if sl < 2 {
            continue;
        }
        // Graph sparsification: quick time-window compatibility check (Finding 19)
        let store = problem.store(store_id).unwrap();
        let earliest_arrival = route
            .scheduled
            .steps
            .last()
            .map(|s| s.arrive)
            .unwrap_or(Minutes::ZERO);
        if !store
            .receiving
            .iter()
            .any(|tw| tw.start <= earliest_arrival && earliest_arrival < tw.end)
        {
            continue;
        }
        for pos in 1..sl {
            let mut cand = seq.clone();
            cand.insert(pos, node);
            if let Some(sched) = greedy_schedule_route(
                problem,
                matrix,
                &driver_indices,
                vehicle,
                &cand,
                &solution.driver_busy,
                exempt,
            ) {
                let cost = sched.cost_cents;
                insertions.push(LNSInsertion {
                    route_idx,
                    _pos: pos,
                    vehicle_index: route.vehicle_index,
                    scheduled: sched.clone(),
                    cost_cents: cost,
                });
            }
        }
    }
    let used_vehicles: HashSet<usize> = solution.routes.iter().map(|r| r.vehicle_index).collect();
    for (vehicle_index, vehicle) in problem.vehicles.iter().enumerate() {
        if used_vehicles.contains(&vehicle_index) {
            continue;
        }
        let home = Node::Depot(vehicle.depot);
        let cand = [home, node, home];
        if let Some(sched) = greedy_schedule_route(
            problem,
            matrix,
            &driver_indices,
            vehicle,
            &cand,
            &solution.driver_busy,
            None,
        ) {
            insertions.push(LNSInsertion {
                route_idx: solution.routes.len(),
                _pos: 1,
                vehicle_index,
                scheduled: sched.clone(),
                cost_cents: sched.cost_cents + 5_000,
            });
        }
    }
    insertions
}

/// Applies an insertion to the solution.
fn apply_insertion(
    solution: &mut LNSSolution,
    _problem: &Problem,
    _matrix: &DistanceMatrix,
    insertion: LNSInsertion,
) {
    let vi = insertion.vehicle_index;
    let sched = insertion.scheduled;
    let d_idx = sched.driver.0 as usize;
    let span = sched.span;
    if insertion.route_idx >= solution.routes.len() {
        solution.routes.push(LNSRoute {
            vehicle_index: vi,
            scheduled: sched,
        });
        solution.driver_busy[d_idx].push(span);
    } else {
        let old_d = solution.routes[insertion.route_idx].scheduled.driver.0 as usize;
        let old_s = solution.routes[insertion.route_idx].scheduled.span;
        if let Some(spans) = solution.driver_busy.get_mut(old_d) {
            if let Some(p) = spans.iter().position(|&s| s == old_s) {
                spans.remove(p);
            }
        }
        solution.driver_busy[d_idx].push(span);
        solution.routes[insertion.route_idx].scheduled = sched;
    }
    solution.total_cost_cents = solution.routes.iter().map(|r| r.scheduled.cost_cents).sum();
}

/// -----------------------------------------------------------------------
/// Internal helpers
/// -----------------------------------------------------------------------

fn driver_order(problem: &Problem) -> Vec<usize> {
    let mut o: Vec<usize> = (0..problem.drivers.len()).collect();
    o.sort_by_key(|&i| (problem.drivers[i].id, i));
    o
}

/// Wraps greedy's schedule_shift for worst_removal's marginal-cost calculation.
fn schedule_shift_candidate(
    problem: &Problem,
    matrix: &DistanceMatrix,
    vehicle: &Vehicle,
    driver: DriverId,
    sequence: &[Node],
    driver_busy: &[Vec<(Minutes, Minutes)>],
    exempt: Option<(usize, (Minutes, Minutes))>,
) -> Option<Scheduled> {
    let driver_obj = problem.drivers.iter().find(|d| d.id == driver)?;
    let busy = driver_busy
        .get(driver.0 as usize)
        .map_or(&[][..], Vec::as_slice);
    for &shift in &driver_obj.shifts {
        let Some(sched) = greedy_schedule_shift(problem, matrix, vehicle, driver, shift, sequence)
        else {
            continue;
        };
        let blocked = busy.iter().any(|&span| {
            spans_overlap(span, sched.span) && Some((driver.0 as usize, span)) != exempt
        });
        if !blocked {
            return Some(sched);
        }
    }
    None
}

/// Converts a public Solution to LNS internal representation.
fn greedy_to_lns(problem: &Problem, solution: Solution) -> Option<LNSSolution> {
    let mut total_cost = 0i64;
    let mut driver_busy: Vec<Vec<(Minutes, Minutes)>> = vec![Vec::new(); problem.drivers.len()];
    let mut routes = Vec::new();
    for route in solution.routes {
        let Some(vehicle_index) = problem.vehicles.iter().position(|v| v.id == route.vehicle)
        else {
            return None;
        };
        total_cost += route.cost_cents;
        let Some(driver) = route.drivers.first().copied() else {
            eprintln!("greedy_to_lns: route {} has no drivers", route.vehicle);
            return None;
        };
        let Some(last_step) = route.steps.last() else {
            eprintln!("greedy_to_lns: route {} has no steps", route.vehicle);
            return None;
        };
        let span = (route.steps[0].depart, last_step.arrive);
        driver_busy[driver.0 as usize].push(span);
        routes.push(LNSRoute {
            vehicle_index,
            scheduled: Scheduled {
                driver,
                span,
                steps: route.steps,
                distance_km: route.distance_km,
                duration: route.duration,
                cost_cents: route.cost_cents,
            },
        });
    }
    Some(LNSSolution {
        routes,
        unassigned: solution.unassigned,
        total_cost_cents: total_cost,
        driver_busy,
    })
}

/// Converts LNS solution back to public Solution.
fn lns_to_solution(problem: &Problem, lns: LNSSolution) -> Solution {
    let routes: Vec<RoutePlan> = lns
        .routes
        .into_iter()
        .filter(|r| r.scheduled.steps.len() > 2) // Must have at least depot -> store -> depot
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
    let dist: f64 = routes.iter().map(|r| r.distance_km).sum();
    let dur = routes.iter().map(|r| r.duration.minutes()).sum();
    let cost = routes.iter().map(|r| r.cost_cents).sum();
    let mut unassigned = lns.unassigned;
    unassigned.sort_unstable();
    Solution {
        routes,
        unassigned,
        total_distance_km: dist,
        total_duration: Minutes::from_minutes(dur),
        total_cost_cents: cost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::DistanceMatrix;
    use domain::{
        Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
        LatLon, Load, Minutes, Node, Problem, Shift, Store, StoreId, TimeWindow, Vehicle,
        VehicleId,
    };

    fn small_problem() -> Problem {
        Problem {
            name: "small metro".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Sydney DC".into(),
                location: LatLon::new(-33.87, 151.21),
                timezone: None,
            }],
            stores: vec![
                Store {
                    id: StoreId(0),
                    chain: Chain::Coles,
                    name: "Coles Inner West".into(),
                    location: LatLon::new(-33.92, 151.23),
                    timezone: None,
                    demand: Load::new(2, 1, 0),
                    service: Minutes::from_minutes(15),
                    receiving: vec![TimeWindow::new(
                        Minutes::from_minutes(480),
                        Minutes::from_minutes(720),
                    )],
                },
                Store {
                    id: StoreId(1),
                    chain: Chain::Woolworths,
                    name: "Woolworths North".into(),
                    location: LatLon::new(-33.80, 151.15),
                    timezone: None,
                    demand: Load::new(1, 2, 1),
                    service: Minutes::from_minutes(10),
                    receiving: vec![TimeWindow::new(
                        Minutes::from_minutes(300),
                        Minutes::from_minutes(600),
                    )],
                },
                Store {
                    id: StoreId(2),
                    chain: Chain::Aldi,
                    name: "ALDI Hills".into(),
                    location: LatLon::new(-33.75, 151.30),
                    timezone: None,
                    demand: Load::new(0, 1, 0),
                    service: Minutes::from_minutes(20),
                    receiving: vec![TimeWindow::new(
                        Minutes::from_minutes(540),
                        Minutes::from_minutes(900),
                    )],
                },
            ],
            vehicles: vec![
                Vehicle {
                    id: VehicleId(0),
                    name: "Rigid 1".into(),
                    depot: DepotId(0),
                    capacity: Load::new(8, 8, 4),
                    max_duration: Minutes::from_minutes(600),
                    avg_speed_kmh: 60.0,
                },
                Vehicle {
                    id: VehicleId(1),
                    name: "Rigid 2".into(),
                    depot: DepotId(0),
                    capacity: Load::new(4, 4, 2),
                    max_duration: Minutes::from_minutes(480),
                    avg_speed_kmh: 75.0,
                },
            ],
            drivers: vec![Driver {
                id: DriverId(0),
                name: "Sam".into(),
                shifts: vec![Shift::new(
                    Minutes::from_minutes(300),
                    Minutes::from_minutes(1200),
                )],
                fatigue_rules: FatigueRules::default(),
            }],
            cost: CostModel {
                fixed_per_route_cents: 15_000,
                per_km_cents: 90,
                per_hour_cents: 4_500,
                late_per_min_cents: 0,
            },
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
        }
    }

    fn lns_solution(problem: &Problem, matrix: &DistanceMatrix) -> LNSSolution {
        let greedy = crate::greedy::plan_with_matrix(problem, matrix.clone());
        greedy_to_lns(problem, greedy).expect("greedy_to_lns should succeed")
    }

    #[test]
    fn random_removal_removes_exactly_k_stores() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        let total_stores: usize = solution
            .routes
            .iter()
            .map(|r| r.scheduled.steps.len().saturating_sub(2))
            .sum();
        assert!(total_stores > 0, "fixture should have stores to remove");

        let mut rng = StdRng::seed_from_u64(42);
        let result = random_removal(&problem, &matrix, &solution, &mut rng, (2, 3));

        assert!(!result._removed_stores.is_empty(), "should remove stores");
        assert!(
            result._removed_stores.len() >= 2 && result._removed_stores.len() <= 3,
            "should remove between 2 and 3 stores, got {}",
            result._removed_stores.len()
        );
        assert!(
            result.modified_solution.unassigned.len() >= 2,
            "unassigned should grow"
        );
    }

    #[test]
    fn random_removal_no_stores_is_noop() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let empty = LNSSolution {
            routes: vec![],
            unassigned: vec![],
            total_cost_cents: 0,
            driver_busy: vec![Vec::new(); problem.drivers.len()],
        };
        let mut rng = StdRng::seed_from_u64(42);
        let result = random_removal(&problem, &matrix, &empty, &mut rng, (2, 5));
        assert!(result._removed_stores.is_empty());
        assert!(result.modified_solution.routes.is_empty());
    }

    #[test]
    fn worst_removal_removes_highest_marginal_stores() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        let total_stores: usize = solution
            .routes
            .iter()
            .map(|r| r.scheduled.steps.len().saturating_sub(2))
            .sum();
        assert!(total_stores > 0);

        let mut rng = StdRng::seed_from_u64(42);
        let result = worst_removal(&problem, &matrix, &solution, &mut rng, (1, 2));
        assert!(!result._removed_stores.is_empty());
        assert!(
            result.modified_solution.unassigned.len() >= 1,
            "unassigned should grow"
        );
    }

    #[test]
    fn proximity_removal_removes_seed_and_neighbors() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        let total_stores: usize = solution
            .routes
            .iter()
            .map(|r| r.scheduled.steps.len().saturating_sub(2))
            .sum();
        assert!(total_stores > 1);

        let mut rng = StdRng::seed_from_u64(42);
        let result = proximity_removal(&problem, &matrix, &solution, &mut rng, (2, 4));
        assert!(
            result._removed_stores.len() >= 2,
            "should remove seed plus at least one neighbor"
        );
    }

    #[test]
    fn shaw_removal_removes_related_stores() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        let total_stores: usize = solution
            .routes
            .iter()
            .map(|r| r.scheduled.steps.len().saturating_sub(2))
            .sum();
        assert!(total_stores > 1);

        let mut rng = StdRng::seed_from_u64(42);
        let result = shaw_removal(&problem, &matrix, &solution, &mut rng, (2, 4));
        assert!(!result._removed_stores.is_empty());
    }

    #[test]
    fn time_window_removal_removes_tight_window_stores() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        let total_stores: usize = solution
            .routes
            .iter()
            .map(|r| r.scheduled.steps.len().saturating_sub(2))
            .sum();
        assert!(total_stores > 0);

        let mut rng = StdRng::seed_from_u64(42);
        let result = time_window_removal(&problem, &matrix, &solution, &mut rng, (1, 3));
        assert!(!result._removed_stores.is_empty());
    }

    #[test]
    fn route_removal_clears_entire_routes() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        assert!(!solution.routes.is_empty());

        let mut rng = StdRng::seed_from_u64(42);
        let result = route_removal(&problem, &matrix, &solution, &mut rng, (3, 6));
        assert!(!result._removed_stores.is_empty());
        // At least one route should be cleared
        assert!(
            result.modified_solution.routes.iter().any(|r| {
                r.scheduled
                    .steps
                    .iter()
                    .all(|s| matches!(s.node, Node::Depot(_)))
            }) || !result.modified_solution.unassigned.is_empty()
        );
    }

    #[test]
    fn cluster_removal_groups_by_depot_and_time() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        let total_stores: usize = solution
            .routes
            .iter()
            .map(|r| r.scheduled.steps.len().saturating_sub(2))
            .sum();
        assert!(total_stores > 0);

        let mut rng = StdRng::seed_from_u64(42);
        let result = cluster_removal(&problem, &matrix, &solution, &mut rng, (2, 4));
        assert!(!result._removed_stores.is_empty());
    }

    #[test]
    fn greedy_to_lns_preserves_all_stores() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        let total_stores: usize = solution
            .routes
            .iter()
            .map(|r| r.scheduled.steps.len().saturating_sub(2))
            .sum();
        assert_eq!(
            total_stores,
            problem.stores.len(),
            "all stores should be in routes after conversion"
        );
    }

    #[test]
    fn lns_to_solution_roundtrip_is_valid() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        let converted = lns_to_solution(&problem, solution);
        assert_eq!(
            converted.validate(&problem),
            Ok(()),
            "round-tripped solution should be valid"
        );
    }

    #[test]
    fn stores_partition_valid_accepts_intact_solutions() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let solution = lns_solution(&problem, &matrix);
        assert!(stores_partition_valid(&problem, &solution));
    }

    #[test]
    fn stores_partition_valid_rejects_dropped_store() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let mut solution = lns_solution(&problem, &matrix);
        // Delete a store from a route without listing it as unassigned —
        // exactly the corruption class the acceptance gate must reject.
        let mut dropped = false;
        for route in &mut solution.routes {
            let before = route.scheduled.steps.len();
            route
                .scheduled
                .steps
                .retain(|step| !matches!(step.node, Node::Store(_)));
            if route.scheduled.steps.len() < before {
                dropped = true;
                break;
            }
        }
        assert!(dropped, "fixture should have a store to drop");
        assert!(!stores_partition_valid(&problem, &solution));
    }

    #[test]
    fn stores_partition_valid_rejects_duplicate_store() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let mut solution = lns_solution(&problem, &matrix);
        let duplicate = solution
            .routes
            .iter()
            .flat_map(|route| route.scheduled.steps.iter())
            .find(|step| matches!(step.node, Node::Store(_)))
            .copied()
            .expect("fixture should have a store step");
        solution.routes[0].scheduled.steps.insert(1, duplicate);
        assert!(!stores_partition_valid(&problem, &solution));
    }

    #[test]
    fn remove_stores_by_id_removes_exactly_the_selection_and_drops_empty_routes() {
        let problem = small_problem();
        let matrix = DistanceMatrix::new(&problem);
        let mut solution = lns_solution(&problem, &matrix);
        let selected: HashSet<StoreId> = solution
            .routes
            .iter()
            .flat_map(|route| route.scheduled.steps.iter())
            .filter_map(|step| match step.node {
                Node::Store(id) => Some(id),
                _ => None,
            })
            .collect();
        assert!(!selected.is_empty());

        let removed = remove_stores_by_id(&mut solution, &selected);
        assert_eq!(removed.len(), selected.len());
        for id in &removed {
            assert!(selected.contains(id));
        }
        // Every route that lost its stores is dropped entirely: no
        // depot-only husks are left behind, and no store remains in a route.
        // (Moving `removed` into `unassigned` is the caller's job.)
        assert!(solution.routes.iter().all(|route| {
            route
                .scheduled
                .steps
                .iter()
                .any(|step| matches!(step.node, Node::Store(_)))
        }));
    }
}
