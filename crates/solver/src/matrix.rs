//! Haversine distance matrix and speed-derived travel times.
//!
//! The matrix covers every problem node: all depots first (in vector order),
//! then all stores. Node ids follow the M0 convention of indexing into the
//! corresponding vector on [`domain::Problem`].

use domain::{LatLon, Minutes, Node, Problem, haversine_km};

/// Planning speed used when a vehicle's `avg_speed_kmh` cannot produce a
/// positive speed (zero, negative or NaN).
///
/// `avg_speed_kmh` is an M0 placeholder until the road-network durations of M3
/// arrive. A broken value must not make the vehicle unusable, so the solver
/// falls back to 45 km/h rather than skipping the vehicle entirely.
pub const FALLBACK_SPEED_KMH: f64 = 45.0;

const MINUTES_PER_HOUR: f64 = 60.0;

/// Great-circle distances between every pair of problem nodes.
///
/// Building the matrix costs one haversine call per unordered pair and
/// `O(n²)` memory, which is cheap at M0 problem sizes.
#[derive(Debug, Clone)]
pub struct DistanceMatrix {
    depot_count: usize,
    node_count: usize,
    /// Row-major `node_count × node_count` matrix.
    distances: Vec<f64>,
}

impl DistanceMatrix {
    /// Computes the matrix for every depot and store of `problem`.
    pub fn new(problem: &Problem) -> Self {
        let depot_count = problem.depots.len();
        let node_count = depot_count + problem.stores.len();
        let locations: Vec<LatLon> = problem
            .depots
            .iter()
            .map(|depot| depot.location)
            .chain(problem.stores.iter().map(|store| store.location))
            .collect();

        let mut distances = vec![0.0; node_count * node_count];
        for (from_index, &from) in locations.iter().enumerate() {
            for (to_index, &to) in locations.iter().enumerate().skip(from_index + 1) {
                let km = haversine_km(from, to);
                distances[from_index * node_count + to_index] = km;
                distances[to_index * node_count + from_index] = km;
            }
        }

        Self {
            depot_count,
            node_count,
            distances,
        }
    }

    /// The number of nodes: depots plus stores.
    pub fn len(&self) -> usize {
        self.node_count
    }

    /// True when the problem has no nodes at all.
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Great-circle distance between two nodes in kilometres.
    ///
    /// Returns `None` when a node id does not exist in the problem.
    pub fn distance_km(&self, from: Node, to: Node) -> Option<f64> {
        let from_index = self.node_index(from)?;
        let to_index = self.node_index(to)?;
        Some(self.distances[from_index * self.node_count + to_index])
    }

    fn node_index(&self, node: Node) -> Option<usize> {
        match node {
            Node::Depot(id) => {
                let index = id.0 as usize;
                (index < self.depot_count).then_some(index)
            }
            Node::Store(id) => {
                let index = id.0 as usize;
                let store_count = self.node_count - self.depot_count;
                (index < store_count).then_some(self.depot_count + index)
            }
        }
    }
}

/// Euclidean distance between two points (for Solomon/Euclidean instances).
pub fn euclidean_km(a: LatLon, b: LatLon) -> f64 {
    let dx = a.lat - b.lat;
    let dy = a.lon - b.lon;
    (dx * dx + dy * dy).sqrt()
}

/// Builds a Euclidean distance matrix for `problem` (for Solomon calibration).
pub fn euclidean_matrix(problem: &Problem) -> DistanceMatrix {
    let depot_count = problem.depots.len();
    let node_count = depot_count + problem.stores.len();
    let locations: Vec<LatLon> = problem
        .depots
        .iter()
        .map(|depot| depot.location)
        .chain(problem.stores.iter().map(|store| store.location))
        .collect();

    let mut distances = vec![0.0; node_count * node_count];
    for (from_index, &from) in locations.iter().enumerate() {
        for (to_index, &to) in locations.iter().enumerate().skip(from_index + 1) {
            let km = euclidean_km(from, to);
            distances[from_index * node_count + to_index] = km;
            distances[to_index * node_count + from_index] = km;
        }
    }

    DistanceMatrix {
        depot_count,
        node_count,
        distances,
    }
}

/// Whole minutes to travel `km` at `avg_speed_kmh`, rounded to the nearest
/// minute.
///
/// A non-positive (or NaN) `avg_speed_kmh` plans at [`FALLBACK_SPEED_KMH`].
pub fn travel_minutes(km: f64, avg_speed_kmh: f64) -> Minutes {
    let speed_kmh = if avg_speed_kmh > 0.0 {
        avg_speed_kmh
    } else {
        FALLBACK_SPEED_KMH
    };
    Minutes::from_minutes((km / speed_kmh * MINUTES_PER_HOUR).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Depot, DepotId, Store, StoreId};

    fn two_node_problem() -> Problem {
        Problem {
            name: "two nodes".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "depot".into(),
                location: LatLon::new(-33.87, 151.21),
            }],
            stores: vec![Store {
                id: StoreId(0),
                chain: domain::Chain::Coles,
                name: "store".into(),
                location: LatLon::new(-33.80, 151.15),
                demand: domain::Load::ZERO,
                service: Minutes::ZERO,
                receiving: vec![],
            }],
            vehicles: vec![],
            drivers: vec![],
            cost: domain::CostModel {
                fixed_per_route_cents: 0,
                per_km_cents: 0,
                per_hour_cents: 0,
                late_per_min_cents: 0,
            },
        }
    }

    #[test]
    fn travel_minutes_rounds_to_whole_minutes() {
        assert_eq!(travel_minutes(60.0, 60.0).minutes(), 60);
        assert_eq!(travel_minutes(0.0, 60.0).minutes(), 0);
        // 7.5 km at 45 km/h is exactly 10 minutes.
        assert_eq!(travel_minutes(7.5, 45.0).minutes(), 10);
    }

    #[test]
    fn travel_minutes_falls_back_for_broken_speeds() {
        assert_eq!(travel_minutes(45.0, 0.0).minutes(), 60);
        assert_eq!(travel_minutes(45.0, -10.0).minutes(), 60);
        assert_eq!(travel_minutes(45.0, f64::NAN).minutes(), 60);
    }

    #[test]
    fn matrix_is_symmetric_with_a_zero_diagonal() {
        let problem = two_node_problem();
        let matrix = DistanceMatrix::new(&problem);
        assert_eq!(matrix.len(), 2);
        assert!(!matrix.is_empty());

        let depot = Node::Depot(DepotId(0));
        let store = Node::Store(StoreId(0));
        let outbound = matrix.distance_km(depot, store).expect("depot to store");
        let inbound = matrix.distance_km(store, depot).expect("store to depot");
        assert!((outbound - inbound).abs() < 1e-12);
        assert!(outbound > 0.0);
        assert_eq!(matrix.distance_km(depot, depot), Some(0.0));
    }

    #[test]
    fn unknown_nodes_have_no_distance() {
        let problem = two_node_problem();
        let matrix = DistanceMatrix::new(&problem);
        let unknown = Node::Store(StoreId(7));
        assert_eq!(matrix.distance_km(unknown, Node::Depot(DepotId(0))), None);
        assert_eq!(matrix.distance_km(Node::Depot(DepotId(3)), unknown), None);
    }
}
