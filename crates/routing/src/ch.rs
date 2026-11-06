//! Contraction-hierarchy routing (M3 gap): prepares the road graph once and
//! answers point-to-point queries without a fresh Dijkstra per pair.
//!
//! Wraps the [`fast_paths`] crate. Distances are recovered by summing the
//! underlying edges along the returned node sequence; the hierarchy's weight
//! is travel time in seconds, so a CH path minimises travel time while
//! [`crate::router::Router`] minimises distance. On graphs where the two
//! disagree the strategies can return different (equally optimal for their
//! own objective) paths.
//!
//! Preparation cost is amortised by the `n²` queries of a matrix build; it
//! is opt-in (`solve --ch`) because `fast_paths` preparation for a very
//! large extract costs significant time and memory.

use std::collections::{BTreeSet, HashMap};

use thiserror::Error;

use crate::graph::{EdgeId, Graph, GraphEdge, NodeId};
use crate::router::RouteResult;
use crate::truck_constraints::{TruckProfile, edge_allowed_for};

/// Why a contraction hierarchy could not be built.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ChError {
    /// No edge in the graph is traversable by the given truck profile.
    #[error("the graph has no edges usable by this truck profile")]
    NoUsableEdges,
}

/// A prepared contraction hierarchy over one filtered view of a graph.
pub struct ContractionHierarchy {
    /// Dense `fast_paths` id -> graph node id.
    dense_nodes: Vec<NodeId>,
    /// Graph node id -> dense id.
    to_dense: HashMap<NodeId, usize>,
    /// Icon of the prepared hierarchy.
    fast_graph: fast_paths::FastGraph,
    /// Reusable query buffers; one calculator per hierarchy is sufficient
    /// for the single-threaded matrix build.
    calculator: fast_paths::PathCalculator,
    /// Best directed edge between an adjacent pair: `(distance_km, minutes,
    /// edge id)`. Used to recover distance/duration from a CH path and to
    /// name the exact edge each hop traverses.
    best_edges: HashMap<(NodeId, NodeId), (f64, i64, EdgeId)>,
}

impl std::fmt::Debug for ContractionHierarchy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContractionHierarchy")
            .field("nodes", &self.dense_nodes.len())
            .field("edges", &self.best_edges.len())
            .finish_non_exhaustive()
    }
}

impl ContractionHierarchy {
    /// Prepares a hierarchy from `graph`, keeping only edges `profile` may
    /// traverse.
    ///
    /// Edge weights are travel time in seconds (minimum 1). Deterministic:
    /// nodes and edges are added in ascending id order.
    pub fn build(graph: &Graph, profile: TruckProfile) -> Result<Self, ChError> {
        // Collect usable edges in a deterministic order (nodes ascending,
        // then edge id).
        let mut nodes: Vec<NodeId> = graph.nodes().map(|(id, _)| id).collect();
        nodes.sort_unstable();
        let mut usable_edges: Vec<(&GraphEdge, NodeId, NodeId)> = Vec::new();
        for node in nodes {
            let mut out = graph.out_edges(node);
            out.sort_by_key(|(edge_id, _, _)| edge_id.0);
            for (_, target, edge) in out {
                // Self-loops can never lie on a path, and `fast_paths` drops
                // them before inferring its node count — keeping one would
                // assign a dense id beyond that count and trip its asserts.
                if target != node && edge_allowed_for(edge, profile).is_ok() {
                    usable_edges.push((edge, node, target));
                }
            }
        }
        if usable_edges.is_empty() {
            return Err(ChError::NoUsableEdges);
        }

        // `fast_paths` infers its node count from the edges it receives, so
        // only nodes touched by a usable edge get a dense id. Isolated nodes
        // simply stay unknown and yield `None` from `shortest_path`.
        let mut touched = BTreeSet::new();
        for (_, from, to) in &usable_edges {
            touched.insert(*from);
            touched.insert(*to);
        }
        let dense_nodes: Vec<NodeId> = touched.into_iter().collect();
        let to_dense: HashMap<NodeId, usize> = dense_nodes
            .iter()
            .enumerate()
            .map(|(dense, &node)| (node, dense))
            .collect();

        let mut input = fast_paths::InputGraph::new();
        let mut best_edges: HashMap<(NodeId, NodeId), (f64, i64, EdgeId)> = HashMap::new();
        for (edge, from, to) in usable_edges {
            let seconds = edge.duration_minutes.saturating_mul(60).max(1) as usize;
            input.add_edge(to_dense[&from], to_dense[&to], seconds);
            let entry = best_edges.entry((from, to)).or_insert((
                edge.distance_km,
                edge.duration_minutes,
                edge.id,
            ));
            let better = edge.duration_minutes < entry.1
                || (edge.duration_minutes == entry.1
                    && (edge.distance_km, edge.id.0) < (entry.0, entry.2.0));
            if better {
                *entry = (edge.distance_km, edge.duration_minutes, edge.id);
            }
        }
        input.freeze();
        let fast_graph = fast_paths::prepare(&input);
        let calculator = fast_paths::create_calculator(&fast_graph);
        Ok(Self {
            dense_nodes,
            to_dense,
            fast_graph,
            calculator,
            best_edges,
        })
    }

