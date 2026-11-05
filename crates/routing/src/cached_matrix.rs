//! Cached distance/duration matrices keyed by problem + vehicle profile.
//!
//! The solver needs a distance matrix between every pair of problem nodes
/// (depots + stores). M3 replaces haversine estimates with real road-network
/// distances from [`crate::graph::Graph`], computed via [`crate::router::Router`]
/// and cached so repeated solves on the same problem don't re-route.
use std::collections::HashMap;

use domain::{LatLon, Problem};

use crate::graph::{Graph, NodeId};
use crate::osm_ingest::ingest_pbf;
use crate::router::Router;
use crate::truck_constraints::TruckProfile;

/// A distance/duration matrix entry between two problem nodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatrixEntry {
    /// Road distance in kilometres.
    pub distance_km: f64,
    /// Estimated driving time in whole minutes.
    pub duration_minutes: i64,
}

/// How shortest paths are computed when a matrix is built.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum MatrixStrategy {
    /// One Dijkstra per source (distance-optimal, the default).
    #[default]
    Dijkstra,
    /// One contraction-hierarchy preparation plus `n²` fast queries
    /// (time-optimal; see [`crate::ch`]).
    ContractionHierarchy,
}

/// Key for caching: a problem's node set plus a truck profile.
///
/// The matrix depends on which nodes exist, which truck profile is used
/// (different profiles may be forbidden from different edges) and how paths
/// are computed (see [`MatrixStrategy`]).
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct MatrixKey {
    /// Sorted list of node ids (depots then stores) that define the matrix.
    pub node_ids: Vec<NodeId>,
    pub profile: TruckProfile,
    pub strategy: MatrixStrategy,
}

/// Builds and caches road-network distance/duration matrices.
///
/// The first call for a given `(problem, profile)` ingests the OSM extract
/// (if not already done), builds the graph, and runs all-pairs shortest paths.
/// Subsequent calls return the cached matrix.
#[derive(Clone, Debug, Default)]
pub struct CachedMatrix {
    graph: Option<Graph>,
    pbf_path: Option<std::path::PathBuf>,
    matrices: HashMap<MatrixKey, Vec<Option<MatrixEntry>>>,
    node_count: usize,
}

impl CachedMatrix {
    /// Creates an empty cache with no graph loaded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads an OSM PBF extract into the cache's graph. Replaces any previous
    /// graph and clears all cached matrices.
    pub fn load_pbf(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        let graph = ingest_pbf(path)?;
        self.graph = Some(graph);
        self.pbf_path = Some(path.to_path_buf());
        self.matrices.clear();
        self.node_count = 0;
        Ok(())
    }

    /// Returns a reference to the loaded graph, if any.
    pub fn graph(&self) -> Option<&Graph> {
        self.graph.as_ref()
    }

    /// Returns a mutable reference to the loaded graph, if any.
    ///
    /// Used to fold curated constraints into the graph *before* building
    /// matrices. Calling this clears any cached matrices, because they may
    /// have been built from the unconstrained graph.
    pub fn graph_mut(&mut self) -> Option<&mut Graph> {
        self.matrices.clear();
        self.graph.as_mut()
    }

    /// Returns the number of nodes in the loaded graph.
    pub fn graph_node_count(&self) -> usize {
        self.graph.as_ref().map_or(0, |g| g.node_count())
    }

