//! Route construction and, later, metaheuristic optimisation.
//!
//! M0 ships a deterministic greedy insertion heuristic on top of a haversine
//! distance matrix. Road-network durations (OSM) replace the distance/speed
//! approximation in M3 of the roadmap.
//!
//! # Algorithm (M0)
//!
//! 1. Stores are offered to the greedy ordered by receiving window end, then
//!    total demand, then id.
//! 2. Each store is evaluated against every interior position of every
//!    existing route and against a fresh route with every unused vehicle, and
//!    the cheapest feasible placement wins; ties break by
//!    `(cost, vehicle id, insertion index)`.
//! 3. A candidate route is scheduled by trying drivers in id order and their
//!    shifts in order, taking the first pair that schedules feasibly. Each
//!    route therefore uses exactly one driver and exactly one shift: the
//!    overnight relay hand-overs of M5 are out of scope.
//! 4. A store with no feasible placement is reported in
//!    [`domain::Solution::unassigned`] rather than dropped.
//!
//! Travel times are `(km / avg_speed_kmh * 60).round()` whole minutes; a
//! vehicle whose `avg_speed_kmh` is not positive plans at
//! [`FALLBACK_SPEED_KMH`]. Every step of the search is deterministic: the
//! same problem always yields the same plan.

pub mod curfew;
pub mod greedy;
pub mod lns;
pub mod local_search;
mod matrix;
pub mod parser;
mod savings; // Solomon/VRPLib parser (vendored for M1)

pub use curfew::{BreachKind, CurfewDrop, CurfewReport, LegTimes, StaticLegs, enforce_curfews};
pub use lns::{LNSConfig, solve_lns};
pub use local_search::{LSConfig, improve};
pub use matrix::{
    DistanceMatrix, FALLBACK_SPEED_KMH, euclidean_km, euclidean_matrix, travel_minutes,
};
pub use parser::{SolomonInstance, parse_solomon_file, solomon_to_problem};
#[allow(unused_imports)]
pub use routing::{CachedMatrix, MatrixStrategy, TruckProfile};

use anyhow;
use domain::{Problem, Solution};

/// Solves `problem` and returns a validated plan using greedy insertion (M0/M1).
///
/// The returned solution is guaranteed to satisfy
/// [`domain::Solution::validate`]; stores that cannot be served within every
/// constraint are returned in [`domain::Solution::unassigned`]. Running the
/// solver twice on the same problem returns the same plan (deterministic).
pub fn solve(problem: &Problem) -> Solution {
    let solution = greedy::plan(problem);
    // Runtime validation (not debug_assert!): must hold in release builds too.
    assert!(
        solution.validate(problem).is_ok(),
        "solver produced an invalid solution"
    );
    solution
}

/// Runs the M0 greedy insertion solver (for comparison/benchmarking).
pub fn solve_greedy(problem: &Problem) -> Solution {
    let solution = greedy::plan(problem);
    assert!(
        solution.validate(problem).is_ok(),
        "solver produced an invalid solution"
    );
    solution
}

/// Runs the greedy solver with a Euclidean distance matrix (for Solomon calibration).
pub fn solve_greedy_euclidean(problem: &Problem) -> Solution {
    let solution = greedy::plan_with_matrix(problem, matrix::euclidean_matrix(problem));
    if let Err(violations) = solution.validate(problem) {
        eprintln!("Calibration validation failed:");
        for v in &violations {
            eprintln!("  - {v}");
        }
    }
    assert!(
        solution.validate(problem).is_ok(),
        "solver produced an invalid solution"
    );
    solution
}

/// Solves `problem` using greedy + local search (M2).
///
/// Runs the greedy insertion heuristic first, then applies local search
/// improvements (2-opt, relocate, exchange, cross-exchange) with multi-start.
/// Uses the provided distance matrix (e.g., haversine or Euclidean).
/// The returned solution is guaranteed to satisfy
/// [`domain::Solution::validate`].
pub fn solve_with_local_search(
    problem: &Problem,
    config: LSConfig,
    matrix: DistanceMatrix,
) -> Solution {
    let greedy_solution = greedy::plan_with_matrix(problem, matrix.clone());
    let solution = local_search::improve(problem, &matrix, greedy_solution, config);
    assert!(
        solution.validate(problem).is_ok(),
        "solver produced an invalid solution"
    );
    solution
}

