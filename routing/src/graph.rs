//! Road-network graph: nodes (OSM positions) and directed edges (way segments).
//!
//! Nodes are keyed by OSM node id (`NodeId`). Edges carry distance, duration,
//! and the OSM tags needed for truck-aware filtering.

use std::collections::HashMap;

use domain::LatLon;

/// Identifier for a graph node (OSM node id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// Identifier for a graph edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeId(pub u64);

/// A node in the road network with a geographic position.
#[derive(Clone, Debug)]
pub struct GraphNode {
    pub id: NodeId,
    pub location: LatLon,
}

/// A directed edge between two graph nodes.
#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub id: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    /// Great-circle distance along this segment, in kilometres.
    pub distance_km: f64,
    /// Estimated travel time in whole minutes.
    pub duration_minutes: i64,
    /// OSM highway tag (e.g. "motorway", "primary", "residential").
    pub highway: Option<String>,
    /// Maximum weight in tonnes, if tagged (`maxweight`).
    pub maxweight_tonnes: Option<f64>,
    /// Maximum height in metres, if tagged (`maxheight`).
    pub maxheight_m: Option<f64>,
    /// Raw OSM `hgv` tag value (e.g. "yes", "no", "delivery"), if tagged.
    pub hgv: Option<String>,
    /// Raw OSM `access` tag value (e.g. "no", "private"), if tagged.
    pub access: Option<String>,
    /// True when the way is one-way in the direction stored (from -> to).
    pub oneway: bool,
    /// OSM way id this edge was derived from, if any.
    ///
    /// `None` for synthetic edges built by tests or tools that do not model
    /// OSM ways. Used to attach curated, hand-verified constraints to a way
    /// (see `curated` module) independently of its tags.
    pub way_id: Option<i64>,
}

/// An adjacency-list road network.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    nodes: HashMap<NodeId, GraphNode>,
    /// Outgoing edges per node: `node_id -> Vec<(edge_id, target_node_id)>`.
    out_edges: HashMap<NodeId, Vec<(EdgeId, NodeId)>>,
    edges: HashMap<EdgeId, GraphEdge>,
    next_edge_id: u64,
}

impl Graph {
    /// Creates an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns the number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Adds a node to the graph. Returns `false` if the node already exists.
    pub fn add_node(&mut self, id: NodeId, location: LatLon) -> bool {
        if self.nodes.contains_key(&id) {
            return false;
        }
        self.nodes.insert(id, GraphNode { id, location });
        true
    }

    /// Looks up a node by id.
    pub fn node(&self, id: NodeId) -> Option<&GraphNode> {
        self.nodes.get(&id)
    }

    /// Adds a directed edge from `from` to `to`. Returns the edge id.
    ///
    /// Panics if either endpoint node does not exist.
    #[allow(clippy::too_many_arguments)] // edge geometry + tags; a params struct would obscure every call site
    pub fn add_edge(
        &mut self,
        from: NodeId,
        to: NodeId,
        distance_km: f64,
        duration_minutes: i64,
        highway: Option<String>,
        maxweight_tonnes: Option<f64>,
        maxheight_m: Option<f64>,
        hgv: Option<String>,
        access: Option<String>,
        oneway: bool,
    ) -> EdgeId {
        self.add_edge_with_way(
            from,
            to,
            distance_km,
            duration_minutes,
            highway,
            maxweight_tonnes,
            maxheight_m,
            hgv,
            access,
            oneway,
            None,
        )
    }

    /// Like [`Graph::add_edge`] but records the OSM way id the edge came
    /// from, so curated constraints can target the way.
    #[allow(clippy::too_many_arguments)]
    pub fn add_edge_with_way(
        &mut self,
        from: NodeId,
        to: NodeId,
        distance_km: f64,
        duration_minutes: i64,
        highway: Option<String>,
        maxweight_tonnes: Option<f64>,
        maxheight_m: Option<f64>,
        hgv: Option<String>,
        access: Option<String>,
        oneway: bool,
        way_id: Option<i64>,
    ) -> EdgeId {
        assert!(
            self.nodes.contains_key(&from) && self.nodes.contains_key(&to),
            "graph edge endpoints must exist"
        );
        let id = EdgeId(self.next_edge_id);
        self.next_edge_id += 1;
        let edge = GraphEdge {
            id,
            from,
            to,
            distance_km,
            duration_minutes,
            highway,
            maxweight_tonnes,
            maxheight_m,
            hgv,
            access,
            oneway,
            way_id,
        };
        self.edges.insert(id, edge.clone());
        self.out_edges.entry(from).or_default().push((id, to));
        id
    }