    /// Maps each problem node (depots first, then stores, in vector order) to
    /// its nearest graph node — the same mapping [`CachedMatrix::matrix_for`]
    /// uses.
    pub fn problem_node_ids(&self, problem: &Problem) -> anyhow::Result<Vec<NodeId>> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no OSM graph loaded"))?;
        let node_locations: Vec<LatLon> = problem
            .depots
            .iter()
            .map(|d| d.location)
            .chain(problem.stores.iter().map(|s| s.location))
            .collect();
        node_locations
            .iter()
            .map(|loc| nearest_graph_node(graph, *loc))
            .collect::<anyhow::Result<_>>()
    }

    /// Computes (or retrieves from cache) the distance/duration matrix for
    /// `problem` using `profile` truck constraints and [`MatrixStrategy::Dijkstra`].
    ///
    /// The matrix is row-major over `problem` nodes: depots first (in vector
    /// order), then stores (in vector order). Entry `[i][j]` is the
    /// distance/duration from node `i` to node `j`.
    ///
    /// Returns `None` for pairs with no road-network path.
    pub fn matrix_for(
        &mut self,
        problem: &Problem,
        profile: TruckProfile,
    ) -> anyhow::Result<Option<Vec<Vec<Option<MatrixEntry>>>>> {
        self.matrix_for_with(problem, profile, MatrixStrategy::Dijkstra)
    }

    /// [`CachedMatrix::matrix_for`] with an explicit path-computation
    /// strategy. Results are cached per strategy.
    pub fn matrix_for_with(
        &mut self,
        problem: &Problem,
        profile: TruckProfile,
        strategy: MatrixStrategy,
    ) -> anyhow::Result<Option<Vec<Vec<Option<MatrixEntry>>>>> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no OSM graph loaded"))?;

        let node_ids: Vec<NodeId> = self.problem_node_ids(problem)?;

        let key = MatrixKey {
            node_ids: node_ids.clone(),
            profile,
            strategy,
        };

        if let Some(cached) = self.matrices.get(&key) {
            return Ok(Some(entries_to_matrix(cached, node_ids.len())));
        }

        let n = node_ids.len();
        let mut matrix: Vec<Vec<Option<MatrixEntry>>> = vec![vec![None; n]; n];

        // For the CH strategy the hierarchy is prepared once and answers
        // every pair; for Dijkstra each pair runs its own search (one-way
        // roads mean i->j and j->i are not symmetric).
        let mut hierarchy = match strategy {
            MatrixStrategy::Dijkstra => None,
            MatrixStrategy::ContractionHierarchy => Some(
                crate::ch::ContractionHierarchy::build(graph, profile)
                    .map_err(|error| anyhow::anyhow!("{error}"))?,
            ),
        };

        for i in 0..n {
            matrix[i][i] = Some(MatrixEntry {
                distance_km: 0.0,
                duration_minutes: 0,
            });
            for j in 0..n {
                if i == j {
                    continue;
                }
                let result = match &mut hierarchy {
                    Some(hierarchy) => hierarchy.shortest_path(node_ids[i], node_ids[j]),
                    None => Router::shortest_path(graph, node_ids[i], node_ids[j], profile),
                };
                matrix[i][j] = result.map(|r| MatrixEntry {
                    distance_km: r.distance_km,
                    duration_minutes: r.duration_minutes,
                });
            }
        }

        // Flatten preserving None positions (unreachable pairs) so the cache
        // round-trips correctly even when the graph is not fully connected.
        let flat: Vec<Option<MatrixEntry>> = matrix.iter().flatten().copied().collect();
        self.matrices.insert(key, flat.clone());

        Ok(Some(matrix))
    }

    /// Returns the number of cached matrices.
    pub fn cached_count(&self) -> usize {
        self.matrices.len()
    }
}

/// Converts a flat cache entry list back into a 2D matrix.
fn entries_to_matrix(flat: &[Option<MatrixEntry>], n: usize) -> Vec<Vec<Option<MatrixEntry>>> {
    let mut matrix: Vec<Vec<Option<MatrixEntry>>> = vec![vec![None; n]; n];
    let mut idx = 0;
    for i in 0..n {
        for j in 0..n {
            if idx < flat.len() {
                matrix[i][j] = flat[idx];
            }
            idx += 1;
        }
    }
    matrix
}

/// Finds the nearest graph node to `location` using haversine distance.
fn nearest_graph_node(graph: &Graph, location: LatLon) -> anyhow::Result<NodeId> {
    let mut best: Option<(NodeId, f64)> = None;
    for (id, node) in graph.nodes() {
        let d = haversine_km(location, node.location);
        match &best {
            None => best = Some((id, d)),
            Some((_, bd)) if d < *bd => best = Some((id, d)),
            _ => {}
        }
    }
    best.map(|(id, _)| id)
        .ok_or_else(|| anyhow::anyhow!("graph must have at least one node"))
}

