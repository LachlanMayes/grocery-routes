//! Multi-start LNS: reproducibility and agreement with the single-run path.

use domain::{
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
};
use solver::{DistanceMatrix, LNSConfig, solve_with_lns, solve_with_lns_multistart};

/// A small, deterministic fixture: two depots, eight scattered stores, two
/// vehicles and wide receiving windows.
fn fixture() -> Problem {
    let depot_locations = [LatLon::new(-33.87, 151.21), LatLon::new(-33.75, 150.95)];
    let store_locations = [
        LatLon::new(-33.80, 151.30),
        LatLon::new(-33.95, 151.10),
        LatLon::new(-33.70, 151.05),
        LatLon::new(-33.90, 151.35),
        LatLon::new(-33.78, 151.42),
        LatLon::new(-34.00, 150.95),
        LatLon::new(-33.68, 151.25),
        LatLon::new(-33.85, 150.85),
    ];
    Problem {
        name: "multistart fixture".into(),
        depots: depot_locations
            .iter()
            .enumerate()
            .map(|(index, &location)| Depot {
                id: DepotId(index as u32),
                name: format!("DC {index}"),
                location,
                timezone: None,
            })
            .collect(),
        stores: store_locations
            .iter()
            .enumerate()
            .map(|(index, &location)| Store {
                id: StoreId(index as u32),
                chain: Chain::Coles,
                name: format!("Store {index}"),
                location,
                demand: Load::new(1 + (index % 3) as u32, 1 + (index % 2) as u32, 1),
                service: Minutes::from_minutes(10),
                receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(720))],
                timezone: None,
            })
            .collect(),
        vehicles: vec![
            Vehicle {
                id: VehicleId(0),
                name: "Rigid 1".into(),
                depot: DepotId(0),
                capacity: Load::new(12, 10, 6),
                max_duration: Minutes::from_minutes(600),
                avg_speed_kmh: 60.0,
            },
            Vehicle {
                id: VehicleId(1),
                name: "Rigid 2".into(),
                depot: DepotId(1),
                capacity: Load::new(12, 10, 6),
                max_duration: Minutes::from_minutes(600),
                avg_speed_kmh: 60.0,
            },
        ],
        drivers: vec![
            Driver {
                id: DriverId(0),
                name: "Sam".into(),
                shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(720))],
                fatigue_rules: FatigueRules::default(),
            },
            Driver {
                id: DriverId(1),
                name: "Alex".into(),
                shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(720))],
                fatigue_rules: FatigueRules::default(),
            },
        ],
        cost: CostModel {
            // Non-zero costs so LNS actually distinguishes candidates; with
            // all-zero costs every run returns the greedy plan and the
            // reproducibility assertions would pass vacuously.
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

fn quick_config(seed: u64) -> LNSConfig {
    LNSConfig {
        max_no_improve: 60,
        time_limit_ms: 5_000,
        seed,
        ..LNSConfig::default()
    }
}

#[test]
fn a_single_repeat_matches_the_plain_lns_run() {
    let problem = fixture();
    let matrix = DistanceMatrix::new(&problem);
    let single = solve_with_lns(&problem, quick_config(7), matrix.clone());
    let multi = solve_with_lns_multistart(&problem, quick_config(7), matrix, 1);
    assert_eq!(
        single, multi,
        "one repeat must reproduce the base seed's run"
    );
}

#[test]
fn multistart_is_reproducible_and_never_leaves_more_stores_unassigned() {
    let problem = fixture();
    let matrix = DistanceMatrix::new(&problem);
    let first = solve_with_lns(&problem, quick_config(11), matrix.clone());
    let a = solve_with_lns_multistart(&problem, quick_config(11), matrix.clone(), 3);
    let b = solve_with_lns_multistart(&problem, quick_config(11), matrix, 3);

    assert_eq!(a, b, "multi-start must be reproducible");
    assert_eq!(a.validate(&problem), Ok(()));
    assert!(
        a.unassigned.len() <= first.unassigned.len(),
        "run 0 is one of the candidates, so multi-start cannot serve fewer stores"
    );
    if a.unassigned.len() == first.unassigned.len() {
        assert!(
            a.total_distance_km <= first.total_distance_km + 1e-9,
            "with the same coverage, multi-start cannot be worse than run 0"
        );
    }
}
