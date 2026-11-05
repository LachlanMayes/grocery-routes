//! Corridor audit: which OSM ways does a solved plan traverse, and what
//! limits do they carry?
//!
//! The audit never claims legality. It lists every way a plan uses, the
//! truck-relevant limit tags found on it (`maxweight`, `maxheight`, `hgv`,
//! `access`) and explicitly flags ways with **no limit data**, because those
//! are the corridors a human has to verify against the authoritative source
//! (see `docs/TRUCK-CONSTRAINTS.md`).
//!
//! Legs follow [`crate::timed::plain_leg`] — the same distance-optimal path
//! the static matrix was built from — so the report describes the roads the
//! planner actually priced.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use domain::{Node, Problem, Solution};

use crate::graph::{Graph, NodeId};
use crate::truck_constraints::TruckProfile;

/// One distinct way used by one route.
#[derive(Clone, Debug, PartialEq)]
pub struct CorridorRow {
    /// Route index in the solution.
    pub route: usize,
    /// OSM way id; `None` for synthetic edges (tests/tools without OSM).
    pub way_id: Option<i64>,
    pub highway: Option<String>,
    pub maxweight_tonnes: Option<f64>,
    pub maxheight_m: Option<f64>,
    pub hgv: Option<String>,
    pub access: Option<String>,
    /// Times the route traverses this way.
    pub traversals: usize,
    /// Total length traversed on this way, kilometres.
    pub length_km: f64,
    /// Any truck-relevant limit tag is present.
    pub has_limit_data: bool,
}

/// The whole audit of one plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CorridorAudit {
    pub rows: Vec<CorridorRow>,
    /// Distinct ways across all routes.
    pub distinct_ways: usize,
    /// Distinct ways with no truck-relevant limit tag.
    pub ways_without_limits: usize,
    /// Legs with no road-network path (excluded from the rows).
    pub legs_without_path: usize,
}

impl CorridorAudit {
    /// Renders the audit as CSV.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from(
            "route,way_id,highway,traversals,length_km,maxweight_t,maxheight_m,hgv,access,has_limit_data\n",
        );
        for row in &self.rows {
            let way = row
                .way_id
                .map_or_else(|| "synthetic".to_string(), |id| id.to_string());
            let _ = writeln!(
                out,
                "{},{},{},{},{:.3},{},{},{},{},{}",
                row.route,
                way,
                csv_field(row.highway.as_deref().unwrap_or("")),
                row.traversals,
                row.length_km,
                row.maxweight_tonnes
                    .map_or_else(String::new, |value| value.to_string()),
                row.maxheight_m
                    .map_or_else(String::new, |value| value.to_string()),
                csv_field(row.hgv.as_deref().unwrap_or("")),
                csv_field(row.access.as_deref().unwrap_or("")),
                if row.has_limit_data { "yes" } else { "no" },
            );
        }
        out
    }

    /// A one-line human summary.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} way(s) across {} route(s); {} way(s) carry truck-limit data, {} do not (legality unverified); {} leg(s) had no path",
            self.distinct_ways,
            self.rows
                .iter()
                .map(|row| row.route)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            self.distinct_ways.saturating_sub(self.ways_without_limits),
            self.ways_without_limits,
            self.legs_without_path,
        )
    }
}

