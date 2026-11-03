//! Route construction and, later, metaheuristic optimisation.
//!
//! M0 ships a deterministic greedy insertion heuristic on top of a haversine
//! distance matrix. Road-network durations (OSM) replace the distance/speed
//! approximation in M3 of the roadmap.

use domain::{Problem, Solution};

/// Solves `problem` and returns a validated plan.
///
/// The returned solution is guaranteed to satisfy
/// [`domain::Solution::validate`]; stores that cannot be served within every
/// constraint are returned in [`domain::Solution::unassigned`].
pub fn solve(_problem: &Problem) -> Solution {
    unimplemented!("solver subagent fills this in")
}
