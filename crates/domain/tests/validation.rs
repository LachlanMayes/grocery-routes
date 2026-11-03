//! Validation fixtures, including the M0 multi-day relay gate.
//!
//! The Sydney–Melbourne fixture proves that a two-driver overnight relay is
//! expressible in the model and accepted by the validator: driver A covers the
//! Sydney → Albury leg on their evening shift, the truck dwells at the Albury
//! relay depot across the midnight hand-over, and driver B covers the
//! Albury → Melbourne leg. Timings are illustrative (synthetic), not real
//! drive times.

use domain::{
    Chain, CostModel, Depot, DepotId, Driver, DriverId, LatLon, Load, Minutes, Node, PlannedStop,
    Problem, RoutePlan, Shift, Solution, Store, StoreId, TimeWindow, Vehicle, VehicleId, Violation,
};

fn relay_fixture() -> (Problem, Solution) {
    let problem = Problem {
        name: "sydney-melbourne relay fixture".into(),
        depots: vec![
            Depot {
                id: DepotId(0),
                name: "Sydney DC".into(),
                location: LatLon::new(-33.79, 150.90),
            },
            Depot {
                id: DepotId(1),
                name: "Albury relay".into(),
                location: LatLon::new(-36.06, 146.92),
            },
            Depot {
                id: DepotId(2),
                name: "Melbourne DC".into(),
                location: LatLon::new(-37.70, 144.85),
            },
        ],
        stores: vec![Store {
            id: StoreId(0),
            chain: Chain::Coles,
            name: "Coles Melbourne #001".into(),
            location: LatLon::new(-37.75, 144.90),
            demand: Load::new(1, 2, 0),
            service: Minutes::from_minutes(15),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(1530),
                Minutes::from_minutes(1600),
            )],
        }],
        vehicles: vec![Vehicle {
            id: VehicleId(0),
            name: "Linehaul B-double".into(),
            depot: DepotId(0),
            capacity: Load::new(20, 12, 8),
            max_duration: Minutes::from_minutes(2880),
            avg_speed_kmh: 75.0,
        }],
        drivers: vec![
            Driver {
                id: DriverId(0),
                name: "A (evening)".into(),
                shifts: vec![Shift::new(
                    Minutes::from_minutes(1020),
                    Minutes::from_minutes(1410),
                )],
            },
            Driver {
                id: DriverId(1),
                name: "B (overnight)".into(),
                shifts: vec![Shift::new(
                    Minutes::from_minutes(1410),
                    Minutes::from_minutes(1890),
                )],
            },
        ],
        cost: CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 90,
            per_hour_cents: 4_500,
            late_per_min_cents: 0,
        },
    };

    let solution = Solution {
        routes: vec![RoutePlan {
            vehicle: VehicleId(0),
            drivers: vec![DriverId(0), DriverId(1)],
            steps: vec![
                PlannedStop {
                    node: Node::Depot(DepotId(0)),
                    arrive: Minutes::from_minutes(1020),
                    depart: Minutes::from_minutes(1020),
                },
                PlannedStop {
                    node: Node::Depot(DepotId(1)),
                    arrive: Minutes::from_minutes(1380),
                    depart: Minutes::from_minutes(1440),
                },
                PlannedStop {
                    node: Node::Store(StoreId(0)),
                    arrive: Minutes::from_minutes(1545),
                    depart: Minutes::from_minutes(1560),
                },
                PlannedStop {
                    node: Node::Depot(DepotId(2)),
                    arrive: Minutes::from_minutes(1600),
                    depart: Minutes::from_minutes(1600),
                },
            ],
            distance_km: 711.0,
            duration: Minutes::from_minutes(580),
            // Aggregates are informational in M0; not re-derived by validate().
            cost_cents: 0,
        }],
        unassigned: vec![],
        total_distance_km: 711.0,
        total_duration: Minutes::from_minutes(580),
        total_cost_cents: 0,
    };

    (problem, solution)
}

fn violations_of(problem: &Problem, solution: &Solution) -> Vec<Violation> {
    solution
        .validate(problem)
        .expect_err("expected at least one violation")
}