/// Audits `solution` against `graph`.
///
/// `node_ids` maps problem nodes (depots first, then stores) to graph nodes,
/// as returned by [`crate::CachedMatrix::problem_node_ids`].
#[must_use]
pub fn audit_solution(
    problem: &Problem,
    solution: &Solution,
    graph: &Graph,
    node_ids: &[NodeId],
    profile: TruckProfile,
) -> CorridorAudit {
    let mut audit = CorridorAudit::default();
    let mut all_way_ids: std::collections::BTreeSet<Option<i64>> =
        std::collections::BTreeSet::new();

    for (route_index, route) in solution.routes.iter().enumerate() {
        // (way key) -> aggregate for this route.
        let mut per_route: BTreeMap<Option<i64>, CorridorRow> = BTreeMap::new();
        for pair in route.steps.windows(2) {
            let (from, to) = (pair[0].node, pair[1].node);
            let (Some(&from_graph), Some(&to_graph)) = (
                node_index(problem, from).and_then(|i| node_ids.get(i)),
                node_index(problem, to).and_then(|i| node_ids.get(i)),
            ) else {
                audit.legs_without_path += 1;
                continue;
            };
            let Some(leg) =
                crate::timed::plain_leg(graph, from_graph, to_graph, pair[0].depart, profile)
            else {
                audit.legs_without_path += 1;
                continue;
            };
            for step in &leg.steps {
                let Some(edge) = graph.edge(step.edge) else {
                    continue;
                };
                let has_limit_data = edge.maxweight_tonnes.is_some()
                    || edge.maxheight_m.is_some()
                    || edge.hgv.is_some()
                    || edge.access.is_some();
                let entry = per_route.entry(edge.way_id).or_insert_with(|| CorridorRow {
                    route: route_index,
                    way_id: edge.way_id,
                    highway: edge.highway.clone(),
                    maxweight_tonnes: edge.maxweight_tonnes,
                    maxheight_m: edge.maxheight_m,
                    hgv: edge.hgv.clone(),
                    access: edge.access.clone(),
                    traversals: 0,
                    length_km: 0.0,
                    has_limit_data,
                });
                entry.traversals += 1;
                entry.length_km += edge.distance_km;
            }
        }
        for row in per_route.into_values() {
            all_way_ids.insert(row.way_id);
            audit.rows.push(row);
        }
    }

    audit.distinct_ways = all_way_ids.len();
    let unlimited: std::collections::BTreeSet<Option<i64>> = audit
        .rows
        .iter()
        .filter(|row| !row.has_limit_data)
        .map(|row| row.way_id)
        .collect();
    audit.ways_without_limits = unlimited.len();
    audit
}

/// Index of a problem node in the depot-then-store ordering.
fn node_index(problem: &Problem, node: Node) -> Option<usize> {
    match node {
        Node::Depot(id) => {
            let index = id.0 as usize;
            (index < problem.depots.len()).then_some(index)
        }
        Node::Store(id) => {
            let store = id.0 as usize;
            (store < problem.stores.len()).then_some(problem.depots.len() + store)
        }
    }
}

