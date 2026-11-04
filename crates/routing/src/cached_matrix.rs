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

/// Key for caching: a problem's node set plus a truck profile.
///
/// The matrix depends on which nodes exist and which truck profile is used
/// (different profiles may be forbidden from different edges).
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct MatrixKey {
    /// Sorted list of node ids (depots then stores) that define the matrix.
    pub node_ids: Vec<NodeId>,
    pub profile: TruckProfile,
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
    matrices: HashMap<MatrixKey, Vec<MatrixEntry>>,
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

    /// Returns the number of nodes in the loaded graph.
    pub fn graph_node_count(&self) -> usize {
        self.graph.as_ref().map_or(0, |g| g.node_count())
    }

    /// Computes (or retrieves from cache) the distance/duration matrix for
    /// `problem` using `profile` truck constraints.
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
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no OSM graph loaded"))?;

        // Map each problem node to a graph node id. Problem nodes are lat/lon
        // coordinates; we need to find the nearest graph node for each.
        let node_locations: Vec<LatLon> = problem
            .depots
            .iter()
            .map(|d| d.location)
            .chain(problem.stores.iter().map(|s| s.location))
            .collect();

        let node_ids: Vec<NodeId> = node_locations
            .iter()
            .map(|loc| nearest_graph_node(graph, *loc))
            .collect();

        let key = MatrixKey {
            node_ids: node_ids.clone(),
            profile,
        };

        if let Some(cached) = self.matrices.get(&key) {
            return Ok(Some(entries_to_matrix(cached, node_ids.len())));
        }

        let n = node_ids.len();
        let mut matrix: Vec<Vec<Option<MatrixEntry>>> = vec![vec![None; n]; n];

        for i in 0..n {
            matrix[i][i] = Some(MatrixEntry {
                distance_km: 0.0,
                duration_minutes: 0,
            });
            for j in (i + 1)..n {
                let result = Router::shortest_path(graph, node_ids[i], node_ids[j], profile);
                let entry = result.map(|r| MatrixEntry {
                    distance_km: r.distance_km,
                    duration_minutes: r.duration_minutes,
                });
                matrix[i][j] = entry;
                // Roads are generally bidirectional; cache both directions.
                matrix[j][i] = entry;
            }
        }

        // Flatten for caching.
        let flat: Vec<MatrixEntry> = matrix.iter().flatten().filter_map(|e| *e).collect();
        self.matrices.insert(key, flat);

        Ok(Some(matrix))
    }

    /// Returns the number of cached matrices.
    pub fn cached_count(&self) -> usize {
        self.matrices.len()
    }
}

/// Converts a flat cache entry list back into a 2D matrix.
fn entries_to_matrix(flat: &[MatrixEntry], n: usize) -> Vec<Vec<Option<MatrixEntry>>> {
    let mut matrix: Vec<Vec<Option<MatrixEntry>>> = vec![vec![None; n]; n];
    let mut idx = 0;
    for i in 0..n {
        for j in 0..n {
            if idx < flat.len() {
                matrix[i][j] = Some(flat[idx]);
            }
            idx += 1;
        }
    }
    matrix
}

/// Finds the nearest graph node to `location` using haversine distance.
fn nearest_graph_node(graph: &Graph, location: LatLon) -> NodeId {
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
        .expect("graph must have at least one node")
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