#[test]
fn multi_day_relay_with_two_shifts_is_expressible_and_valid() {
    let (problem, solution) = relay_fixture();
    assert_eq!(solution.validate(&problem), Ok(()));
}

#[test]
fn a_shift_gap_is_rejected() {
    let (mut problem, solution) = relay_fixture();
    // Driver A no longer covers the first leg: shift now starts at 18:20.
    problem.drivers[0].shifts = vec![Shift::new(
        Minutes::from_minutes(1100),
        Minutes::from_minutes(1410),
    )];
    let violations = violations_of(&problem, &solution);
    assert!(
        violations.contains(&Violation::ShiftNotCovered { route: 0, step: 0 }),
        "got {violations:?}"
    );
}

#[test]
fn a_missed_receiving_window_is_rejected() {
    let (mut problem, solution) = relay_fixture();
    problem.stores[0].receiving = vec![TimeWindow::new(
        Minutes::from_minutes(1500),
        Minutes::from_minutes(1540),
    )];
    let violations = violations_of(&problem, &solution);
    assert!(
        violations.contains(&Violation::WindowMissed {
            route: 0,
            store: StoreId(0),
        }),
        "got {violations:?}"
    );
}

#[test]
fn a_capacity_overload_is_rejected() {
    let (mut problem, solution) = relay_fixture();
    problem.stores[0].demand = Load::new(1, 99, 0);
    let violations = violations_of(&problem, &solution);
    assert!(
        violations.contains(&Violation::LoadExceeded { route: 0, step: 2 }),
        "got {violations:?}"
    );
}

#[test]
fn an_unserved_store_is_reported() {
    let (problem, _) = relay_fixture();
    let empty = Solution {
        routes: vec![],
        unassigned: vec![],
        total_distance_km: 0.0,
        total_duration: Minutes::ZERO,
        total_cost_cents: 0,
    };
    let violations = violations_of(&problem, &empty);
    assert!(
        violations.contains(&Violation::MissingStore { store: StoreId(0) }),
        "got {violations:?}"
    );
}

#[test]
fn a_served_store_may_not_also_be_unassigned() {
    let (problem, mut solution) = relay_fixture();
    solution.unassigned = vec![StoreId(0)];
    let violations = violations_of(&problem, &solution);
    assert!(
        violations.contains(&Violation::ServedAndUnassigned { store: StoreId(0) }),
        "got {violations:?}"
    );
}

#[test]
fn a_duplicate_delivery_is_reported() {
    let (problem, mut solution) = relay_fixture();
    solution.routes.push(RoutePlan {
        vehicle: VehicleId(0),
        drivers: vec![DriverId(0)],
        steps: vec![
            PlannedStop {
                node: Node::Depot(DepotId(0)),
                arrive: Minutes::from_minutes(2000),
                depart: Minutes::from_minutes(2000),
            },
            PlannedStop {
                node: Node::Store(StoreId(0)),
                arrive: Minutes::from_minutes(2100),
                depart: Minutes::from_minutes(2115),
            },
            PlannedStop {
                node: Node::Depot(DepotId(0)),
                arrive: Minutes::from_minutes(2200),
                depart: Minutes::from_minutes(2200),
            },
        ],
        distance_km: 0.0,
        duration: Minutes::from_minutes(200),
        cost_cents: 0,
    });
    let violations = violations_of(&problem, &solution);
    assert!(
        violations.contains(&Violation::DuplicateStore {
            store: StoreId(0),
            count: 2,
        }),
        "got {violations:?}"
    );
}

#[test]
fn problems_and_solutions_round_trip_through_json() {
    let (problem, solution) = relay_fixture();
    let problem_json = serde_json::to_string_pretty(&problem).expect("serialize problem");
    let solution_json = serde_json::to_string_pretty(&solution).expect("serialize solution");
    let parsed_problem: Problem = serde_json::from_str(&problem_json).expect("parse problem");
    let parsed_solution: Solution = serde_json::from_str(&solution_json).expect("parse solution");
    assert_eq!(parsed_problem, problem);
    assert_eq!(parsed_solution, solution);
    assert_eq!(parsed_solution.validate(&parsed_problem), Ok(()));
}