/// Convenience wrapper that builds a haversine distance matrix.
pub fn solve_with_local_search_haversine(problem: &Problem, config: LSConfig) -> Solution {
    let matrix = DistanceMatrix::new(problem);
    solve_with_local_search(problem, config, matrix)
}

/// Solves `problem` using greedy + LNS/ALNS (M2).
///
/// Runs the greedy insertion heuristic first, then applies Large Neighborhood Search
/// with adaptive operator selection to escape local optima. Uses the provided
/// distance matrix. The returned solution is guaranteed to satisfy
/// [`domain::Solution::validate`].
pub fn solve_with_lns(problem: &Problem, config: LNSConfig, matrix: DistanceMatrix) -> Solution {
    let greedy_solution = greedy::plan_with_matrix(problem, matrix.clone());
    let solution = lns::solve_lns(problem, &matrix, greedy_solution, config);
    assert!(
        solution.validate(problem).is_ok(),
        "solver produced an invalid solution"
    );
    solution
}

/// Convenience wrapper that builds a haversine distance matrix.
pub fn solve_with_lns_haversine(problem: &Problem, config: LNSConfig) -> Solution {
    let matrix = DistanceMatrix::new(problem);
    solve_with_lns(problem, config, matrix)
}

/// Solves `problem` using greedy + LNS/ALNS with road-network distances and
/// durations from an OSM PBF extract.
///
/// Loads the PBF at `pbf_path` into a [`CachedMatrix`], builds a
/// [`DistanceMatrix`] from the road-network distances/durations for the given
/// `profile`, then runs LNS/ALNS. Returns an error if the PBF cannot be read
/// or no graph is loaded.
pub fn solve_with_lns_routing(
    problem: &Problem,
    config: LNSConfig,
    cached_matrix: &mut CachedMatrix,
    profile: TruckProfile,
) -> anyhow::Result<Solution> {
    solve_with_lns_routing_with(
        problem,
        config,
        cached_matrix,
        profile,
        MatrixStrategy::Dijkstra,
    )
}

/// Builds the road-network distance matrix for one solve call, naming the
/// missing graph in the error.
fn routing_matrix(
    problem: &Problem,
    cached_matrix: &mut CachedMatrix,
    profile: TruckProfile,
    strategy: MatrixStrategy,
) -> anyhow::Result<DistanceMatrix> {
    let entries = cached_matrix
        .matrix_for_with(problem, profile, strategy)?
        .ok_or_else(|| anyhow::anyhow!("no OSM graph loaded"))?;
    let node_count = problem.depots.len() + problem.stores.len();
    Ok(DistanceMatrix::from_routing(
        problem.depots.len(),
        node_count,
        &entries,
    ))
}

/// [`solve_with_lns_routing`] with an explicit matrix strategy; see
/// [`routing::MatrixStrategy`].
pub fn solve_with_lns_routing_with(
    problem: &Problem,
    config: LNSConfig,
    cached_matrix: &mut CachedMatrix,
    profile: TruckProfile,
    strategy: MatrixStrategy,
) -> anyhow::Result<Solution> {
    let matrix = routing_matrix(problem, cached_matrix, profile, strategy)?;
    let greedy_solution = greedy::plan_with_matrix(problem, matrix.clone());
    let solution = lns::solve_lns(problem, &matrix, greedy_solution, config);
    assert!(
        solution.validate(problem).is_ok(),
        "solver produced an invalid solution"
    );
    Ok(solution)
}

