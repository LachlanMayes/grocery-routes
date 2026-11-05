//! Criterion benchmark for the M0 greedy insertion solver on a synthetic
//! 40-store metro network.
//!
//! Note: Cargo's default bench harness is libtest, which ignores
//! `criterion_main!`; `cargo bench` only runs this benchmark once the
//! workspace declares the target as
//! `[[bench]] name = "solve_bench", harness = false`. The file still compiles
//! under the default harness.

use std::f64::consts::TAU;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use domain::{
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
};
use solver::solve;

const STORE_COUNT: usize = 40;
const VEHICLE_COUNT: usize = 12;
const DEPOT_LOCATION: LatLon = LatLon::new(-33.87, 151.21);
const KM_PER_DEGREE_LAT: f64 = 111.0;
const CHAINS: [Chain; 4] = [Chain::Coles, Chain::Woolworths, Chain::Aldi, Chain::Iga];

/// One depot, 40 stores spread deterministically within ~20 km of it, and 12
/// vehicles of mixed capacity with one driver shift each.
fn metro_problem() -> Problem {
    let golden_angle = TAU * 0.618_033_988_749_895;
    let stores: Vec<Store> = (0..STORE_COUNT)
        .map(|index| {
            let angle = index as f64 * golden_angle;
            let radius_km = 2.0 + (index as f64 * 3.7) % 16.0;
            let lat = DEPOT_LOCATION.lat + radius_km / KM_PER_DEGREE_LAT * angle.sin();
            let cos_lat = DEPOT_LOCATION.lat.to_radians().cos();
            let lon = DEPOT_LOCATION.lon + radius_km / (KM_PER_DEGREE_LAT * cos_lat) * angle.cos();
            Store {
                id: StoreId(index as u32),
                chain: CHAINS[index % CHAINS.len()],
                name: format!("store {index}"),
                location: LatLon::new(lat, lon),
                demand: Load::new(
                    3 + (index % 4) as u32,
                    2 + (index % 3) as u32,
                    (index % 2) as u32,
                ),
                service: Minutes::from_minutes(15),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(330),
                    Minutes::from_minutes(1170),
                )],
                timezone: None,
            }
        })
        .collect();

    let vehicles: Vec<Vehicle> = (0..VEHICLE_COUNT)
        .map(|index| Vehicle {
            id: VehicleId(index as u32),
            name: format!("vehicle {index}"),
            depot: DepotId(0),
            capacity: Load::new(
                30 + 5 * index as u32,
                20 + 4 * index as u32,
                10 + 2 * index as u32,
            ),
            max_duration: Minutes::from_minutes(900),
            avg_speed_kmh: [60.0, 75.0, 90.0][index % 3],
        })
        .collect();

    let drivers: Vec<Driver> = (0..VEHICLE_COUNT)
        .map(|index| Driver {
            id: DriverId(index as u32),
            name: format!("driver {index}"),
            shifts: vec![Shift::new(
                Minutes::from_minutes(300),
                Minutes::from_minutes(1200),
            )],
            fatigue_rules: FatigueRules::default(),
        })
        .collect();

    Problem {
        name: "metro 40".into(),
        depots: vec![Depot {
            id: DepotId(0),
            name: "synthetic DC".into(),
            location: DEPOT_LOCATION,
            timezone: None,
        }],
        stores,
        vehicles,
        drivers,
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

fn bench_greedy_insertion(c: &mut Criterion) {
    let problem = metro_problem();
    c.bench_function("greedy_insertion/metro_40", |b| {
        b.iter(|| solve(black_box(&problem)));
    });
}

criterion_group!(benches, bench_greedy_insertion);
criterion_main!(benches);
