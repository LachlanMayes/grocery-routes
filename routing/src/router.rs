//! A* shortest-path router over a [`crate::graph::Graph`].
//!
//! Returns distance (km) and duration (minutes) between any two nodes, with an
//! optional truck profile to filter edges. The search is A*: the priority is
//! `g + h`, where `g` is the distance so far and `h` is the straight-line
//! (haversine) distance to the target. Haversine never exceeds the road
//! distance, so the heuristic is admissible and the returned path is the same
//! distance-optimal path plain Dijkstra would find - it just explores far fewer
//! nodes, which matters on the full NSW extract (millions of nodes).

use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::graph::{EdgeId, Graph, NodeId};
use crate::truck_constraints::{TruckProfile, edge_allowed_for};

/// Result of a shortest-path query.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteResult {
    /// Total distance in kilometres.
    pub distance_km: f64,
    /// Total travel time in whole minutes.
    pub duration_minutes: i64,
    /// Ordered sequence of node ids from source to target.
    pub nodes: Vec<NodeId>,
}

/// Internal priority-queue state: f-score (g + h) + node id.
#[derive(Clone, Copy, Debug, PartialEq)]
struct State {
    cost: f64,
    node: NodeId,
}

impl Ord for State {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // BinaryHeap is a max-heap; invert so smallest cost pops first.
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl PartialOrd for State {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Eq is required for BinaryHeap; distance-based ordering is sufficient.
impl Eq for State {}

/// A* router over a graph.
#[derive(Clone, Debug, Default)]
pub struct Router;

impl Router {
    /// Computes the shortest path from `source` to `target` using A*,
    /// respecting `profile` truck constraints.
    ///
    /// Returns `None` when no path exists.
    pub fn shortest_path(
        graph: &Graph,
        source: NodeId,
        target: NodeId,
        profile: TruckProfile,
    ) -> Option<RouteResult> {
        Self::shortest_path_with_edges(graph, source, target, profile).map(|(route, _)| route)
    }

    /// [`Router::shortest_path`] with the traversed edge sequence, one edge
    /// per consecutive node pair.
    ///
    /// Corridor audits replay this series so they describe the exact edges
    /// the distance was priced on, even where parallel edges exist.
    pub fn shortest_path_with_edges(
        graph: &Graph,
        source: NodeId,
        target: NodeId,
        profile: TruckProfile,
    ) -> Option<(RouteResult, Vec<EdgeId>)> {
        if source == target {
            return Some((
                RouteResult {
                    distance_km: 0.0,
                    duration_minutes: 0,
                    nodes: vec![source],
                },
                Vec::new(),
            ));
        }
        let target_location = graph.node(target)?.location;

        // g-score: best distance from source found so far.
        let mut g: HashMap<NodeId, f64> = HashMap::new();
        let mut total_secs: HashMap<NodeId, f64> = HashMap::new();
        let mut prev: HashMap<NodeId, (NodeId, u64)> = HashMap::new(); // node -> (predecessor, edge_id)
        let mut settled: HashSet<NodeId> = HashSet::new();

        g.insert(source, 0.0);
        total_secs.insert(source, 0.0);

        let mut heap: BinaryHeap<State> = BinaryHeap::new();
        heap.push(State {
            cost: heuristic(graph, source, target_location),
            node: source,
        });

        while let Some(State {
            cost: _f_score,
            node: current,
        }) = heap.pop()
        {
            // The first time a node is popped its g-score is optimal (the
            // heuristic is consistent); later stale pops are skipped.
            if !settled.insert(current) {
                continue;
            }
            if current == target {
                break;
            }

            let current_dist = g[&current];
            let current_secs = *total_secs
                .get(&current)
                .expect("popped node must have a time entry");

            for (edge_id, next_node, edge) in graph.out_edges(current) {
                if settled.contains(&next_node) || edge_allowed_for(edge, profile).is_err() {
                    continue;
                }

                let new_dist = current_dist + edge.distance_km;
                let known_dist = g.get(&next_node).copied().unwrap_or(f64::INFINITY);
                let new_secs = current_secs + edge.duration_seconds;
                let known_secs = total_secs.get(&next_node).copied().unwrap_or(f64::INFINITY);

                if new_dist < known_dist || (new_dist == known_dist && new_secs < known_secs) {
                    g.insert(next_node, new_dist);
                    total_secs.insert(next_node, new_secs);
                    prev.insert(next_node, (current, edge_id.0));
                    heap.push(State {
                        cost: new_dist + heuristic(graph, next_node, target_location),
                        node: next_node,
                    });
                }
            }
        }

        let final_dist = *g.get(&target)?;
        let final_time = (*total_secs.get(&target)? / 60.0).round() as i64;
        reconstruct(source, target, final_dist, final_time, &prev)
    }

    /// Computes distance-optimal `(distance_km, duration_minutes)` from `source`
    /// to every node in `targets` in a single multi-target Dijkstra sweep,
    /// stopping as soon as all distinct targets are settled.
    pub fn shortest_paths_from(
        graph: &Graph,
        source: NodeId,
        targets: &[NodeId],
        profile: TruckProfile,
    ) -> Vec<Option<(f64, i64)>> {
        if targets.is_empty() {
            return Vec::new();
        }
        let mut remaining: HashSet<NodeId> =
            targets.iter().copied().filter(|&t| t != source).collect();
        if remaining.is_empty() || graph.node(source).is_none() {
            return targets
                .iter()
                .map(|&t| (t == source).then_some((0.0, 0)))
                .collect();
        }

        let mut g: HashMap<NodeId, f64> = HashMap::new();
        let mut total_secs: HashMap<NodeId, f64> = HashMap::new();
        let mut settled: HashSet<NodeId> = HashSet::new();
        let mut heap: BinaryHeap<State> = BinaryHeap::new();

        g.insert(source, 0.0);
        total_secs.insert(source, 0.0);
        heap.push(State {
            cost: 0.0,
            node: source,
        });

        while let Some(State {
            cost: _,
            node: current,
        }) = heap.pop()
        {
            if !settled.insert(current) {
                continue;
            }
            if remaining.remove(&current) && remaining.is_empty() {
                break;
            }

            let current_dist = g[&current];
            let current_secs = total_secs[&current];
            for (_edge_id, next_node, edge) in graph.out_edges(current) {
                if settled.contains(&next_node) || edge_allowed_for(edge, profile).is_err() {
                    continue;
                }
                let new_dist = current_dist + edge.distance_km;
                let known_dist = g.get(&next_node).copied().unwrap_or(f64::INFINITY);
                let new_secs = current_secs + edge.duration_seconds;
                let known_secs = total_secs.get(&next_node).copied().unwrap_or(f64::INFINITY);
                if new_dist < known_dist || (new_dist == known_dist && new_secs < known_secs) {
                    g.insert(next_node, new_dist);
                    total_secs.insert(next_node, new_secs);
                    heap.push(State {
                        cost: new_dist,
                        node: next_node,
                    });
                }
            }
        }

        targets
            .iter()
            .map(|&t| {
                if t == source {
                    Some((0.0, 0))
                } else {
                    g.get(&t)
                        .and_then(|&d| total_secs.get(&t).map(|&s| (d, (s / 60.0).round() as i64)))
                }
            })
            .collect()
    }

    /// Computes distance and duration (without path reconstruction) from
    /// `source` to `target`, respecting `profile` truck constraints.
    ///
    /// Returns `None` when no path exists.
    pub fn distance_duration(
        graph: &Graph,
        source: NodeId,
        target: NodeId,
        profile: TruckProfile,
    ) -> Option<(f64, i64)> {
        let result = Self::shortest_path(graph, source, target, profile)?;
        Some((result.distance_km, result.duration_minutes))
    }

    /// Ordered coordinates of the shortest road path from `source` to `target`
    /// (one point per graph node along the way). This is the exact road
    /// geometry the distance was priced on - use it to draw a route that
    /// follows real streets instead of a straight line between stops.
    pub fn path_geometry(
        graph: &Graph,
        source: NodeId,
        target: NodeId,
        profile: TruckProfile,
    ) -> Option<Vec<domain::LatLon>> {
        Self::shortest_path(graph, source, target, profile).map(|route| {
            route
                .nodes
                .iter()
                .filter_map(|id| graph.node(*id).map(|node| node.location))
                .collect()
        })
    }
}

/// Straight-line distance from `node` to the target, in km. Never exceeds the
/// road distance, so it is an admissible (and consistent) A* heuristic.
fn heuristic(graph: &Graph, node: NodeId, target: domain::LatLon) -> f64 {
    graph
        .node(node)
        .map_or(0.0, |n| domain::haversine_km(n.location, target))
}

/// Rebuilds the node and edge sequences from the predecessor map, target back
/// to source; `None` when the map is missing a link.
fn reconstruct(
    source: NodeId,
    target: NodeId,
    final_dist: f64,
    final_time: i64,
    prev: &HashMap<NodeId, (NodeId, u64)>,
) -> Option<(RouteResult, Vec<EdgeId>)> {
    let mut path = Vec::new();
    let mut edges = Vec::new();
    let mut current = target;
    loop {
        path.push(current);
        if current == source {
            break;
        }
        let (pred, edge_id) = prev.get(&current)?;
        edges.push(EdgeId(*edge_id));
        current = *pred;
    }
    path.reverse();
    edges.reverse();
    Some((
        RouteResult {
            distance_km: final_dist,
            duration_minutes: final_time,
            nodes: path,
        },
        edges,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, NodeId};
    use crate::truck_constraints::TruckProfile;
    use domain::LatLon;

    fn build_graph() -> Graph {
        let mut g = Graph::new();
        // Nodes 0..4 at known positions (roughly 10 km apart in a line)
        let positions = [
            LatLon::new(-33.87, 151.21), // 0
            LatLon::new(-33.87, 151.30), // 1 ~10km east of 0
            LatLon::new(-33.87, 151.39), // 2 ~10km east of 1
            LatLon::new(-33.87, 151.48), // 3 ~10km east of 2
            LatLon::new(-33.87, 151.57), // 4 ~10km east of 3
        ];
        for (i, &pos) in positions.iter().enumerate() {
            g.add_node(NodeId(i as u64), pos);
        }
        // Chain: 0-1-2-3-4, each segment ~10 km, 10 min at 60 km/h
        for i in 0..4 {
            g.add_edge(
                NodeId(i as u64),
                NodeId((i + 1) as u64),
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
    fn shortest_path_direct_edge() {
        let g = build_graph();
        let r = Router::shortest_path(&g, NodeId(0), NodeId(1), TruckProfile::default())
            .expect("direct edge should exist");
        assert!((r.distance_km - 10.0).abs() < 1e-9);
        assert_eq!(r.duration_minutes, 10);
        assert_eq!(r.nodes, vec![NodeId(0), NodeId(1)]);
    }

    #[test]
    fn shortest_path_multi_hop() {
        let g = build_graph();
        let r = Router::shortest_path(&g, NodeId(0), NodeId(4), TruckProfile::default())
            .expect("path 0->4 should exist");
        assert!((r.distance_km - 40.0).abs() < 1e-9);
        assert_eq!(r.duration_minutes, 40);
        assert_eq!(
            r.nodes,
            vec![NodeId(0), NodeId(1), NodeId(2), NodeId(3), NodeId(4)]
        );
    }

    #[test]
    fn shortest_path_same_source_and_target() {
        let g = build_graph();
        let r = Router::shortest_path(&g, NodeId(2), NodeId(2), TruckProfile::default())
            .expect("source==target should return zero result");
        assert_eq!(r.distance_km, 0.0);
        assert_eq!(r.duration_minutes, 0);
        assert_eq!(r.nodes, vec![NodeId(2)]);
    }

    #[test]
    fn shortest_path_unreachable_target() {
        let mut g = build_graph();
        // Add a disconnected node 5
        g.add_node(NodeId(5), LatLon::new(-33.87, 152.00));
        let r = Router::shortest_path(&g, NodeId(0), NodeId(5), TruckProfile::default());
        assert_eq!(r, None, "disconnected node should be unreachable");
    }

    #[test]
    fn shortest_path_truck_profile_blocks_edges() {
        // (can't remove edges easily, so build a separate graph where the
        // only path goes through an hgv=no road)
        let mut g2 = Graph::new();
        let positions = [
            LatLon::new(-33.87, 151.21), // 0
            LatLon::new(-33.87, 151.30), // 1
            LatLon::new(-33.87, 151.39), // 2
        ];
        for (i, &pos) in positions.iter().enumerate() {
            g2.add_node(NodeId(i as u64), pos);
        }
        g2.add_edge(
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
        // 1->2 is tagged hgv=no (oneway forward)
        g2.add_edge(
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

        let hgv = TruckProfile {
            hgv: true,
            ..Default::default()
        };
        let r = Router::shortest_path(&g2, NodeId(0), NodeId(2), hgv);
        assert_eq!(
            r, None,
            "HGV should not traverse hgv=no road, no alternate path"
        );

        let car = TruckProfile::default();
        let r = Router::shortest_path(&g2, NodeId(0), NodeId(2), car)
            .expect("car should traverse hgv=no road");
        assert!((r.distance_km - 20.0).abs() < 1e-9);
    }

    #[test]
    fn distance_duration_returns_tuple() {
        let g = build_graph();
        let (dist, dur) =
            Router::distance_duration(&g, NodeId(0), NodeId(3), TruckProfile::default())
                .expect("path 0->3 should exist");
        assert!((dist - 30.0).abs() < 1e-9);
        assert_eq!(dur, 30);
    }

    #[test]
    fn subminute_segments_accumulate_seconds_and_match_multitarget() {
        let mut g = Graph::new();
        for i in 0..=5u64 {
            g.add_node(NodeId(i), LatLon::new(-33.87, 151.21 + i as f64 * 0.002));
        }
        // Five short 0.2 km edges of 24 seconds each -> total 120s = 2 minutes,
        // even though each individual edge rounds to 0 minutes.
        for i in 0..5u64 {
            g.add_edge_with_way_and_duration_secs(
                NodeId(i),
                NodeId(i + 1),
                0.2,
                0,
                24.0,
                Some("residential".to_string()),
                None,
                None,
                None,
                None,
                false,
                None,
            );
        }

        let single = Router::shortest_path(&g, NodeId(0), NodeId(5), TruckProfile::default())
            .expect("path 0->5 exists");
        assert_eq!(single.duration_minutes, 2);

        let multi = Router::shortest_paths_from(
            &g,
            NodeId(0),
            &[NodeId(0), NodeId(3), NodeId(5), NodeId(99)],
            TruckProfile::default(),
        );
        assert_eq!(multi[0], Some((0.0, 0)));
        assert_eq!(multi[1].map(|(_, d)| d), Some(1)); // 72s -> 1 min
        assert_eq!(multi[2].map(|(_, d)| d), Some(2)); // 120s -> 2 min
        assert_eq!(multi[3], None);
    }
}
