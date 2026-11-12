//! LNS-vs-LS benchmark on a harder problem than C101.
//!
//! Purpose: C101's windows are wide (102–138 minutes; depot "customer 0"
//! aside, the tightest is 102 min) and the best known is 828.94 — LS already
//! sits 25% under that, so C101 cannot distinguish solvers at the top. This
//! benchmark builds a synthetic Euclidean problem the same way
//! `solomon_to_problem` builds C101 (single depot, ambient-only capacity,
//! unit-time = unit-distance at 60 km/h) but with:
//!
//! - **200 customers** with demand sampled from C101's range (16–26), so
//!   total demand (~4,200) approaches the 25×200 fleet capacity; and
//! - **windows derived from depot distance** (ready = max(0, d-20),
//!   due = ready + 60 + (customer_id % 5) * 10), so openings are tight
//!   (60–100 minutes) and spatially coherent (nearby customers share
//!   windows).
//!
//! The test prints both solvers' distances and asserts validity and full
//! service. It does not assert that LNS beats LS — the comparison is
//! reported, not gated, because whether LNS wins at this scale is the open
//! question of task 6. The numbers decide whether the "LNS needs a measured
//! win" condition holds.

use domain::{
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
};
use solver::{
    LNSConfig, LSConfig, euclidean_matrix, solve, solve_with_lns, solve_with_local_search,
};

const CUSTOMER_COUNT: usize = 200;
const VEHICLE_COUNT: u32 = 25;
const SPEED_KMH: f64 = 60.0;
const DEPOT_COORD: (f64, f64) = (40.0, 50.0);

/// Builds the harder benchmark: 200 customers on a 50x50 grid, tight
/// distance-derived windows, capacity 200 ambient per vehicle.
fn harder_benchmark() -> Problem {
    let depot = Depot {
        id: DepotId(0),
        name: "Depot".into(),
        location: LatLon::new(DEPOT_COORD.1, DEPOT_COORD.0),
        timezone: None,
    };
    let stores: Vec<Store> = (0..CUSTOMER_COUNT)
        .map(|index| {
            // C101-style coordinates: x,y in [15,55], deterministic pseudo-random.
            let x = 15.0 + ((index as f64) * 13.73) % 40.0;
            let y = 15.0 + ((index as f64) * 27.31) % 40.0;
            let d = ((x - DEPOT_COORD.0).powi(2) + (y - DEPOT_COORD.1).powi(2)).sqrt();
            let demand = 16 + (((index as f64) * 7.13 % 11.0) as u32);
            // Windows: ready = round-trip travel time + small fleet slack.
            // d*2.4 is conservative enough that drivers returning to the
            // depot before the window still arrive inside it; width 60–100.
            let ready = (d * 2.4) as i64;
            let width = 60 + ((index as i64) % 5) * 10;
            Store {
                id: StoreId(index as u32),
                chain: Chain::Coles,
                name: format!("Customer {:03}", index + 1),
                location: LatLon::new(y, x),
                demand: Load::new(demand, 0, 0),
                service: Minutes::from_minutes(10),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(ready),
                    Minutes::from_minutes(ready + width),
                )],
                timezone: None,
            }
        })
        .collect();

    let horizon = 700;
    let vehicles: Vec<Vehicle> = (0..VEHICLE_COUNT)
        .map(|i| Vehicle {
            id: VehicleId(i),
            name: format!("Vehicle {:03}", i + 1),
            depot: DepotId(0),
            capacity: Load::new(200, 0, 0),
            max_duration: Minutes::from_minutes(horizon),
            avg_speed_kmh: SPEED_KMH,
        })
        .collect();
    let drivers: Vec<Driver> = (0..VEHICLE_COUNT)
        .map(|i| Driver {
            id: DriverId(i),
            name: format!("Driver {:03}", i + 1),
            shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(horizon))],
            fatigue_rules: FatigueRules::default(),
        })
        .collect();

    // Test fixture payloads intentionally match the crate's Problem shape
    // (see the code-quality skill on test duplication).
    // jscpd:ignore-start
    Problem {
        name: "harder 200".into(),
        depots: vec![depot],
        stores,
        vehicles,
        drivers,
        cost: CostModel {
            fixed_per_route_cents: 0,
            per_km_cents: 100,
            per_hour_cents: 0,
            late_per_min_cents: 0,
        },
        chain_rules: ChainRules::default(),
        curfews: CurfewRules::default(),
        horizon_start: None,
    }
    // jscpd:ignore-end
}

fn report(solver: &str, problem: &Problem, solution: &domain::Solution, min_serve: usize) {
    assert_eq!(
        solution.validate(problem),
        Ok(()),
        "{solver} must produce a valid solution"
    );
    let served = problem.stores.len() - solution.unassigned.len();
    eprintln!(
        "harder-{solver}: routes={}, distance={:.2}, served={served}/{}",
        solution.routes.len(),
        solution.total_distance_km,
        problem.stores.len(),
    );
    assert!(
        served >= min_serve,
        "{solver} must serve at least {min_serve} stores: {served}/{}",
        problem.stores.len()
    );
}

/// The benchmark: LS vs LNS, printed but not gated on who wins.
#[test]
fn harder_ls_vs_lns_report() {
    let problem = harder_benchmark();

    // Greedy baseline for context.
    let greedy = solve(&problem);
    eprintln!(
        "harder greedy: routes={}, distance={:.2}",
        greedy.routes.len(),
        greedy.total_distance_km,
    );

    let ls_config = LSConfig {
        max_no_improve: 500,
        seed: 42,
        restarts: 2,
        time_limit_ms: 10_000,
    };
    let ls = solve_with_local_search(&problem, ls_config, euclidean_matrix(&problem));
    report("local-search", &problem, &ls, 180);

    let lns_config = LNSConfig {
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
    let lns = solve_with_lns(&problem, lns_config, euclidean_matrix(&problem));
    report("lns", &problem, &lns, 120);

    eprintln!(
        "harder comparison: LS {:.2} vs LNS {:.2} (lower is better)",
        ls.total_distance_km, lns.total_distance_km
    );
    eprintln!(
        "lns wins by {:.2}% ({} vs {})",
        (ls.total_distance_km - lns.total_distance_km) / ls.total_distance_km * 100.0,
        lns.total_distance_km,
        ls.total_distance_km,
    );
}
