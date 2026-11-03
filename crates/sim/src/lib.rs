//! Synthetic scenario generator for Australian grocery distribution networks.
//!
//! Generates reproducible (seeded) [`domain::Problem`] instances that mimic a
//! retail distribution network around one of the five mainland state capitals:
//! stores clustered by suburb, distribution centres on the urban fringe, and a
//! mixed-temperature fleet with one driver per vehicle.
//!
//! All generated places, names and parameters are synthetic fixtures, not real
//! addresses or researched operating data.
//!
//! Requested counts below one are clamped to one: [`generate`] always emits at
//! least one store, one depot and one vehicle with its driver, and the problem
//! name reports those effective counts.

use std::f64::consts::TAU;

use domain::{
    Chain, CostModel, Depot, DepotId, Driver, DriverId, LatLon, Load, Minutes, Problem, Shift,
    Store, StoreId, TimeWindow, Vehicle, VehicleId,
};
use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;

/// One of the five mainland state capitals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum City {
    Sydney,
    Melbourne,
    Brisbane,
    Perth,
    Adelaide,
}

impl City {
    /// Every city, in a stable order.
    pub const ALL: [Self; 5] = [
        Self::Sydney,
        Self::Melbourne,
        Self::Brisbane,
        Self::Perth,
        Self::Adelaide,
    ];

    /// The city name as used on the command line and in generated names.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sydney => "sydney",
            Self::Melbourne => "melbourne",
            Self::Brisbane => "brisbane",
            Self::Perth => "perth",
            Self::Adelaide => "adelaide",
        }
    }
}

/// Knobs for [`generate`].
#[derive(Clone, Copy, Debug)]
pub struct SimOptions {
    /// The banner whose stores are generated.
    pub chain: Chain,
    /// The city the network is generated around.
    pub city: City,
    /// Number of stores (clamped to at least 1).
    pub stores: usize,
    /// Number of distribution centres (clamped to at least 1).
    pub depots: usize,
    /// Number of vehicles (one driver is generated per vehicle; clamped to at
    /// least 1).
    pub vehicles: usize,
    /// Random seed; the same seed always produces the same problem.
    pub seed: u64,
}

/// Approximate kilometres per degree of latitude; longitude degrees shrink by
/// the cosine of the latitude.
const KM_PER_DEGREE: f64 = 111.0;

/// Standard deviation of the spread of stores around their cluster centre, km.
const STORE_SCATTER_KM: f64 = 5.0;

/// Maximum jitter applied to the evenly-spread depot bearings, in radians.
const DEPOT_ANGLE_JITTER: f64 = 0.2;

/// Maximum jitter applied to each bound of a receiving window, in minutes.
const WINDOW_JITTER: i64 = 30;

/// Illustrative fleet mix cycled by vehicle index: tri-temp, tri-temp large
/// and dual-temp capacities, as (ambient, chilled, frozen) handling units.
const VEHICLE_CAPACITIES: [Load; 3] = [
    Load::new(18, 12, 10),
    Load::new(28, 18, 16),
    Load::new(20, 14, 0),
];

/// Generates a deterministic synthetic problem from `options`.
///
/// Layout: up to four cluster centres are scattered 0–35 km around the city
/// centre and the stores are spread around them with roughly 5 km of spread.
/// Depots sit on a ring 18–30 km from the centre. Every store gets one
/// illustrative receiving window; the fleet cycles through three temperature
/// classes with one driver per vehicle on a single 05:30–14:30 shift.
///
/// Counts are clamped to at least one (`stores.max(1)`, `depots.max(1)`,
/// `vehicles.max(1)`), and [`Problem::name`] reports those effective counts.
/// Identical options always produce an identical [`Problem`].
pub fn generate(options: &SimOptions) -> Problem {
    let store_count = options.stores.max(1);
    let depot_count = options.depots.max(1);
    let vehicle_count = options.vehicles.max(1);

    let mut rng = StdRng::seed_from_u64(options.seed);
    let centre = city_centre(options.city);

    Problem {
        name: problem_name(options, store_count, depot_count, vehicle_count),
        depots: build_depots(&mut rng, options.city, centre, depot_count),
        stores: build_stores(&mut rng, options.chain, options.city, centre, store_count),
        vehicles: build_vehicles(vehicle_count, depot_count),
        drivers: build_drivers(vehicle_count),
        cost: CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 90,
            per_hour_cents: 4_500,
            late_per_min_cents: 0,
        },
    }
}

/// The scenario label, e.g. `sim coles sydney stores=40 depots=3 vehicles=12 seed=42`.
fn problem_name(options: &SimOptions, stores: usize, depots: usize, vehicles: usize) -> String {
    format!(
        "sim {} {} stores={stores} depots={depots} vehicles={vehicles} seed={}",
        chain_slug(options.chain),
        options.city.name(),
        options.seed,
    )
}

/// The approximate city-centre coordinate of each capital.
fn city_centre(city: City) -> LatLon {
    match city {
        City::Sydney => LatLon::new(-33.868, 151.209),
        City::Melbourne => LatLon::new(-37.814, 144.963),
        City::Brisbane => LatLon::new(-27.470, 153.021),
        City::Perth => LatLon::new(-31.953, 115.857),
        City::Adelaide => LatLon::new(-34.929, 138.601),
    }
}

/// The capitalised city name used in generated display names ("Sydney", ...).
fn city_display(city: City) -> &'static str {
    match city {
        City::Sydney => "Sydney",
        City::Melbourne => "Melbourne",
        City::Brisbane => "Brisbane",
        City::Perth => "Perth",
        City::Adelaide => "Adelaide",
    }
}

