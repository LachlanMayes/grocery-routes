//! Dijkstra shortest-path router over a [`crate::graph::Graph`].
//!
//! Returns distance (km) and duration (minutes) between any two nodes, with an
//! optional truck profile to filter edges.

use std::collections::{BinaryHeap, HashMap};

use crate::graph::{Graph, NodeId};
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

/// Internal priority-queue state: distance so far + node id.
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

/// Dijkstra router over a graph.
#[derive(Clone, Debug, Default)]
pub struct Router;

impl Router {
    /// Computes the shortest path from `source` to `target` using Dijkstra,
    /// respecting `profile` truck constraints.
    ///
    /// Returns `None` when no path exists.
    pub fn shortest_path(
        graph: &Graph,
        source: NodeId,
        target: NodeId,
        profile: TruckProfile,
    ) -> Option<RouteResult> {
        if source == target {
            return Some(RouteResult {
                distance_km: 0.0,
                duration_minutes: 0,
                nodes: vec![source],
            });
        }

        let mut dist: HashMap<NodeId, f64> = HashMap::new();
        let mut total_time: HashMap<NodeId, i64> = HashMap::new();
        let mut prev: HashMap<NodeId, (NodeId, u64)> = HashMap::new(); // node -> (predecessor, edge_id)
        let mut visited: HashMap<NodeId, bool> = HashMap::new();

        dist.insert(source, 0.0);
        total_time.insert(source, 0);

        let mut heap: BinaryHeap<State> = BinaryHeap::new();
        heap.push(State {
            cost: 0.0,
            node: source,
        });

        while let Some(State {
            cost: current_dist,
            node: current,
        }) = heap.pop()
        {
            if visited.get(&current).copied().unwrap_or(false) {
                continue;
            }
            visited.insert(current, true);

            if current == target {
                break;
            }

            let current_time = *total_time
                .get(&current)
                .expect("popped node must have a time entry");

            for (edge_id, next_node, edge) in graph.out_edges(current) {
                if visited.get(&next_node).copied().unwrap_or(false) {
                    continue;
                }
                if edge_allowed_for(edge, profile).is_err() {
                    continue;
                }

                let new_dist = current_dist + edge.distance_km;
                let known_dist = dist.get(&next_node).copied().unwrap_or(f64::INFINITY);

                if new_dist < known_dist {
                    dist.insert(next_node, new_dist);
                    total_time.insert(next_node, current_time + edge.duration_minutes);
                    prev.insert(next_node, (current, edge_id.0));
                    heap.push(State {
                        cost: new_dist,
                        node: next_node,
                    });
                }
            }
        }

        let final_dist = *dist.get(&target)?;
        let final_time = *total_time.get(&target)?;

        // Reconstruct path from target back to source.
        let mut path = Vec::new();
        let mut current = target;
        loop {
            path.push(current);
            if current == source {
                break;
            }
            let (pred, _) = prev.get(&current)?;
            current = *pred;
        }
        path.reverse();

        Some(RouteResult {
            distance_km: final_dist,
            duration_minutes: final_time,
            nodes: path,
        })
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
                false,
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
            false,
            false,
        );
        // 1->2 is tagged hgv=no (oneway forward)
        g2.add_edge(
            NodeId(1),
            NodeId(2),
            10.0,
            10,
            Some("hgv=no".to_string()),
            None,
            None,
            false,
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
}
