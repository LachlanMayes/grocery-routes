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

mod greedy;
mod matrix;
pub mod parser; // Solomon/VRPLib parser (vendored for M1)

pub use matrix::{
    DistanceMatrix, FALLBACK_SPEED_KMH, euclidean_km, euclidean_matrix, travel_minutes,
};
pub use parser::{SolomonInstance, parse_solomon_file, solomon_to_problem};

use domain::{Problem, Solution};

/// Solves `problem` and returns a validated plan.
///
/// The returned solution is guaranteed to satisfy
/// [`domain::Solution::validate`]; stores that cannot be served within every
/// constraint are returned in [`domain::Solution::unassigned`]. Running the
/// solver twice on the same problem returns the same plan.
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
    debug_assert!(
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
    debug_assert!(
        solution.validate(problem).is_ok(),
        "solver produced an invalid solution"
    );
    solution
}
