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
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Problem, ReturnablesDefaults, Shift, Store, StoreId, TimeWindow,
    Vehicle, VehicleId,
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

    /// IANA timezone for the city (Brisbane and Perth do not observe DST).
    pub fn iana_timezone(self) -> &'static str {
        match self {
            Self::Sydney => "Australia/Sydney",
            Self::Melbourne => "Australia/Melbourne",
            Self::Brisbane => "Australia/Brisbane",
            Self::Perth => "Australia/Perth",
            Self::Adelaide => "Australia/Adelaide",
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
    /// Number of vehicles (one driver is generated per vehicle; clamped to
    /// at least 1).
    pub vehicles: usize,
    /// Random seed; the same seed always produces the same problem.
    pub seed: u64,
    /// Apply the Christmas-peak trading profile: higher demand and wider
    /// receiving windows (see [`apply_christmas_peak`]). Illustrative
    /// fixture parameters, not researched operator data.
    pub christmas_peak: bool,
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

/// Illustrative Christmas-peak demand uplift per compartment (ambient,
/// chilled, frozen). Fixture parameters, not researched operator data.
const PEAK_DEMAND_FACTORS: [f64; 3] = [1.5, 1.4, 1.3];

/// Peak-season receiving windows open this much earlier (clamped to the start
/// of the horizon).
const PEAK_WINDOW_OPEN_EARLIER: i64 = 60;

/// Peak-season receiving windows close this much later, reflecting the extra
/// delivery slots chains run in December.
const PEAK_WINDOW_CLOSE_LATER: i64 = 120;

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
/// With `christmas_peak`, stores get the peak trading profile (higher demand,
/// wider receiving windows) before the problem is assembled.
/// Identical options always produce an identical [`Problem`].
pub fn generate(options: &SimOptions) -> Problem {
    let store_count = options.stores.max(1);
    let depot_count = options.depots.max(1);
    let vehicle_count = options.vehicles.max(1);

    let mut rng = StdRng::seed_from_u64(options.seed);
    let centre = city_centre(options.city);

    let mut stores = build_stores(&mut rng, options.chain, options.city, centre, store_count);
    if options.christmas_peak {
        apply_christmas_peak(&mut stores);
    }

    Problem {
        name: problem_name(options, store_count, depot_count, vehicle_count),
        depots: build_depots(&mut rng, options.city, centre, depot_count),
        stores,
        vehicles: build_vehicles(vehicle_count, depot_count),
        drivers: build_drivers(vehicle_count),
        cost: CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 90,
            per_hour_cents: 4_500,
            late_per_min_cents: 0,
        },
        chain_rules: build_chain_rules(),
        curfews: CurfewRules::default(),
        horizon_start: None,
    }
}

/// Illustrative per-banner returnables fixture: empties (roll cages) the
/// stores hand back on each visit. Fixture parameters, not researched
/// operator data.
fn build_chain_rules() -> ChainRules {
    ChainRules {
        returnables: ReturnablesDefaults {
            coles: Load::new(3, 2, 2),
            woolworths: Load::new(3, 2, 2),
            aldi: Load::new(2, 1, 1),
            iga: Load::new(2, 1, 1),
        },
        ..ChainRules::default()
    }
}

/// Applies the Christmas-peak trading profile in place: a per-compartment
/// demand uplift (rounded up) and wider receiving windows.
///
/// Illustrative fixture parameters, not researched operator data: the point
/// is a deterministic high-pressure scenario that stresses vehicle capacity
/// and receiving windows.
fn apply_christmas_peak(stores: &mut [Store]) {
    for store in stores {
        store.demand = Load::new(
            uplift(store.demand.ambient, PEAK_DEMAND_FACTORS[0]),
            uplift(store.demand.chilled, PEAK_DEMAND_FACTORS[1]),
            uplift(store.demand.frozen, PEAK_DEMAND_FACTORS[2]),
        );
        for window in &mut store.receiving {
            window.start =
                Minutes::from_minutes((window.start.minutes() - PEAK_WINDOW_OPEN_EARLIER).max(0));
            window.end = Minutes::from_minutes(window.end.minutes() + PEAK_WINDOW_CLOSE_LATER);
        }
    }
}

/// Multiplies a handling-unit count by `factor`, rounding up. Never decreases
/// the count.
fn uplift(units: u32, factor: f64) -> u32 {
    ((f64::from(units) * factor).ceil() as u32).max(units)
}

