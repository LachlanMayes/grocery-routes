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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
