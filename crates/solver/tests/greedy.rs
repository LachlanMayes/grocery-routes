//! End-to-end tests for the M0 greedy insertion solver.

use domain::{
    Chain, CostModel, Depot, DepotId, Driver, DriverId, LatLon, Load, Minutes, Node, Problem,
    Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
};
use solver::{solve, solve_greedy};

/// One depot, three stores and two vehicles on one driver shift.
fn small_problem() -> Problem {
    Problem {
        name: "small metro".into(),
        depots: vec![Depot {
            id: DepotId(0),
            name: "Sydney DC".into(),
            location: LatLon::new(-33.87, 151.21),
        }],
        stores: vec![
            Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "Coles Inner West".into(),
                location: LatLon::new(-33.92, 151.23),
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
        }],
        cost: CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 90,
            per_hour_cents: 4_500,
            late_per_min_cents: 0,
        },
    }
}

#[test]
fn small_problem_is_fully_served_and_valid() {
    let problem = small_problem();
    let solution = solve(&problem);

    assert_eq!(solution.validate(&problem), Ok(()));
    assert!(
        solution.unassigned.is_empty(),
        "unassigned: {:?}",
        solution.unassigned
    );
    assert!(!solution.routes.is_empty());
    assert!(solution.routes.len() <= problem.vehicles.len());

    let mut served = vec![0_u32; problem.stores.len()];
    for route in &solution.routes {
        assert_eq!(
            route.drivers.len(),
            1,
            "M0 uses exactly one driver per route"
        );
        for step in &route.steps {
            if let Node::Store(id) = step.node {
                served[id.0 as usize] += 1;
            }
        }
    }
    assert_eq!(served, vec![1, 1, 1]);

    let total_km: f64 = solution.routes.iter().map(|route| route.distance_km).sum();
    assert!((solution.total_distance_km - total_km).abs() < 1e-9);
    let total_minutes: i64 = solution
        .routes
        .iter()
        .map(|route| route.duration.minutes())
        .sum();
    assert_eq!(solution.total_duration.minutes(), total_minutes);
    let total_cents: i64 = solution.routes.iter().map(|route| route.cost_cents).sum();
    assert_eq!(solution.total_cost_cents, total_cents);
}

#[test]
fn solve_is_deterministic() {
    let problem = small_problem();
    assert_eq!(solve(&problem), solve(&problem));
}

/// The M0 multi-day gate: a 48-hour horizon whose single store can only be
/// served on day 2, by one long shift.
fn multi_day_problem() -> Problem {
    Problem {
        name: "multi-day gate".into(),
        depots: vec![Depot {
            id: DepotId(0),
            name: "Sydney DC".into(),
            location: LatLon::new(-33.8688, 151.2093),
        }],
        stores: vec![Store {
            id: StoreId(0),
            chain: Chain::Coles,
            name: "Coles Melbourne #001".into(),
            location: LatLon::new(-37.8136, 144.9631),
            demand: Load::new(1, 1, 0),
            service: Minutes::from_minutes(15),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(1500),
                Minutes::from_minutes(1800),
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
        drivers: vec![Driver {
            id: DriverId(0),
            name: "Long haul".into(),
            shifts: vec![Shift::new(
                Minutes::from_minutes(600),
                Minutes::from_minutes(2400),
            )],
        }],
        cost: CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 90,
            per_hour_cents: 4_500,
            late_per_min_cents: 0,
        },
    }
}

#[test]
fn multi_day_horizon_store_is_served_on_day_two() {
    let problem = multi_day_problem();
    let window = problem.stores[0].receiving[0];
    assert!(
        window.start >= Minutes::DAY,
        "fixture window must start on day 2"
    );

    let solution = solve(&problem);

    assert_eq!(solution.validate(&problem), Ok(()));
    assert!(
        solution.unassigned.is_empty(),
        "unassigned: {:?}",
        solution.unassigned
    );
    assert_eq!(solution.routes.len(), 1);

    let route = &solution.routes[0];
    let stop = route
        .steps
        .iter()
        .find(|step| step.node == Node::Store(StoreId(0)))
        .expect("store stop");
    assert!(
        stop.arrive >= window.start,
        "arrival {} is before the window start {}",
        stop.arrive,
        window.start
    );
    assert!(
        stop.arrive >= Minutes::DAY,
        "store must be served on day 2 of the horizon"
    );
    assert_eq!(stop.depart - stop.arrive, problem.stores[0].service);

    // The run is ~714 km each way at 75 km/h, so the plan spends most of its
    // time driving; the return leg must land back inside the shift.
    let last = route.steps.last().expect("return leg");
    assert!(route.distance_km > 1300.0, "got {}", route.distance_km);
    assert!(last.depart <= problem.drivers[0].shifts[0].end);
}

/// C101 calibration test — the standard Solomon benchmark.
/// Best known: 10 routes, 828.94 distance (with Euclidean).
/// Our greedy solver on Euclidean coordinates should get within reasonable range.
#[test]
fn c101_calibration_greedy() {
    use solver::parser::{parse_solomon_file, solomon_to_problem};

    // For the calibration gate we parse the full file if present,
    // otherwise we skip the test.
    let path = std::path::Path::new("data/solomon/C101.txt");
    if !path.exists() {
        eprintln!("Skipping C101 calibration: data/solomon/C101.txt not found");
        return;
    }

    let inst = parse_solomon_file(path).unwrap();
    let problem = solomon_to_problem(inst, 1.0);

    let solution = solve_greedy(&problem);

    // Validate
    assert_eq!(solution.validate(&problem), Ok(()));
    assert!(
        solution.unassigned.is_empty(),
        "unassigned: {:?}",
        solution.unassigned
    );

    // Record the baseline for the M1 gate
    let routes_used = solution.routes.len();
    let total_dist = solution.total_distance_km;
    let served = problem.stores.len() - solution.unassigned.len();

    eprintln!(
        "C101 calibration: routes={}, distance={:.2}, served={}/{}",
        routes_used,
        total_dist,
        served,
        problem.stores.len()
    );

    // The exact targets will be set after this run; for now just ensure it runs.
    assert!(routes_used > 0);
    assert!(total_dist > 0.0);
    assert_eq!(served, problem.stores.len());
}

/// One depot, one vehicle with `capacity` and one driver on a 10-hour shift;
/// callers add the stores.
fn single_vehicle_problem(capacity: Load, stores: Vec<Store>) -> Problem {
    Problem {
        name: "edge case".into(),
        depots: vec![Depot {
            id: DepotId(0),
            name: "Depot".into(),
            location: LatLon::new(-33.87, 151.21),
        }],
        stores,
        vehicles: vec![Vehicle {
            id: VehicleId(0),
            name: "Rigid".into(),
            depot: DepotId(0),
            capacity,
            max_duration: Minutes::from_minutes(600),
            avg_speed_kmh: 60.0,
        }],
        drivers: vec![Driver {
            id: DriverId(0),
            name: "Sam".into(),
            shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(600))],
        }],
        cost: CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 90,
            per_hour_cents: 4_500,
            late_per_min_cents: 0,
        },
    }
}