/// Great-circle distance in kilometres.
fn haversine_km(a: LatLon, b: LatLon) -> f64 {
    const EARTH_RADIUS_KM: f64 = 6371.0088;
    let phi1 = a.lat.to_radians();
    let phi2 = b.lat.to_radians();
    let delta_phi = (b.lat - a.lat).to_radians();
    let delta_lambda = (b.lon - a.lon).to_radians();
    let h = (delta_phi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (delta_lambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().atan2((1.0 - h).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
        LatLon, Load, Minutes, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
    };

    fn test_problem_with_nodes(locations: Vec<LatLon>) -> Problem {
        Problem {
            name: "test".into(),
            depots: locations
                .iter()
                .enumerate()
                .map(|(i, &loc)| Depot {
                    id: DepotId(i as u32),
                    name: format!("Depot {i}"),
                    location: loc,
                    timezone: None,
                })
                .take(1)
                .collect(),
            stores: locations
                .iter()
                .enumerate()
                .skip(1)
                .map(|(i, &loc)| Store {
                    id: StoreId(i as u32),
                    chain: Chain::Coles,
                    name: format!("Store {i}"),
                    location: loc,
                    demand: Load::ZERO,
                    service: Minutes::ZERO,
                    receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(1440))],
                    timezone: None,
                })
                .collect(),
            vehicles: vec![Vehicle {
                id: VehicleId(0),
                name: "Truck".into(),
                depot: DepotId(0),
                capacity: Load::new(8, 8, 4),
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
                fixed_per_route_cents: 0,
                per_km_cents: 0,
                per_hour_cents: 0,
                late_per_min_cents: 0,
            },
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
        }
    }

    fn build_line_graph() -> Graph {
        // Two nodes ~10 km apart at known Sydney coordinates
        let mut g = Graph::new();
        g.add_node(NodeId(0), LatLon::new(-33.87, 151.21)); // node 0
        g.add_node(NodeId(1), LatLon::new(-33.87, 151.30)); // node 1 ~10 km east
        // Add edges in both directions (build_line_graph doesn't go through
        // add_way_to_graph, so we add both directions manually).
        for (from, to) in [(NodeId(0), NodeId(1)), (NodeId(1), NodeId(0))] {
            g.add_edge(
                from,
                to,
                10.0,
                10,
                Some("primary".to_string()),
                None,
                None,
                None,
                None,
                false,
            );
        }
        g
    }

    #[test]
    fn matrix_for_direct_edge_matches() {
        let graph = build_line_graph();
        let mut cache = CachedMatrix::new();
        cache.graph = Some(graph);

        let problem = test_problem_with_nodes(vec![
            LatLon::new(-33.87, 151.21), // depot at node 0
            LatLon::new(-33.87, 151.30), // store at node 1
        ]);

        let profile = TruckProfile::default();
        let matrix = cache
            .matrix_for(&problem, profile)
            .expect("matrix_for should succeed")
            .expect("graph should be loaded");

        // Diagonal: depot->depot is zero
        assert_eq!(
            matrix[0][0],
            Some(MatrixEntry {
                distance_km: 0.0,
                duration_minutes: 0
            })
        );
        // depot->store should match the graph edge (~10 km)
        let entry = matrix[0][1].expect("depot to store should have a path");
        assert!((entry.distance_km - 10.0).abs() < 0.1);
        assert_eq!(entry.duration_minutes, 10);
        // store->depot should be symmetric
        let entry = matrix[1][0].expect("store to depot should have a path");
        assert!((entry.distance_km - 10.0).abs() < 0.1);
    }

    #[test]
    fn matrix_for_unreachable_pair_returns_none() {
        let mut g = Graph::new();
        // Two disconnected nodes
        g.add_node(NodeId(0), LatLon::new(-33.87, 151.21));
        g.add_node(NodeId(1), LatLon::new(-33.87, 151.30));
        // No edges added
        let mut cache = CachedMatrix::new();
        cache.graph = Some(g);

        let problem = test_problem_with_nodes(vec![
            LatLon::new(-33.87, 151.21), // depot at node 0
            LatLon::new(-33.87, 151.30), // store at node 1
        ]);

        let profile = TruckProfile::default();
        let matrix = cache
            .matrix_for(&problem, profile)
            .expect("matrix_for should succeed")
            .expect("graph should be loaded");

        // depot->store should be None (no path)
        assert_eq!(matrix[0][1], None);
        assert_eq!(matrix[1][0], None);
        // Diagonal should still be zero
        assert_eq!(
            matrix[0][0],
            Some(MatrixEntry {
                distance_km: 0.0,
                duration_minutes: 0
            })
        );
    }

    #[test]
    fn matrix_for_cache_hit_returns_same_results() {
        let graph = build_line_graph();
        let mut cache = CachedMatrix::new();
        cache.graph = Some(graph);

        let problem = test_problem_with_nodes(vec![
            LatLon::new(-33.87, 151.21),
            LatLon::new(-33.87, 151.30),
        ]);
        let profile = TruckProfile::default();

        let first = cache
            .matrix_for(&problem, profile)
            .expect("first call should succeed")
            .expect("graph loaded");
        let second = cache
            .matrix_for(&problem, profile)
            .expect("second call should succeed")
            .expect("graph loaded");

        assert_eq!(first, second, "cache hit should return identical matrix");
        assert_eq!(cache.cached_count(), 1, "only one matrix should be cached");
    }

    #[test]
    fn matrix_for_diagonal_is_zero() {
        let graph = build_line_graph();
        let mut cache = CachedMatrix::new();
        cache.graph = Some(graph);

        let problem = test_problem_with_nodes(vec![
            LatLon::new(-33.87, 151.21),
            LatLon::new(-33.87, 151.30),
        ]);

        let profile = TruckProfile::default();
        let matrix = cache
            .matrix_for(&problem, profile)
            .expect("matrix_for should succeed")
            .expect("graph loaded");

        let zero = MatrixEntry {
            distance_km: 0.0,
            duration_minutes: 0,
        };
        assert_eq!(matrix[0][0], Some(zero), "diagonal entry 0 should be zero");
        assert_eq!(matrix[1][1], Some(zero), "diagonal entry 1 should be zero");
    }

    #[test]
    fn matrix_for_truck_profile_blocks_edges() {
        // Graph: 0 -> 1 -> 2, but edge 1->2 is tagged hgv=no
        let mut g = Graph::new();
        g.add_node(NodeId(0), LatLon::new(-33.87, 151.21));
        g.add_node(NodeId(1), LatLon::new(-33.87, 151.30));
        g.add_node(NodeId(2), LatLon::new(-33.87, 151.39));
        g.add_edge(
            NodeId(0),
            NodeId(1),
            10.0,
            10,
            Some("primary".to_string()),
            None,
            None,
            None,
            None,
            false,
        );
        g.add_edge(
            NodeId(1),
            NodeId(2),
            10.0,
            10,
            Some("secondary".to_string()),
            None,
            None,
            Some("no".to_string()),
            None,
            true,
        );
        let mut cache = CachedMatrix::new();
        cache.graph = Some(g);

        let problem = test_problem_with_nodes(vec![
            LatLon::new(-33.87, 151.21), // node 0
            LatLon::new(-33.87, 151.39), // node 2
        ]);

        // Car profile: can traverse hgv=no, path exists 0->1->2
        let car = TruckProfile::default();
        let car_matrix = cache
            .matrix_for(&problem, car)
            .expect("car matrix should succeed")
            .expect("graph loaded");
        assert!(
            car_matrix[0][1].is_some(),
            "car should find path depot->store"
        );

        // HGV profile: blocked by hgv=no edge, no path
        let hgv = TruckProfile {
            hgv: true,
            ..Default::default()
        };
        let hgv_matrix = cache
            .matrix_for(&problem, hgv)
            .expect("hgv matrix should succeed")
            .expect("graph loaded");
        assert_eq!(
            hgv_matrix[0][1], None,
            "HGV should find no path when edge is hgv=no"
        );
    }

    #[test]
    fn contraction_hierarchy_matrix_matches_dijkstra_on_a_line_graph() {
        let graph = build_line_graph();
        let problem = test_problem_with_nodes(vec![
            LatLon::new(-33.87, 151.21),
            LatLon::new(-33.87, 151.30),
        ]);
        let profile = TruckProfile::default();

        let mut cache = CachedMatrix::new();
        cache.graph = Some(graph.clone());
        let dijkstra = cache
            .matrix_for_with(&problem, profile, MatrixStrategy::Dijkstra)
            .expect("dijkstra matrix builds")
            .expect("graph loaded");
        let hierarchy = cache
            .matrix_for_with(&problem, profile, MatrixStrategy::ContractionHierarchy)
            .expect("ch matrix builds")
            .expect("graph loaded");
        assert_eq!(dijkstra, hierarchy);
        // Both strategies are cached separately.
        assert_eq!(cache.cached_count(), 2);
    }

    #[test]
    fn matrix_for_no_graph_returns_error() {
        let mut cache = CachedMatrix::new(); // no graph loaded
        let problem = test_problem_with_nodes(vec![LatLon::new(-33.87, 151.21)]);
        let result = cache.matrix_for(&problem, TruckProfile::default());
        assert!(
            result.is_err(),
            "matrix_for without graph should return an error"
        );
    }
}
