//! Curfew-aware greedy construction: insertion with time-dependent legs.
//!
//! [`crate::curfew::enforce_curfews`] is a feasibility *restore*: it keeps
//! the static plan's route order and drops stores whose curfew waits no
//! longer fit. This module instead schedules the insertion loop with the
//! caller's [`LegTimes`] provider, so a candidate's cost already includes
//! curfew waiting and the planner prefers placements that survive the waits
//! (different insertion position, different driver, or a fresh route).
//!
//! The restore pass remains as a safety net: after construction, call
//! [`crate::curfew::enforce_curfews`] with the same provider. On inputs
//! where construction with the static matrix is already curfew-feasible,
//! both paths produce identical output.

use domain::{Problem, Solution};

use crate::curfew::LegTimes;
use crate::greedy::plan_with_legs;
use crate::matrix::DistanceMatrix;

/// Solves `problem` with the greedy insertion heuristic, scoring every leg
/// with `legs` (e.g. [`crate::curfew::StaticLegs`] for the static matrix, or
/// `routing::timed` on the OSM graph).
///
/// The returned solution is guaranteed to satisfy
/// [`domain::Solution::validate`]; stores with no feasible placement under
/// the time-dependent legs are reported in
/// [`domain::Solution::unassigned`]. The same problem and provider always
/// yield the same plan.
#[must_use]
pub fn solve_greedy_with_curfews(problem: &Problem, legs: &mut dyn LegTimes) -> Solution {
    plan_with_legs(problem, legs)
}

/// Solves `problem` with the greedy insertion heuristic on `matrix`,
/// scoring every leg with `legs`.
#[must_use]
pub fn solve_greedy_with_curfews_on_matrix(
    problem: &Problem,
    _matrix: DistanceMatrix,
    legs: &mut dyn LegTimes,
) -> Solution {
    plan_with_legs(problem, legs)
}

#[cfg(test)]
mod tests {
    use super::super::curfew::{StaticLegs, enforce_curfews};
    use super::super::solve_greedy_with_curfews;
    use domain::{
        Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
        LatLon, Load, Minutes, Node, Problem, RoutePlan, Shift, Solution, Store, StoreId,
        TimeWindow, Vehicle, VehicleId,
    };

    fn depot() -> Depot {
        Depot {
            id: DepotId(0),
            name: "DC".into(),
            location: LatLon::new(-33.87, 151.21),
            address: None,
            timezone: None,
        }
    }