fn make_store(id: u32, demand: Load, window: (i64, i64)) -> Store {
    Store {
        id: StoreId(id),
        chain: Chain::Coles,
        name: format!("Coles #{id}"),
        location: LatLon::new(-33.90, 151.20),
        demand,
        service: Minutes::from_minutes(5),
        receiving: vec![TimeWindow::new(
            Minutes::from_minutes(window.0),
            Minutes::from_minutes(window.1),
        )],
    }
}

#[test]
fn a_store_that_cannot_fit_any_vehicle_is_reported_unassigned() {
    let problem = single_vehicle_problem(
        Load::new(1, 0, 0),
        vec![
            make_store(0, Load::new(1, 0, 0), (0, 300)),
            make_store(1, Load::new(2, 0, 0), (0, 300)),
        ],
    );
    let solution = solve(&problem);
    assert_eq!(solution.validate(&problem), Ok(()));
    assert_eq!(solution.unassigned, vec![StoreId(1)]);
    assert_eq!(solution.routes.len(), 1);
}

#[test]
fn a_store_with_no_receiving_window_is_reported_unassigned() {
    let mut problem = single_vehicle_problem(
        Load::new(4, 4, 4),
        vec![make_store(0, Load::new(1, 0, 0), (0, 300))],
    );
    problem.stores[0].receiving.clear();
    let solution = solve(&problem);
    assert_eq!(solution.validate(&problem), Ok(()));
    assert_eq!(solution.unassigned, vec![StoreId(0)]);
    assert!(solution.routes.is_empty());
}

#[test]
fn one_driver_cannot_run_two_overlapping_routes() {
    // Two vehicles but a single driver: the second route cannot be scheduled
    // without double-booking the driver, so the store stays unassigned.
    let mut problem = single_vehicle_problem(
        Load::new(1, 0, 0),
        vec![
            make_store(0, Load::new(1, 0, 0), (0, 300)),
            make_store(1, Load::new(1, 0, 0), (0, 300)),
        ],
    );
    problem.vehicles.push(Vehicle {
        id: VehicleId(1),
        name: "Rigid 2".into(),
        depot: DepotId(0),
        capacity: Load::new(1, 0, 0),
        max_duration: Minutes::from_minutes(600),
        avg_speed_kmh: 60.0,
    });
    let solution = solve(&problem);
    assert_eq!(solution.validate(&problem), Ok(()));
    assert_eq!(solution.routes.len(), 1);
    assert_eq!(solution.unassigned, vec![StoreId(1)]);
}