/// Quotes a CSV field when it contains a comma or quote.
fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
        LatLon, Load, Minutes, PlannedStop, Problem, RoutePlan, Shift, Solution, Store, StoreId,
        TimeWindow, Vehicle, VehicleId,
    };

    fn fixture() -> (Problem, Solution, Graph, Vec<NodeId>) {
        let problem = Problem {
            name: "audit fixture".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "DC".into(),
                location: LatLon::new(-33.87, 151.21),
                timezone: None,
            }],
            stores: vec![
                Store {
                    id: StoreId(0),
                    chain: Chain::Coles,
                    name: "A".into(),
                    location: LatLon::new(-33.87, 151.30),
                    demand: Load::new(1, 1, 1),
                    service: Minutes::from_minutes(5),
                    receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
                    timezone: None,
                },
                Store {
                    id: StoreId(1),
                    chain: Chain::Coles,
                    name: "B".into(),
                    location: LatLon::new(-33.87, 151.39),
                    demand: Load::new(1, 1, 1),
                    service: Minutes::from_minutes(5),
                    receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
                    timezone: None,
                },
            ],
            vehicles: vec![Vehicle {
                id: VehicleId(0),
                name: "Rigid".into(),
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
            cost: CostModel::default(),
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
        };
        let solution = Solution {
            routes: vec![RoutePlan {
                vehicle: VehicleId(0),
                drivers: vec![DriverId(0)],
                steps: vec![
                    PlannedStop {
                        node: Node::Depot(DepotId(0)),
                        arrive: Minutes::ZERO,
                        depart: Minutes::ZERO,
                    },
                    PlannedStop {
                        node: Node::Store(StoreId(0)),
                        arrive: Minutes::from_minutes(10),
                        depart: Minutes::from_minutes(15),
                    },
                    PlannedStop {
                        node: Node::Store(StoreId(1)),
                        arrive: Minutes::from_minutes(25),
                        depart: Minutes::from_minutes(30),
                    },
                    PlannedStop {
                        node: Node::Depot(DepotId(0)),
                        arrive: Minutes::from_minutes(40),
                        depart: Minutes::from_minutes(40),
                    },
                ],
                distance_km: 20.0,
                duration: Minutes::from_minutes(40),
                cost_cents: 0,
            }],
            unassigned: Vec::new(),
            total_distance_km: 20.0,
            total_duration: Minutes::from_minutes(40),
            total_cost_cents: 0,
        };

        // Depot 0 -> A 1 -> B 2 on two ways: way 10 (primary, maxweight 20)
        // for the first hop; way 11 (residential, untagged) for the second.
        let mut graph = Graph::new();
        for (node, lon) in [(0u64, 151.21), (1, 151.30), (2, 151.39)] {
            graph.add_node(NodeId(node), LatLon::new(-33.87, lon));
        }
        graph.add_edge_with_way(
            NodeId(0),
            NodeId(1),
            9.0,
            9,
            Some("primary".to_string()),
            Some(20.0),
            None,
            None,
            None,
            false,
            Some(10),
        );
        graph.add_edge_with_way(
            NodeId(1),
            NodeId(0),
            9.0,
            9,
            Some("primary".to_string()),
            Some(20.0),
            None,
            None,
            None,
            false,
            Some(10),
        );
        graph.add_edge_with_way(
            NodeId(1),
            NodeId(2),
            9.0,
            9,
            Some("residential".to_string()),
            None,
            None,
            None,
            None,
            false,
            Some(11),
        );
        graph.add_edge_with_way(
            NodeId(2),
            NodeId(1),
            9.0,
            9,
            Some("residential".to_string()),
            None,
            None,
            None,
            None,
            false,
            Some(11),
        );
        graph.add_edge_with_way(
            NodeId(2),
            NodeId(0),
            18.0,
            18,
            Some("motorway".to_string()),
            None,
            Some(4.6),
            None,
            None,
            false,
            Some(12),
        );
        let node_ids = vec![NodeId(0), NodeId(1), NodeId(2)];
        (problem, solution, graph, node_ids)
    }

    #[test]
    fn audit_lists_ways_with_limits_and_flags_untagged_ones() {
        let (problem, solution, graph, node_ids) = fixture();
        let audit = audit_solution(
            &problem,
            &solution,
            &graph,
            &node_ids,
            TruckProfile::default(),
        );

        assert_eq!(audit.legs_without_path, 0);
        // Route legs: 0->1 (way 10), 1->2 (way 11), 2->0 (way 12).
        assert_eq!(audit.distinct_ways, 3);
        let way10 = audit
            .rows
            .iter()
            .find(|row| row.way_id == Some(10))
            .expect("way 10 used");
        assert_eq!(way10.maxweight_tonnes, Some(20.0));
        assert!(way10.has_limit_data);
        let way11 = audit
            .rows
            .iter()
            .find(|row| row.way_id == Some(11))
            .expect("way 11 used");
        assert!(!way11.has_limit_data, "untagged residential way");
        assert_eq!(way11.highway.as_deref(), Some("residential"));
        let way12 = audit
            .rows
            .iter()
            .find(|row| row.way_id == Some(12))
            .expect("way 12 used");
        assert_eq!(way12.maxheight_m, Some(4.6));
        assert!(way12.has_limit_data);

        assert_eq!(audit.ways_without_limits, 1);
        let summary = audit.summary();
        assert!(summary.contains("3 way(s)"), "{summary}");
        assert!(summary.contains("1 do not"), "{summary}");
    }

    #[test]
    fn audit_csv_has_a_row_per_way_per_route() {
        let (problem, solution, graph, node_ids) = fixture();
        let audit = audit_solution(
            &problem,
            &solution,
            &graph,
            &node_ids,
            TruckProfile::default(),
        );
        let csv = audit.to_csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 1 + audit.rows.len());
        assert!(lines[0].starts_with("route,way_id,highway,"));
        assert!(csv.contains(",10,primary,"));
        assert!(csv.contains(",no\n") || csv.contains(",no"));
        assert!(csv.contains(",yes"));
    }

    #[test]
    fn audit_counts_legs_with_no_path() {
        let (mut problem, solution, graph, _) = fixture();
        // Map both stores onto disconnected synthetic nodes far away: no
        // graph node near them and no path.
        problem.stores.clear();
        let audit = audit_solution(
            &problem,
            &solution,
            &graph,
            &[NodeId(0), NodeId(1)],
            TruckProfile::default(),
        );
        // Store ids reference stores that no longer exist, so the store legs
        // cannot be resolved and are counted as missing paths.
        assert!(audit.legs_without_path >= 2, "{}", audit.legs_without_path);
    }
}