    /// Returns the outgoing edges for a node as `(edge_id, target, edge_ref)`.
    pub fn out_edges(&self, id: NodeId) -> Vec<(EdgeId, NodeId, &GraphEdge)> {
        self.out_edges
            .get(&id)
            .map(|v| {
                v.iter()
                    .filter_map(|(eid, target)| {
                        self.edges.get(eid).map(|edge| (*eid, *target, edge))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns a reference to an edge by id.
    pub fn edge(&self, id: EdgeId) -> Option<&GraphEdge> {
        self.edges.get(&id)
    }

    /// Returns a mutable reference to an edge by id.
    pub fn edge_mut(&mut self, id: EdgeId) -> Option<&mut GraphEdge> {
        self.edges.get_mut(&id)
    }

    /// Iterates over all nodes in the graph.
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &GraphNode)> {
        self.nodes.iter().map(|(id, node)| (*id, node))
    }

    /// Iterates mutably over all edges in the graph.
    ///
    /// Used by the curated constraint layer to fold hand-verified limits
    /// into edges without changing edge identities.
    pub fn edges_mut(&mut self) -> impl Iterator<Item = &mut GraphEdge> {
        self.edges.values_mut()
    }
}

/// A uniform-grid spatial index over graph-node coordinates, for fast
/// nearest-node queries (used to snap generated points onto the road network).
#[derive(Clone, Debug)]
pub struct NodeIndex {
    /// Cell size in degrees.
    cell_deg: f64,
    /// Node coordinates bucketed by grid cell.
    cells: HashMap<(i32, i32), Vec<LatLon>>,
}

impl NodeIndex {
    /// Cell size in degrees (~5.5 km at Sydney's latitude).
    pub const CELL_DEG: f64 = 0.05;

    /// Build an index over every node in `graph`.
    pub fn build(graph: &Graph) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<LatLon>> = HashMap::new();
        for (_id, node) in graph.nodes() {
            let key = Self::key(node.location);
            cells.entry(key).or_default().push(node.location);
        }
        Self {
            cell_deg: Self::CELL_DEG,
            cells,
        }
    }

    fn key(point: LatLon) -> (i32, i32) {
        (
            (point.lat / Self::CELL_DEG).floor() as i32,
            (point.lon / Self::CELL_DEG).floor() as i32,
        )
    }

    /// The nearest node to `point`, if one is within `max_km`. Returns the
    /// node coordinate and its distance in kilometres. Searches outward ring
    /// by ring and stops once the best hit is closer than the next ring.
    pub fn nearest(&self, point: LatLon, max_km: f64) -> Option<(LatLon, f64)> {
        let (base_lat, base_lon) = Self::key(point);
        let km_per_cell = self.cell_deg * 111.0;
        let max_ring = (max_km / km_per_cell).ceil() as i32 + 1;
        let mut best: Option<(LatLon, f64)> = None;
        for ring in 0..=max_ring {
            self.scan_ring(point, base_lat, base_lon, ring, &mut best);
            // Once the best hit is closer than the next ring's inner edge, no
            // farther ring can improve it.
            if let Some((_, best_distance)) = best
                && best_distance <= ring as f64 * km_per_cell
            {
                break;
            }
        }
        best.filter(|(_, distance)| *distance <= max_km)
    }

    /// Scan the cells at Chebyshev distance `ring` from `(base_lat, base_lon)`,
    /// updating `best` with any closer node.
    fn scan_ring(
        &self,
        point: LatLon,
        base_lat: i32,
        base_lon: i32,
        ring: i32,
        best: &mut Option<(LatLon, f64)>,
    ) {
        for d_lat in -ring..=ring {
            for d_lon in -ring..=ring {
                // Only the perimeter of this ring; the interior was scanned by
                // an earlier ring.
                if ring > 0 && d_lat.abs() != ring && d_lon.abs() != ring {
                    continue;
                }
                let Some(bucket) = self.cells.get(&(base_lat + d_lat, base_lon + d_lon)) else {
                    continue;
                };
                for &location in bucket {
                    let distance = domain::haversine_km(point, location);
                    if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                        *best = Some((location, distance));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_with(nodes: &[(u64, f64, f64)]) -> Graph {
        let mut graph = Graph::new();
        for (id, lat, lon) in nodes {
            graph.add_node(NodeId(*id), LatLon::new(*lat, *lon));
        }
        graph
    }

    #[test]
    fn nearest_finds_the_closest_node() {
        let graph = graph_with(&[
            (1, -33.90, 151.10),
            (2, -33.90, 151.30),
            (3, -33.95, 151.20),
        ]);
        let index = NodeIndex::build(&graph);
        let (location, distance) = index
            .nearest(LatLon::new(-33.905, 151.305), 50.0)
            .expect("a node is in range");
        assert_eq!(location, LatLon::new(-33.90, 151.30));
        assert!(distance < 1.0, "expected the close node, got {distance} km");
    }

    #[test]
    fn nearest_respects_the_max_distance() {
        let graph = graph_with(&[(1, -33.90, 151.10)]);
        let index = NodeIndex::build(&graph);
        // A point ~10 km away: in range at 20 km, out of range at 5 km.
        assert!(index.nearest(LatLon::new(-33.99, 151.10), 20.0).is_some());
        assert!(index.nearest(LatLon::new(-33.99, 151.10), 5.0).is_none());
    }

    #[test]
    fn nearest_on_an_empty_index_is_none() {
        let index = NodeIndex::build(&Graph::new());
        assert!(index.nearest(LatLon::new(-33.87, 151.21), 50.0).is_none());
    }
}