/// The lowercase banner slug used in the scenario label.
fn chain_slug(chain: Chain) -> &'static str {
    match chain {
        Chain::Coles => "coles",
        Chain::Woolworths => "woolworths",
        Chain::Aldi => "aldi",
        Chain::Iga => "iga",
    }
}

/// A standard normal sample built from two uniforms (Box–Muller transform),
/// keeping `rand_distr` out of the dependency tree.
fn standard_normal(rng: &mut StdRng) -> f64 {
    let u1 = rng.random_range(f64::MIN_POSITIVE..1.0);
    let u2 = rng.random_range(0.0..1.0);
    (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
}

/// A point `east_km` east and `north_km` north of `origin`.
///
/// Offsets are converted with ≈111 km per degree of latitude, scaled by the
/// cosine of `origin`'s latitude for longitude.
fn offset_km(origin: LatLon, east_km: f64, north_km: f64) -> LatLon {
    let dlat = north_km / KM_PER_DEGREE;
    let dlon = east_km / (KM_PER_DEGREE * origin.lat.to_radians().cos());
    LatLon::new(origin.lat + dlat, origin.lon + dlon)
}

/// Depots on a ring 18–30 km from the city centre, spread evenly around it
/// with a small angular jitter.
fn build_depots(rng: &mut StdRng, city: City, centre: LatLon, count: usize) -> Vec<Depot> {
    let display = city_display(city);
    let mut depots = Vec::with_capacity(count);
    for index in 0..count {
        let bearing = TAU * index as f64 / count as f64
            + rng.random_range(-DEPOT_ANGLE_JITTER..=DEPOT_ANGLE_JITTER);
        let radius_km = rng.random_range(18.0..30.0);
        depots.push(Depot {
            id: DepotId(index as u32),
            name: format!("{display} DC {}", index + 1),
            location: offset_km(centre, radius_km * bearing.cos(), radius_km * bearing.sin()),
        });
    }
    depots
}

/// Stores gathered around up to four cluster centres that are scattered
/// 0–35 km from the city centre; each store is displaced from its cluster by a
/// normal draw with roughly 5 km of spread.
fn build_stores(
    rng: &mut StdRng,
    chain: Chain,
    city: City,
    centre: LatLon,
    count: usize,
) -> Vec<Store> {
    // Up to four suburb-scale clusters; always at least one.
    let cluster_count = count.clamp(1, 4);
    let mut clusters = Vec::with_capacity(cluster_count);
    for _ in 0..cluster_count {
        let bearing = rng.random_range(0.0..TAU);
        let radius_km = rng.random_range(0.0..35.0);
        clusters.push(offset_km(
            centre,
            radius_km * bearing.cos(),
            radius_km * bearing.sin(),
        ));
    }

    // Illustrative per-banner receiving windows in minutes from 00:00 —
    // synthetic fixtures, not researched operator data: Coles 06:00–16:00,
    // Woolworths 05:30–15:00, ALDI 07:00–13:00, IGA 07:00–17:00.
    let (base_start, base_end) = match chain {
        Chain::Coles => (360, 960),
        Chain::Woolworths => (330, 900),
        Chain::Aldi => (420, 780),
        Chain::Iga => (420, 1020),
    };

    let display = city_display(city);
    let mut stores = Vec::with_capacity(count);
    for index in 0..count {
        let cluster = clusters[index % cluster_count];
        let location = offset_km(
            cluster,
            standard_normal(rng) * STORE_SCATTER_KM,
            standard_normal(rng) * STORE_SCATTER_KM,
        );

        let mut demand = Load::new(
            rng.random_range(0..=8),
            rng.random_range(0..=6),
            rng.random_range(0..=4),
        );
        if demand.total() == 0 {
            // Every store must need at least something delivered.
            demand.ambient = 1;
        }

        let start = base_start + rng.random_range(-WINDOW_JITTER..=WINDOW_JITTER);
        let end = base_end + rng.random_range(-WINDOW_JITTER..=WINDOW_JITTER);
        // The shortest base window is 6 h long and the jitter is ±30 min per
        // bound, so `start < end` cannot invert; the assertion documents it.
        debug_assert!(start < end, "receiving window {start}..{end}");

        stores.push(Store {
            id: StoreId(index as u32),
            chain,
            name: format!("{chain} {display} #{:03}", index + 1),
            location,
            demand,
            service: Minutes::from_minutes(rng.random_range(3..=6) * 5),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(start),
                Minutes::from_minutes(end),
            )],
        });
    }
    stores
}

/// The fleet: three illustrative temperature classes cycled by index, each
/// vehicle assigned to a depot round-robin.
fn build_vehicles(count: usize, depot_count: usize) -> Vec<Vehicle> {
    debug_assert!(depot_count > 0, "generate clamps the depot count to >= 1");
    (0..count)
        .map(|index| Vehicle {
            id: VehicleId(index as u32),
            name: format!("Vehicle {:03}", index + 1),
            depot: DepotId((index % depot_count) as u32),
            capacity: VEHICLE_CAPACITIES[index % VEHICLE_CAPACITIES.len()],
            max_duration: Minutes::from_minutes(480),
            avg_speed_kmh: 45.0,
        })
        .collect()
}

/// One driver per vehicle on a single 05:30–14:30 shift; the driver and
/// vehicle ids share the fleet index.
fn build_drivers(count: usize) -> Vec<Driver> {
    (0..count)
        .map(|index| Driver {
            id: DriverId(index as u32),
            name: format!("Driver {:03}", index + 1),
            shifts: vec![Shift::new(
                Minutes::from_minutes(330),
                Minutes::from_minutes(870),
            )],
        })
        .collect()
}