/// Runs `repeats` independent LNS runs over the same greedy start in
/// parallel and keeps the best plan by `(fewest unassigned, total distance,
/// route count)`.
///
/// Each run derives a distinct seed from `config.seed` (or the problem hash
/// when it is zero); run 0 uses the base seed itself, so a single-repeat
/// call matches [`solve_with_lns`] exactly. Ties prefer the earliest run and
/// threads are joined in order, so scheduling cannot change the selected
/// plan, which makes the outcome reproducible.
pub fn solve_with_lns_multistart(
    problem: &Problem,
    config: LNSConfig,
    matrix: DistanceMatrix,
    repeats: usize,
) -> Solution {
    let repeats = repeats.max(1);
    let greedy_solution = greedy::plan_with_matrix(problem, matrix.clone());
    let base_seed = lns::derived_seed(problem, config.seed);
    let results: Vec<Solution> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..repeats)
            .map(|index| {
                let mut run_config = config.clone();
                // Run 0 keeps the base seed; later runs extend it. (When the
                // hash-based base seed is zero, fall back to small non-zero
                // seeds.)
                run_config.seed = if base_seed == 0 {
                    1 + index as u64
                } else {
                    base_seed.wrapping_add(index as u64)
                };
                let matrix_ref = &matrix;
                let greedy_ref = &greedy_solution;
                scope.spawn(move || {
                    lns::solve_lns(problem, matrix_ref, greedy_ref.clone(), run_config)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("LNS worker panicked"))
            .collect()
    });
    let solution = results
        .into_iter()
        .min_by(|a, b| {
            a.unassigned
                .len()
                .cmp(&b.unassigned.len())
                .then_with(|| a.total_distance_km.total_cmp(&b.total_distance_km))
                .then_with(|| a.routes.len().cmp(&b.routes.len()))
        })
        .expect("repeats is at least one");
    assert!(
        solution.validate(problem).is_ok(),
        "solver produced an invalid solution"
    );
    solution
}

/// [`solve_with_lns_routing`] with multi-start: runs `repeats` LNS runs over
/// the road-network matrix and keeps the best plan.
pub fn solve_with_lns_routing_multistart(
    problem: &Problem,
    config: LNSConfig,
    cached_matrix: &mut CachedMatrix,
    profile: TruckProfile,
    repeats: usize,
) -> anyhow::Result<Solution> {
    solve_with_lns_routing_multistart_with(
        problem,
        config,
        cached_matrix,
        profile,
        MatrixStrategy::Dijkstra,
        repeats,
    )
}

/// [`solve_with_lns_routing_multistart`] with an explicit matrix strategy.
pub fn solve_with_lns_routing_multistart_with(
    problem: &Problem,
    config: LNSConfig,
    cached_matrix: &mut CachedMatrix,
    profile: TruckProfile,
    strategy: MatrixStrategy,
    repeats: usize,
) -> anyhow::Result<Solution> {
    let matrix = routing_matrix(problem, cached_matrix, profile, strategy)?;
    Ok(solve_with_lns_multistart(problem, config, matrix, repeats))
}

/// Solves `problem` using greedy insertion (M0) with road-network distances
/// and durations from an OSM PBF extract.
///
/// Loads the PBF at `pbf_path` into a [`CachedMatrix`], builds a
/// [`DistanceMatrix`] from the road-network distances/durations for the given
/// `profile`, then runs the greedy insertion heuristic.
pub fn solve_greedy_routing(
    problem: &Problem,
    cached_matrix: &mut CachedMatrix,
    profile: TruckProfile,
) -> anyhow::Result<Solution> {
    solve_greedy_routing_with(problem, cached_matrix, profile, MatrixStrategy::Dijkstra)
}

/// [`solve_greedy_routing`] with an explicit matrix strategy.
pub fn solve_greedy_routing_with(
    problem: &Problem,
    cached_matrix: &mut CachedMatrix,
    profile: TruckProfile,
    strategy: MatrixStrategy,
) -> anyhow::Result<Solution> {
    let matrix = routing_matrix(problem, cached_matrix, profile, strategy)?;
    let solution = greedy::plan_with_matrix(problem, matrix);
    assert!(
        solution.validate(problem).is_ok(),
        "solver produced an invalid solution"
    );
    Ok(solution)
}
