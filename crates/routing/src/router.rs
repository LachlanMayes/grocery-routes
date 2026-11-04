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

            let current_time = *total_time.get(&current).unwrap_or(&0);

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
