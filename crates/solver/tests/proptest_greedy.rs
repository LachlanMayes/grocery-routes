//! Property tests for the greedy solver.
//!
//! The generated fixture family is deliberately generous: one depot, 1–8
//! stores in a Sydney-sized box (lat −34.0..−33.5, lon 150.5..151.5), 1–4
//! vehicles whose capacities are twice the total demand, one driver per
//! vehicle on a 300..1200 shift, and wide receiving windows.
//!
//! Fixture rationale: the box is at most ~108 km across, so the worst possible
//! leg is ~86 minutes at 75 km/h. With a 300..=320 receiving start, a 1100
//! window end, 5-minute service and the 900-minute duration cap, *any*
//! sequence of the eight stores serves its last store by
//! `300 + 8*87 + 7*5 + 20 = 1051` minutes and returns by 1137, inside every
//! window, the shift and the duration cap. A store can therefore always be
//! placed into a route with at most seven stores (or a fresh vehicle), which
//! is why `unassigned` is provably empty. Looser example ranges (longer
//! services, later window starts) can genuinely strand stores when one
//! vehicle has to take all eight, so the generator narrows them instead of
//! weakening the assertion.

use domain::{
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Node, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
};
use proptest::prelude::*;
use solver::solve;

const DEPOT_LOCATION: LatLon = LatLon::new(-33.75, 151.0);
const CHAINS: [Chain; 4] = [Chain::Coles, Chain::Woolworths, Chain::Aldi, Chain::Iga];
const SERVICE_MINUTES: i64 = 5;
const WINDOW_END: i64 = 1100;
const SHIFT: Shift = Shift::new(Minutes::from_minutes(300), Minutes::from_minutes(1200));
const MAX_DURATION_MINUTES: i64 = 900;
const SPEED_KMH: f64 = 75.0;

/// One store: location, per-compartment demand, receiving window start.
fn store_strategy() -> impl Strategy<Value = (f64, f64, u32, u32, u32, i64)> {
    (
        -34.0f64..-33.5,
        150.5f64..151.5,
        0u32..=6,
        0u32..=4,
        0u32..=2,
        300i64..=320,
    )
}

/// Generates a problem where some stores may be infeasible, testing unassigned handling.
fn store_strategy_with_infeasible() -> impl Strategy<Value = (f64, f64, u32, u32, u32, i64)> {
    (
        -34.0f64..-33.5,
        150.5f64..151.5,
        0u32..=8, // Allow higher demand that may exceed capacity
        0u32..=6,
        0u32..=4,
        300i64..=320,
    )
}

fn build_problem(raw_stores: Vec<(f64, f64, u32, u32, u32, i64)>, vehicle_count: usize) -> Problem {
    let stores: Vec<Store> = raw_stores
        .into_iter()
        .enumerate()
        .map(
            |(index, (lat, lon, ambient, chilled, frozen, window_start))| Store {
                id: StoreId(index as u32),
                chain: CHAINS[index % CHAINS.len()],
                name: format!("store {index}"),
                location: LatLon::new(lat, lon),
                demand: Load::new(ambient, chilled, frozen),
                service: Minutes::from_minutes(SERVICE_MINUTES),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(window_start),
                    Minutes::from_minutes(WINDOW_END),
                )],
            },
        )
        .collect();

    let total = stores
        .iter()
        .fold(Load::ZERO, |sum, store| sum + store.demand);
    let capacity = Load::new(2 * total.ambient, 2 * total.chilled, 2 * total.frozen);

    let vehicles: Vec<Vehicle> = (0..vehicle_count)
        .map(|index| Vehicle {
            id: VehicleId(index as u32),
            name: format!("vehicle {index}"),
            depot: DepotId(0),
            capacity,
            max_duration: Minutes::from_minutes(MAX_DURATION_MINUTES),
            avg_speed_kmh: SPEED_KMH,
        })
        .collect();
    let drivers: Vec<Driver> = (0..vehicle_count)
        .map(|index| Driver {
            id: DriverId(index as u32),
            name: format!("driver {index}"),
            shifts: vec![SHIFT],
            fatigue_rules: FatigueRules::default(),
        })
        .collect();

    Problem {
        name: "property fixture".into(),
        depots: vec![Depot {
            id: DepotId(0),
            name: "synthetic DC".into(),
            location: DEPOT_LOCATION,
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
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn greedy_serves_every_store_in_a_valid_plan(
        raw_stores in prop::collection::vec(store_strategy(), 1..=8),
        vehicle_count in 1usize..=4,
    ) {
        let problem = build_problem(raw_stores, vehicle_count);
        let solution = solve(&problem);

        prop_assert_eq!(solution.validate(&problem), Ok(()));
        prop_assert!(
            solution.unassigned.is_empty(),
            "unassigned: {:?}",
            solution.unassigned
        );
        prop_assert!(solution.routes.len() <= problem.vehicles.len());
    }

    #[test]
    fn greedy_handles_unassigned_stores(
        raw_stores in prop::collection::vec(store_strategy_with_infeasible(), 1..=8),
        vehicle_count in 1usize..=4,
    ) {
        let problem = build_problem(raw_stores, vehicle_count);
        let solution = solve(&problem);

        prop_assert_eq!(solution.validate(&problem), Ok(()));
        // Every store is either served exactly once or listed as unassigned
        let mut served = vec![0u32; problem.stores.len()];
        for route in &solution.routes {
            for step in &route.steps {
                if let Node::Store(id) = step.node {
                    served[id.0 as usize] += 1;
                }
            }
        }
        for (idx, &count) in served.iter().enumerate() {
            if count == 0 {
                prop_assert!(solution.unassigned.contains(&StoreId(idx as u32)),
                    "store {} neither served nor unassigned", idx);
            } else if count == 1 {
                prop_assert!(!solution.unassigned.contains(&StoreId(idx as u32)),
                    "store {} served but also unassigned", idx);
            } else {
                prop_assert!(false, "store {} served {} times", idx, count);
            }
        }
        prop_assert!(solution.routes.len() <= problem.vehicles.len());
    }
}