/// The scenario label, e.g. `sim coles sydney stores=40 depots=3 vehicles=12 seed=42`.
fn problem_name(options: &SimOptions, stores: usize, depots: usize, vehicles: usize) -> String {
    format!(
        "sim {} {} stores={stores} depots={depots} vehicles={vehicles} seed={}{}",
        chain_slug(options.chain),
        options.city.name(),
        options.seed,
        if options.christmas_peak {
            " profile=christmas-peak"
        } else {
            ""
        },
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
        Chain::General => "general",
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

/// Approximate Sydney coastline: for each latitude, the easternmost longitude
/// that is still land. East of it is the Tasman Sea. A coarse piecewise-linear
/// fit between known coastal points — only used to keep synthetic depots and
/// stores out of the ocean (the project is NSW-only, so other cities are left
/// unclamped). Public so the generator tests can assert against the same
/// coastline the generator uses.
pub fn sydney_coast_lon(lat: f64) -> f64 {
    // (latitude, coast longitude), north to south.
    const COAST: [(f64, f64); 11] = [
        (-33.80, 151.29),
        (-33.85, 151.28),
        (-33.87, 151.31),
        (-33.90, 151.31),
        (-33.95, 151.28),
        (-34.00, 151.20),
        (-34.05, 151.16),
        (-34.10, 151.11),
        (-34.15, 151.07),
        (-34.20, 151.03),
        (-34.25, 150.90),
    ];
    if lat >= COAST[0].0 {
        return COAST[0].1;
    }
    if lat <= COAST[COAST.len() - 1].0 {
        return COAST[COAST.len() - 1].1;
    }
    for pair in COAST.windows(2) {
        let (lat1, lon1) = pair[0];
        let (lat2, lon2) = pair[1];
        if lat1 >= lat && lat >= lat2 {
            let t = (lat - lat1) / (lat2 - lat1);
            return lon1 + t * (lon2 - lon1);
        }
    }
    COAST[COAST.len() - 1].1
}

/// Whether a generated point is on land. Only Sydney is modelled; other
/// cities are treated as all-land (out of the NSW scope).
fn on_land(city: City, point: LatLon) -> bool {
    city != City::Sydney || point.lon <= sydney_coast_lon(point.lat)
}

/// Clamp an east/north offset (km from `centre`) so the point is on land,
/// mirroring it across the north–south axis through `centre` when it would
/// otherwise fall in the sea. Mirroring preserves the distance from `centre`,
/// which the fringe-ring depot invariant depends on.
fn clamp_offset_to_land(city: City, centre: LatLon, east_km: f64, north_km: f64) -> (f64, f64) {
    if on_land(city, offset_km(centre, east_km, north_km)) {
        return (east_km, north_km);
    }
    (-east_km, north_km)
}

/// Clamp an absolute point onto land by reflecting its longitude across the
/// coastline (used for stores, which are placed by absolute coordinate).
fn clamp_point_to_land(city: City, point: LatLon) -> LatLon {
    if on_land(city, point) {
        return point;
    }
    let coast = sydney_coast_lon(point.lat);
    let lon = (2.0 * coast - point.lon).min(coast - 1e-3);
    LatLon::new(point.lat, lon)
}

/// Depots on a ring 18–30 km from the city centre, spread evenly around it
/// with a small angular jitter. Points that would fall in the sea are mirrored
/// inland so the ring stays on land.
fn build_depots(rng: &mut StdRng, city: City, centre: LatLon, count: usize) -> Vec<Depot> {
    let display = city_display(city);
    let mut depots = Vec::with_capacity(count);
    for index in 0..count {
        let bearing = TAU * index as f64 / count as f64
            + rng.random_range(-DEPOT_ANGLE_JITTER..=DEPOT_ANGLE_JITTER);
        let radius_km = rng.random_range(18.0..30.0);
        let (east_km, north_km) = clamp_offset_to_land(
            city,
            centre,
            radius_km * bearing.cos(),
            radius_km * bearing.sin(),
        );
        depots.push(Depot {
            id: DepotId(index as u32),
            name: format!("{display} DC {}", index + 1),
            location: offset_km(centre, east_km, north_km),
            address: None,
            timezone: None,
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

    // Illustrative receiving windows in minutes from 00:00 —
    // synthetic fixtures, not researched operator data: General 06:00–18:00,
    // Coles 06:00–16:00, Woolworths 05:30–15:00, ALDI 07:00–13:00, IGA 07:00–17:00.
    let (base_start, base_end) = match chain {
        Chain::General => (360, 1080),
        Chain::Coles => (360, 960),
        Chain::Woolworths => (330, 900),
        Chain::Aldi => (420, 780),
        Chain::Iga => (420, 1020),
    };

    let display = city_display(city);
    let mut stores = Vec::with_capacity(count);
    for index in 0..count {
        let cluster = clusters[index % cluster_count];
        let location = clamp_point_to_land(
            city,
            offset_km(
                cluster,
                standard_normal(rng) * STORE_SCATTER_KM,
                standard_normal(rng) * STORE_SCATTER_KM,
            ),
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

        let mut start = base_start + rng.random_range(-WINDOW_JITTER..=WINDOW_JITTER);
        let mut end = base_end + rng.random_range(-WINDOW_JITTER..=WINDOW_JITTER);
        if start >= end {
            // Jitter could theoretically invert bounds; swap to keep valid window
            (start, end) = (end, start);
        }

        let store_name = match chain {
            Chain::General => format!("{display} Stop #{:03}", index + 1),
            _ => format!("{chain} {display} #{:03}", index + 1),
        };
        stores.push(Store {
            id: StoreId(index as u32),
            chain,
            name: store_name,
            location,
            address: None,
            timezone: None,
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
    if depot_count == 0 {
        panic!("generate clamps the depot count to >= 1");
    };
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
            fatigue_rules: FatigueRules::default(),
        })
        .collect()
}

// The parser tests for `parse_solomon_file` / `solomon_to_problem` live in
// `solver::parser`, the crate that owns them; they are not duplicated here.
