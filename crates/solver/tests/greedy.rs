//! End-to-end tests for the M0 greedy insertion solver.

use domain::{
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Node, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
};
use solver::{solve, solve_greedy_euclidean};

/// One depot, three stores and two vehicles on one driver shift.
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
            timezone: None,
        }],
        stores: vec![Store {
            id: StoreId(0),
            chain: Chain::Coles,
            name: "Coles Wollongong #001".into(),
            location: LatLon::new(-34.4253, 150.8935),
            timezone: None,
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
                Minutes::from_minutes(1300),
                Minutes::from_minutes(2400),
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

    // The round trip should complete within the driver's shift and fatigue limits
    let last = route.steps.last().expect("return leg");
    assert!(last.depart <= problem.drivers[0].shifts[0].end);
    let total_dist: f64 = solution.routes.iter().map(|r| r.distance_km).sum();
    assert!(total_dist > 100.0, "got {}", total_dist);
    assert!(total_dist < 500.0, "got {}", total_dist);
}

/// C101 calibration test — the standard Solomon benchmark.
/// Best known: 10 routes, 828.94 distance (with Euclidean).
/// Our greedy solver on Euclidean coordinates should get within reasonable range.
#[test]
fn c101_calibration_greedy() {
    use solver::parser::{parse_solomon_file, solomon_to_problem};

    // The calibration dataset is tracked in the repository. Fail loudly when
    // it is missing rather than silently skipping the calibration gate.
    let path = std::path::Path::new("../../data/solomon/C101.txt");
    if !path.exists() {
        panic!(
            "C101 dataset is tracked in this repository at data/solomon/C101.txt; path {} does not exist from this test's working directory",
            path.display()
        );
    }

    let inst = parse_solomon_file(path).unwrap();
    // C101 treats each distance unit as 1 minute; at 60 km/h, 1 unit of travel
    // time = 1 minute of driving (60 km/h = 1 km/min).
    let problem = solomon_to_problem(inst, 60.0);

    let solution = solve_greedy_euclidean(&problem);

    // Validate
    assert_eq!(
        solution.validate(&problem),
        Ok(()),
        "solution must be valid even if not all stores served"
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

    // C101 best known: 10 routes, 828.94 km, 100 served.
    // M2 gate: ≤5% gap → distance ≤ 870.39, all stores served.
    // Routes are free (fixed_per_route_cents = 0) so distance is the objective.
    assert_eq!(served, 100, "must serve all 100 stores: {}", served);
    assert!(
        total_dist <= 870.39,
        "distance {:.2} exceeds 5% gap to best-known 828.94 (limit 870.39)",
        total_dist
    );
}

/// C101 calibration with local search (M2).
/// Tests whether local search improves the greedy baseline.
#[test]
fn c101_calibration_local_search() {
    use solver::{
        LSConfig, euclidean_matrix, parse_solomon_file, solomon_to_problem, solve_with_local_search,
    };

    let path = std::path::Path::new("../../data/solomon/C101.txt");
    if !path.exists() {
        panic!(
            "C101 dataset is tracked in this repository at data/solomon/C101.txt; path {} does not exist from this test's working directory",
            path.display()
        );
    }

    let inst = parse_solomon_file(path).unwrap();
    // C101 treats each distance unit as 1 minute; at 60 km/h, 1 unit of travel
    // time = 1 minute of driving (60 km/h = 1 km/min).
    let problem = solomon_to_problem(inst, 60.0);

    let config = LSConfig {
        max_no_improve: 500,
        seed: 42,
        restarts: 2,
        time_limit_ms: 10_000,
    };
    // C101 uses Euclidean coordinates, not lat/lon
    let matrix = euclidean_matrix(&problem);
    let solution = solve_with_local_search(&problem, config, matrix);

    // Validate
    assert_eq!(
        solution.validate(&problem),
        Ok(()),
        "solution must be valid"
    );

    let routes_used = solution.routes.len();
    let total_dist = solution.total_distance_km;
    let served = problem.stores.len() - solution.unassigned.len();

    eprintln!(
        "C101 local search: routes={}, distance={:.2}, served={}/{}",
        routes_used,
        total_dist,
        served,
        problem.stores.len()
    );

    // C101 best known: 10 routes, 828.94 km, 100 served.
    // M2 gate: ≤5% gap → distance ≤ 870.39, all stores served.
    assert_eq!(served, 100, "must serve all 100 stores: {}", served);
    assert!(
        total_dist <= 870.39,
        "distance {:.2} exceeds 5% gap to best-known 828.94 (limit 870.39)",
        total_dist
    );

    // Fleet cost is now non-zero (15_000 cents/route, matching gen-sim); the
    // solver still emits 11 routes on C101 (best-known 10). Route-count gating
    // would need a route-elimination operator; distance gate remains authoritative.
}

/// C101 calibration with multi-start LNS: do eight parallel seeds beat the
/// single run (709.10) and the local-search path (621.42)?
#[test]
fn c101_calibration_lns_multistart() {
    use solver::{
        LNSConfig, euclidean_matrix, parse_solomon_file, solomon_to_problem,
        solve_with_lns_multistart,
    };

    let path = std::path::Path::new("../../data/solomon/C101.txt");
    if !path.exists() {
        panic!(
            "C101 dataset is tracked in this repository at data/solomon/C101.txt; path {} does not exist from this test's working directory",
            path.display()
        );
    }

    let inst = parse_solomon_file(path).unwrap();
    let problem = solomon_to_problem(inst, 60.0);
    // Each parallel run is wall-clock bounded; the budget is tunable so
    // longer measurements can be taken without editing the test.
    let time_limit_ms = std::env::var("C101_LNS_MS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(30_000);
    let config = LNSConfig {
        max_no_improve: 1000,
        ruin_size_range: (6, 14),
        seed: 42,
        time_limit_ms,
        initial_temperature: 1500.0,
        cooling_rate: 0.9995,
        min_temperature: 0.1,
        adaptive: true,
        reaction_factor: 0.1,
    };

    let started = std::time::Instant::now();
    let solution = solve_with_lns_multistart(&problem, config, euclidean_matrix(&problem), 8);
    let elapsed = started.elapsed();

    assert_eq!(
        solution.validate(&problem),
        Ok(()),
        "solution must be valid"
    );
    let served = problem.stores.len() - solution.unassigned.len();
    assert_eq!(served, 100, "must serve all 100 stores: {served}");
    assert!(
        solution.total_distance_km <= 870.39,
        "distance {:.2} exceeds the 5% gate",
        solution.total_distance_km
    );
    eprintln!(
        "C101 LNS x8: routes={}, distance={:.2}, served={}/100, elapsed={elapsed:.1?}",
        solution.routes.len(),
        solution.total_distance_km,
        served,
    );
}

/// C101 calibration with LNS/ALNS (M2).
/// Tests whether LNS improves the greedy baseline.
#[test]
fn c101_calibration_lns() {
    use solver::{
        LNSConfig, euclidean_matrix, parse_solomon_file, solomon_to_problem, solve_with_lns,
    };

    let path = std::path::Path::new("../../data/solomon/C101.txt");
    if !path.exists() {
        panic!(
            "C101 dataset is tracked in this repository at data/solomon/C101.txt; path {} does not exist from this test's working directory",
            path.display()
        );
    }

    let inst = parse_solomon_file(path).unwrap();
    // C101 treats each distance unit as 1 minute; at 60 km/h, 1 unit of travel
    // time = 1 minute of driving (60 km/h = 1 km/min). Using 60.0 keeps
    // Solomon's unit-distance ≈ unit-time mapping so the ready/due windows
    // (e.g. 912–967) line up with travel distances (~19 units).
    let problem = solomon_to_problem(inst, 60.0);

    let config = LNSConfig {
        max_no_improve: 1000,
        ruin_size_range: (6, 14),
        seed: 42,
        time_limit_ms: 30_000,
        initial_temperature: 1500.0,
        cooling_rate: 0.9995,
        min_temperature: 0.1,
        adaptive: true,
        reaction_factor: 0.1,
    };
    // C101 uses Euclidean coordinates, not lat/lon
    let matrix = euclidean_matrix(&problem);

    // The greedy baseline, computed the same way LNS computes it. Asserting
    // that LNS beats it (strictly) is what distinguishes a genuine LNS
    // result from the greedy fallback: the fallback returns exactly this
    // distance, so equality fails loudly instead of passing silently.
    let greedy = solver::solve_greedy_euclidean(&problem);

    let solution = solve_with_lns(&problem, config, matrix);

    // Validate
    assert_eq!(
        solution.validate(&problem),
        Ok(()),
        "solution must be valid"
    );

    let routes_used = solution.routes.len();
    let total_dist = solution.total_distance_km;
    let served = problem.stores.len() - solution.unassigned.len();

    eprintln!(
        "C101 LNS: routes={}, distance={:.2}, served={}/{} (greedy baseline {:.2})",
        routes_used,
        total_dist,
        served,
        problem.stores.len(),
        greedy.total_distance_km
    );

    // C101 best known: 10 routes, 828.94 km, 100 served.
    // M2 gate: ≤5% gap → distance ≤ 870.39, all stores served.
    assert_eq!(served, 100, "must serve all 100 stores: {}", served);
    assert!(
        total_dist < greedy.total_distance_km,
        "LNS ({:.2}) must strictly beat the greedy baseline ({:.2}); \
         equality means the greedy fallback fired and LNS returned no plan \
         of its own",
        total_dist,
        greedy.total_distance_km
    );
    assert!(
        total_dist <= 870.39,
        "distance {:.2} exceeds 5% gap to best-known 828.94 (limit 870.39)",
        total_dist
    );

    // Fleet cost is now non-zero (15_000 cents/route, matching gen-sim); the
    // solver still emits 11 routes on C101 (best-known 10). Route-count gating
    // would need a route-elimination operator; distance gate remains authoritative.
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
            timezone: None,
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

fn make_store(id: u32, demand: Load, window: (i64, i64)) -> Store {
    Store {
        id: StoreId(id),
        chain: Chain::Coles,
        name: format!("Coles #{id}"),
        location: LatLon::new(-33.90, 151.20),
        timezone: None,
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