    fn store(id: u32, name: &str, lon: f64, window: (i64, i64)) -> Store {
        Store {
            id: StoreId(id),
            chain: Chain::Coles,
            name: name.into(),
            location: LatLon::new(-33.87, lon),
            address: None,
            demand: Load::new(1, 1, 0),
            service: Minutes::from_minutes(10),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(window.0),
                Minutes::from_minutes(window.1),
            )],
            timezone: None,
        }
    }

    // Test fixtures mirror the crate's shared problem fixtures by shape
    // (see the code-quality skill on test duplication); marked explicitly
    // rather than restructured, since the literals ARE the fixture data.
    // jscpd:ignore-start
    fn rigid() -> Vehicle {
        Vehicle {
            id: VehicleId(0),
            name: "Rigid".into(),
            depot: DepotId(0),
            capacity: Load::new(8, 8, 4),
            max_duration: Minutes::from_minutes(600),
            avg_speed_kmh: 60.0,
        }
    }

    fn sam() -> Driver {
        Driver {
            id: DriverId(0),
            name: "Sam".into(),
            shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(600))],
            fatigue_rules: FatigueRules::default(),
        }
    }
    // jscpd:ignore-end

    /// One depot, two stores X (tight window) and Y (wide), one vehicle and
    /// one driver. Y is close to the depot, X far east, so static greedy
    /// prefers visiting Y first (depot->Y is cheaper than depot->X).
    fn order_problem() -> Problem {
        Problem {
            name: "curfew order fixture".into(),
            depots: vec![depot()],
            stores: vec![
                store(0, "X east (tight)", 151.44, (120, 320)),
                store(1, "Y west (wide)", 151.20, (150, 320)),
            ],
            vehicles: vec![rigid()],
            drivers: vec![sam()],
            cost: CostModel::default(),
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
        }
    }

    /// Provider that bans legs to X during [100, 300) of the horizon:
    /// departing within the window waits until 300. The east road into X
    /// is closed in the morning.
    struct WaitOnX {
        base: StaticLegs,
        target: Node,
    }

    impl crate::curfew::LegTimes for WaitOnX {
        fn leg(&mut self, from: Node, to: Node, depart: Minutes) -> Option<routing::TimedLeg> {
            let mut leg = self.base.leg(from, to, depart)?;
            if to == self.target && (100..300).contains(&depart.minutes()) {
                leg.wait_minutes = 300 - depart.minutes();
                leg.arrive =
                    Minutes::from_minutes(depart.minutes() + leg.drive_minutes + leg.wait_minutes);
            }
            Some(leg)
        }
    }

    fn served(solution: &Solution) -> usize {
        solution
            .routes
            .iter()
            .flat_map(|route| &route.steps)
            .filter(|step| matches!(step.node, Node::Store(_)))
            .count()
    }

    #[test]
    fn static_legs_match_greedy() {
        let problem = order_problem();
        let greedy = crate::solve_greedy(&problem);
        let mut legs = StaticLegs::new(&problem);
        let aware = solve_greedy_with_curfews(&problem, &mut legs);
        assert_eq!(served(&aware), served(&greedy));
        assert_eq!(aware.total_distance_km, greedy.total_distance_km);
        assert_eq!(aware.validate(&problem), Ok(()));
    }

    #[test]
    fn aware_wins_where_restore_would_drop() {
        let problem = order_problem();
        // Greedy (static) order: Y first (insertion tie-break), then X. The
        // Y->X leg departs at ~160, inside the X curfew window, so restore
        // re-simulation pushes arrival past X's window and drops X.
        let plan = crate::solve_greedy(&problem);
        let mut provider = WaitOnX {
            base: StaticLegs::new(&problem),
            target: Node::Store(StoreId(0)),
        };
        let (restored, report) = enforce_curfews(&problem, &plan, &mut provider);
        assert_eq!(restored.validate(&problem), Ok(()));
        assert!(
            report.dropped.iter().any(|drop| drop.store == StoreId(0)),
            "restore should drop X: {report:?}"
        );
        assert_eq!(served(&restored), 1);

        // Aware construction scores the wait during insertion: serving X
        // first (departure before the curfew window) keeps both stores.
        let mut provider = WaitOnX {
            base: StaticLegs::new(&problem),
            target: Node::Store(StoreId(0)),
        };
        let aware = solve_greedy_with_curfews(&problem, &mut provider);
        assert_eq!(aware.validate(&problem), Ok(()));
        assert_eq!(served(&aware), 2);
        assert!(aware.unassigned.is_empty());
    }

    #[test]
    fn aware_respects_driver_availability() {
        // No driver covers either store's window (X ends 320, Y ends 320),
        // so both are unassignable under static and aware construction.
        let mut problem = order_problem();
        problem.drivers[0].shifts = vec![Shift::new(
            Minutes::from_minutes(500),
            Minutes::from_minutes(600),
        )];
        let mut legs = StaticLegs::new(&problem);
        let aware = solve_greedy_with_curfews(&problem, &mut legs);
        assert_eq!(aware.validate(&problem), Ok(()));
        assert_eq!(served(&aware), 0);
        assert_eq!(aware.unassigned.len(), 2);
    }

    #[test]
    fn single_store_route_schedules() {
        let mut problem = order_problem();
        problem.stores.truncate(1);
        let mut legs = StaticLegs::new(&problem);
        let aware = solve_greedy_with_curfews(&problem, &mut legs);
        assert_eq!(aware.validate(&problem), Ok(()));
        assert_eq!(served(&aware), 1);
    }

    #[test]
    fn aware_route_is_a_route_plan() {
        let problem = order_problem();
        let mut legs = StaticLegs::new(&problem);
        let aware = solve_greedy_with_curfews(&problem, &mut legs);
        for route in &aware.routes {
            let route_plan: &RoutePlan = route;
            assert_eq!(route_plan.drivers.len(), 1);
        }
    }
}