    /// Number of nodes in the prepared hierarchy.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.dense_nodes.len()
    }

    /// Time-optimal shortest path, in the same shape [`Router`] returns.
    ///
    /// Returns `None` when either node is unknown to the hierarchy or no
    /// path exists.
    pub fn shortest_path(&mut self, source: NodeId, target: NodeId) -> Option<RouteResult> {
        // Same graph node (including two problem nodes snapped to the same
        // node): zero-length leg, even when that node has no usable edges.
        // This must precede the `to_dense` lookups so isolated or
        // profile-filtered nodes behave exactly like `Router`.
        if source == target {
            return Some(RouteResult {
                distance_km: 0.0,
                duration_minutes: 0,
                nodes: vec![source],
            });
        }
        let &source_dense = self.to_dense.get(&source)?;
        let &target_dense = self.to_dense.get(&target)?;
        let path = self
            .calculator
            .calc_path(&self.fast_graph, source_dense, target_dense)?;
        let nodes: Vec<NodeId> = path
            .get_nodes()
            .iter()
            .map(|&dense| self.dense_nodes[dense])
            .collect();
        let mut distance_km = 0.0;
        let mut duration_minutes = 0i64;
        for pair in nodes.windows(2) {
            let (distance, minutes, _) = *self.best_edges.get(&(pair[0], pair[1]))?;
            distance_km += distance;
            duration_minutes += minutes;
        }
        Some(RouteResult {
            distance_km,
            duration_minutes,
            nodes,
        })
    }

    /// The directed edge the hierarchy used between two adjacent nodes on a
    /// returned path: the fastest one, ties by lower distance then edge id.
    ///
    /// For parallel edges of equal travel time the hierarchy's internal tie
    /// break may differ, so the id is exact whenever durations differ.
    #[must_use]
    pub fn edge_between(&self, from: NodeId, to: NodeId) -> Option<EdgeId> {
        self.best_edges.get(&(from, to)).map(|&(_, _, id)| id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::Router;
    use domain::LatLon;

    /// Builds a directed graph from `(from, to, km, minutes, highway)` rows.
    fn graph_from(rows: &[(u64, u64, f64, i64, &str)]) -> Graph {
        let mut graph = Graph::new();
        let mut seen = std::collections::BTreeSet::new();
        for &(from, to, _, _, _) in rows {
            seen.insert(from);
            seen.insert(to);
        }
        for node in seen {
            graph.add_node(
                NodeId(node),
                LatLon::new(-33.87, 151.21 + node as f64 * 0.01),
            );
        }
        for &(from, to, km, minutes, highway) in rows {
            graph.add_edge(
                NodeId(from),
                NodeId(to),
                km,
                minutes,
                Some(highway.to_string()),
                None,
                None,
                None,
                None,
                false,
            );
        }
        graph
    }

    /// A line 0-1-2-3 in both directions, 10 km / 10 min per hop.
    fn line_graph() -> Graph {
        graph_from(&[
            (0, 1, 10.0, 10, "primary"),
            (1, 0, 10.0, 10, "primary"),
            (1, 2, 10.0, 10, "primary"),
            (2, 1, 10.0, 10, "primary"),
            (2, 3, 10.0, 10, "primary"),
            (3, 2, 10.0, 10, "primary"),
        ])
    }

    #[test]
    fn ch_matches_dijkstra_on_a_line() {
        let graph = line_graph();
        let mut ch = ContractionHierarchy::build(&graph, TruckProfile::default()).expect("build");
        for &from in &[NodeId(0), NodeId(1), NodeId(2), NodeId(3)] {
            for &to in &[NodeId(0), NodeId(1), NodeId(2), NodeId(3)] {
                let expected = Router::shortest_path(&graph, from, to, TruckProfile::default());
                let actual = ch.shortest_path(from, to);
                assert_eq!(actual, expected, "pair {from:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn ch_prefers_the_time_optimal_route_and_still_sums_distance() {
        // Two routes 0 -> 3: a short slow one and a long fast one.
        let graph = graph_from(&[
            (0, 1, 5.0, 30, "primary"),
            (1, 3, 5.0, 30, "primary"),
            (0, 2, 20.0, 5, "primary"),
            (2, 3, 20.0, 5, "primary"),
        ]);
        let mut ch = ContractionHierarchy::build(&graph, TruckProfile::default()).expect("build");
        let result = ch.shortest_path(NodeId(0), NodeId(3)).expect("path");
        assert_eq!(result.duration_minutes, 10, "time-optimal route");
        assert!((result.distance_km - 40.0).abs() < 1e-9);
        assert_eq!(result.nodes, vec![NodeId(0), NodeId(2), NodeId(3)]);
        // Dijkstra minimises distance and picks the 10 km route instead.
        let dijkstra = Router::shortest_path(&graph, NodeId(0), NodeId(3), TruckProfile::default())
            .expect("path");
        assert!((dijkstra.distance_km - 10.0).abs() < 1e-9);
    }

    #[test]
    fn ch_respects_one_ways_and_unreachable_pairs() {
        // 0 -> 1 one-way only; 2 is disconnected.
        let graph = graph_from(&[(0, 1, 10.0, 10, "primary")]);
        // Node 2 exists without edges.
        let mut graph = graph;
        graph.add_node(NodeId(2), LatLon::new(-33.9, 151.0));
        let mut ch = ContractionHierarchy::build(&graph, TruckProfile::default()).expect("build");
        assert!(ch.shortest_path(NodeId(0), NodeId(1)).is_some());
        assert_eq!(ch.shortest_path(NodeId(1), NodeId(0)), None);
        assert_eq!(ch.shortest_path(NodeId(0), NodeId(2)), None);
        assert_eq!(ch.shortest_path(NodeId(0), NodeId(99)), None);
        // A node with no usable edges still answers a zero-length query,
        // exactly like Router::shortest_path(source, source).
        assert_eq!(
            ch.shortest_path(NodeId(2), NodeId(2)),
            Some(RouteResult {
                distance_km: 0.0,
                duration_minutes: 0,
                nodes: vec![NodeId(2)],
            })
        );
    }

    #[test]
    fn ch_respects_truck_profiles() {
        let mut graph = line_graph();
        // Relabel the 2 -> 3 hop as hgv=no: the HGV profile cannot pass.
        let edge_ids: Vec<EdgeId> = graph
            .out_edges(NodeId(2))
            .into_iter()
            .map(|(edge_id, _, _)| edge_id)
            .collect();
        for edge_id in edge_ids {
            if let Some(edge) = graph.edge_mut(edge_id) {
                edge.hgv = Some("no".to_string());
            }
        }
        let hgv = TruckProfile {
            hgv: true,
            ..TruckProfile::default()
        };
        let mut ch = ContractionHierarchy::build(&graph, hgv).expect("build");
        assert_eq!(ch.shortest_path(NodeId(0), NodeId(3)), None);
        assert!(ch.shortest_path(NodeId(0), NodeId(2)).is_some());

        let mut car = ContractionHierarchy::build(&graph, TruckProfile::default()).expect("build");
        assert!(car.shortest_path(NodeId(0), NodeId(3)).is_some());
    }

    /// Timing measurement (not a correctness gate) on a grid graph: all-pairs
    /// Dijkstra versus CH preparation + all-pairs queries. Run with
    /// `cargo test -p routing --lib grid_measurement -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing measurement, not a correctness gate"]
    fn grid_measurement() {
        let width = 15usize;
        let graph = grid_graph(width);
        let profile = TruckProfile::default();
        let mut nodes: Vec<NodeId> = graph.nodes().map(|(id, _)| id).collect();
        nodes.sort_unstable();

        let (routed, dijkstra_elapsed) = time_all_pairs(&graph, &nodes, profile);
        let started = std::time::Instant::now();
        let mut ch = ContractionHierarchy::build(&graph, profile).expect("build");
        let prepare_elapsed = started.elapsed();
        let (ch_routed, query_elapsed) = time_ch_pairs(&mut ch, &nodes);

        assert_eq!(routed, ch_routed);
        eprintln!(
            "grid {width}x{width} ({pairs} ordered pairs): Dijkstra {dijkstra_elapsed:?}; \
             CH prepare {prepare_elapsed:?} + queries {query_elapsed:?}",
            pairs = nodes.len() * nodes.len(),
        );
    }

    /// A `width` x `width` grid of nodes with unit-cost bidirectional edges.
    fn grid_graph(width: usize) -> Graph {
        let mut graph = Graph::new();
        for y in 0..width {
            for x in 0..width {
                let node = (y * width + x) as u64;
                graph.add_node(
                    NodeId(node),
                    LatLon::new(-33.87 + y as f64 * 0.01, 151.21 + x as f64 * 0.01),
                );
            }
        }
        for y in 0..width {
            for x in 0..width {
                let node = (y * width + x) as u64;
                if x + 1 < width {
                    connect_both(&mut graph, node, node + 1);
                }
                if y + 1 < width {
                    connect_both(&mut graph, node, node + width as u64);
                }
            }
        }
        graph
    }

    /// Adds a unit-cost edge in both directions between two grid nodes.
    fn connect_both(graph: &mut Graph, from: u64, to: u64) {
        for (a, b) in [(from, to), (to, from)] {
            graph.add_edge(
                NodeId(a),
                NodeId(b),
                1.0,
                1,
                Some("primary".to_string()),
                None,
                None,
                None,
                None,
                false,
            );
        }
    }

    /// Times all-pairs Dijkstra and counts the reachable pairs.
    fn time_all_pairs(
        graph: &Graph,
        nodes: &[NodeId],
        profile: TruckProfile,
    ) -> (usize, std::time::Duration) {
        let started = std::time::Instant::now();
        let routed = nodes
            .iter()
            .flat_map(|&from| nodes.iter().map(move |&to| (from, to)))
            .filter(|&(from, to)| Router::shortest_path(graph, from, to, profile).is_some())
            .count();
        (routed, started.elapsed())
    }

    /// Times all-pairs CH queries and counts the reachable pairs.
    fn time_ch_pairs(
        ch: &mut ContractionHierarchy,
        nodes: &[NodeId],
    ) -> (usize, std::time::Duration) {
        let started = std::time::Instant::now();
        let routed = nodes
            .iter()
            .flat_map(|&from| nodes.iter().map(move |&to| (from, to)))
            .filter(|&(from, to)| ch.shortest_path(from, to).is_some())
            .count();
        (routed, started.elapsed())
    }

    #[test]
    fn an_all_forbidden_graph_reports_no_usable_edges() {
        let mut graph = line_graph();
        let edge_ids: Vec<EdgeId> = [NodeId(0), NodeId(1), NodeId(2), NodeId(3)]
            .into_iter()
            .flat_map(|node| graph.out_edges(node))
            .map(|(edge_id, _, _)| edge_id)
            .collect();
        for edge_id in edge_ids {
            if let Some(edge) = graph.edge_mut(edge_id) {
                edge.access = Some("no".to_string());
            }
        }
        assert_eq!(
            ContractionHierarchy::build(&graph, TruckProfile::default()).err(),
            Some(ChError::NoUsableEdges)
        );
    }

    #[test]
    fn self_loop_edges_do_not_break_queries() {
        // Node 2 has only a self-loop. `fast_paths` drops loops before
        // inferring its node count, so keeping one used to assign a dense id
        // beyond that count and panic inside `calc_path`.
        let mut graph = graph_from(&[(0, 1, 10.0, 10, "primary")]);
        graph.add_node(NodeId(2), LatLon::new(-33.9, 151.0));
        graph.add_edge(
            NodeId(2),
            NodeId(2),
            1.0,
            1,
            Some("primary".to_string()),
            None,
            None,
            None,
            None,
            false,
        );
        let mut ch = ContractionHierarchy::build(&graph, TruckProfile::default()).expect("build");
        assert_eq!(ch.shortest_path(NodeId(2), NodeId(1)), None);
        assert_eq!(ch.shortest_path(NodeId(0), NodeId(2)), None);
        assert!(ch.shortest_path(NodeId(0), NodeId(1)).is_some());
    }
}
