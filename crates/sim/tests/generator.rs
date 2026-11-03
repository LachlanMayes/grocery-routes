//! Integration tests for the seeded synthetic scenario generator.

use domain::{
    Chain, DepotId, DriverId, LatLon, Load, Minutes, Shift, StoreId, VehicleId, haversine_km,
};
use sim::{City, SimOptions, generate};

/// The M0 reference scenario: 40 Coles stores around Sydney, 3 depots,
/// 12 vehicles, seed 42.
fn reference_options() -> SimOptions {
    SimOptions {
        chain: Chain::Coles,
        city: City::Sydney,
        stores: 40,
        depots: 3,
        vehicles: 12,
        seed: 42,
    }
}

fn json(problem: &domain::Problem) -> String {
    serde_json::to_string(problem).expect("problem serialises to JSON")
}

#[test]
fn identical_options_produce_byte_identical_problems() {
    let first = json(&generate(&reference_options()));
    let second = json(&generate(&reference_options()));
    assert_eq!(first, second);
}

#[test]
fn a_different_seed_produces_a_different_problem() {
    let mut other = reference_options();
    other.seed += 1;
    let first = json(&generate(&reference_options()));
    let second = json(&generate(&other));
    assert_ne!(first, second);
}

#[test]
fn reference_scenario_is_structurally_sound() {
    let problem = generate(&reference_options());

    assert_eq!(
        problem.name,
        "sim coles sydney stores=40 depots=3 vehicles=12 seed=42"
    );
    assert_eq!(problem.depots.len(), 3);
    assert_eq!(problem.stores.len(), 40);
    assert_eq!(problem.vehicles.len(), 12);
    assert_eq!(problem.drivers.len(), problem.vehicles.len());

    for index in 0..problem.depots.len() as u32 {
        assert!(problem.depot(DepotId(index)).is_some(), "depot {index}");
    }
    for index in 0..problem.stores.len() as u32 {
        assert!(problem.store(StoreId(index)).is_some(), "store {index}");
    }
    for index in 0..problem.vehicles.len() as u32 {
        assert!(
            problem.vehicle(VehicleId(index)).is_some(),
            "vehicle {index}"
        );
        assert!(problem.driver(DriverId(index)).is_some(), "driver {index}");
    }

    for vehicle in &problem.vehicles {
        assert!(
            problem.depot(vehicle.depot).is_some(),
            "vehicle {} references unknown depot {}",
            vehicle.id,
            vehicle.depot
        );
    }

    let total_demand: u64 = problem
        .stores
        .iter()
        .map(|store| store.demand.total())
        .sum();
    assert!(total_demand > 0, "generated network has no demand");

    let sydney = LatLon::new(-33.868, 151.209);
    for store in &problem.stores {
        assert!(
            !store.receiving.is_empty(),
            "store {} has no receiving window",
            store.id
        );
        for window in &store.receiving {
            assert!(
                window.start < window.end,
                "store {} window {window:?}",
                store.id
            );
        }
        assert!(
            (store.location.lat - sydney.lat).abs() < 1.5,
            "store {} latitude {} is far from Sydney",
            store.id,
            store.location.lat
        );
        assert!(
            (store.location.lon - sydney.lon).abs() < 1.5,
            "store {} longitude {} is far from Sydney",
            store.id,
            store.location.lon
        );
    }
}

#[test]
fn counts_below_one_are_clamped() {
    let problem = generate(&SimOptions {
        chain: Chain::Iga,
        city: City::Adelaide,
        stores: 0,
        depots: 0,
        vehicles: 0,
        seed: 7,
    });

    assert_eq!(problem.stores.len(), 1);
    assert_eq!(problem.depots.len(), 1);
    assert_eq!(problem.vehicles.len(), 1);
    assert_eq!(problem.drivers.len(), 1);
    assert_eq!(
        problem.name,
        "sim iga adelaide stores=1 depots=1 vehicles=1 seed=7"
    );
}

#[test]
fn cities_are_five_with_lowercase_names() {
    assert_eq!(City::ALL.len(), 5);
    let mut names: Vec<&str> = City::ALL.iter().map(|city| city.name()).collect();
    for name in &names {
        assert!(!name.is_empty());
        assert!(
            name.chars().all(|c| c.is_ascii_lowercase()),
            "city name {name:?} is not lowercase"
        );
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 5, "city names must be distinct");
}

#[test]
fn fleet_cycles_classes_depots_and_shifts_as_fixtured() {
    let problem = generate(&reference_options());
    let classes = [
        Load::new(18, 12, 10),
        Load::new(28, 18, 16),
        Load::new(20, 14, 0),
    ];

    for (index, vehicle) in problem.vehicles.iter().enumerate() {
        assert_eq!(vehicle.id, VehicleId(index as u32));
        assert_eq!(vehicle.name, format!("Vehicle {:03}", index + 1));
        assert_eq!(vehicle.capacity, classes[index % classes.len()]);
        assert_eq!(
            vehicle.depot,
            DepotId((index % problem.depots.len()) as u32)
        );
        assert_eq!(vehicle.max_duration, Minutes::from_minutes(480));
        assert_eq!(vehicle.avg_speed_kmh, 45.0);
    }

    for (index, driver) in problem.drivers.iter().enumerate() {
        assert_eq!(driver.id, DriverId(index as u32));
        assert_eq!(driver.name, format!("Driver {:03}", index + 1));
        assert_eq!(driver.shifts.len(), 1);
        assert_eq!(
            driver.shifts[0],
            Shift::new(Minutes::from_minutes(330), Minutes::from_minutes(870))
        );
    }
}

#[test]
fn stores_follow_banner_naming_demand_and_window_fixtures() {
    let problem = generate(&reference_options());
    for (index, store) in problem.stores.iter().enumerate() {
        assert_eq!(store.id, StoreId(index as u32));
        assert_eq!(store.chain, Chain::Coles);
        assert_eq!(store.name, format!("Coles Sydney #{:03}", index + 1));
        assert!(
            store.demand.ambient <= 8,
            "store {} ambient demand",
            store.id
        );
        assert!(
            store.demand.chilled <= 6,
            "store {} chilled demand",
            store.id
        );
        assert!(store.demand.frozen <= 4, "store {} frozen demand", store.id);
        assert!(
            store.demand.total() >= 1,
            "store {} has no demand",
            store.id
        );
        assert!(
            (15..=30).contains(&store.service.minutes()),
            "store {} service {}",
            store.id,
            store.service
        );
        assert_eq!(
            store.service.minutes() % 5,
            0,
            "store {} service must be a multiple of 5 minutes",
            store.id
        );
        assert_eq!(store.receiving.len(), 1, "store {} windows", store.id);
    }
}

#[test]
fn depots_sit_on_the_fringe_ring() {
    let problem = generate(&reference_options());
    let centre = LatLon::new(-33.868, 151.209);
    for (index, depot) in problem.depots.iter().enumerate() {
        assert_eq!(depot.id, DepotId(index as u32));
        assert_eq!(depot.name, format!("Sydney DC {}", index + 1));
        let distance_km = haversine_km(centre, depot.location);
        assert!(
            (17.9..=30.1).contains(&distance_km),
            "depot {} is {distance_km:.2} km from the city centre",
            depot.id
        );
    }
}
