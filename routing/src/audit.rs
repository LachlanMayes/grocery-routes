//! Corridor audit: which OSM ways does a solved plan traverse, and what
//! limits do they carry?
//!
//! The audit never claims legality. It lists every way a plan uses, the
//! truck-relevant limit tags found on it (`maxweight`, `maxheight`, `hgv`,
//! `access`) and explicitly flags ways with **no limit data**, because those
//! are the corridors a human has to verify against the authoritative source
//! (see `docs/TRUCK-CONSTRAINTS.md`).
//!
//! Legs are replayed with the same matrix objective the plan was solved
//! with ([`AuditStrategy`]): the distance-optimal Dijkstra path for a plain
//! solve, the time-optimal contraction-hierarchy path for a `solve --ch`
//! plan — so the report describes the roads the planner actually priced.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use domain::{Minutes, Node, Problem, Solution};

use crate::graph::{EdgeId, Graph, NodeId};
use crate::truck_constraints::TruckProfile;

/// How each leg is replayed when auditing a plan.
///
/// The planner's matrix strategy decides which path was priced: Dijkstra
/// matrices price the distance-optimal path, contraction hierarchies the
/// time-optimal one. The audit must replay the same objective or it reports
/// corridors the plan never uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuditStrategy {
    /// Distance-optimal Dijkstra paths; matches a plan solved without `--ch`.
    #[default]
    Dijkstra,
    /// Time-optimal contraction-hierarchy paths; matches `solve --ch`.
    ContractionHierarchy,
}

/// Aggregation key: a real way, or one synthetic edge (which has no way id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum WayKey {
    Way(i64),
    Synthetic(EdgeId),
}

/// One distinct way used by one route.
#[derive(Clone, Debug, PartialEq)]
pub struct CorridorRow {
    /// Route index in the solution.
    pub route: usize,
    /// OSM way id; `None` for synthetic edges (tests/tools without OSM),
    /// which are keyed per edge and identified by `edge_id`.
    pub way_id: Option<i64>,
    /// The first edge of this way traversed (always known; distinguishes
    /// synthetic edges from each other).
    pub edge_id: EdgeId,
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
            let way = row.way_id.map_or_else(
                || format!("synthetic:{}", row.edge_id.0),
                |id| id.to_string(),
            );
            writeln!(
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
            )
            .expect("writing to an in-memory String cannot fail");
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

/// Audits `solution` against `graph`, replaying each leg with `strategy`.
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
    strategy: AuditStrategy,
) -> CorridorAudit {
    let mut audit = CorridorAudit::default();
    let mut all_way_ids: std::collections::BTreeSet<WayKey> = std::collections::BTreeSet::new();
    // Prepared once when the plan was solved through a hierarchy.
    let mut hierarchy = match strategy {
        AuditStrategy::Dijkstra => None,
        AuditStrategy::ContractionHierarchy => {
            crate::ch::ContractionHierarchy::build(graph, profile).ok()
        }
    };

    for (route_index, route) in solution.routes.iter().enumerate() {
        // (way key) -> aggregate for this route. Synthetic edges (no way id)
        // are keyed individually so distinct corridors never merge.
        let mut per_route: BTreeMap<WayKey, CorridorRow> = BTreeMap::new();
        for pair in route.steps.windows(2) {
            let (from, to) = (pair[0].node, pair[1].node);
            let (Some(&from_graph), Some(&to_graph)) = (
                node_index(problem, from).and_then(|i| node_ids.get(i)),
                node_index(problem, to).and_then(|i| node_ids.get(i)),
            ) else {
                audit.legs_without_path += 1;
                continue;
            };
            let leg = match strategy {
                AuditStrategy::Dijkstra => {
                    crate::timed::plain_leg(graph, from_graph, to_graph, pair[0].depart, profile)
                }
                AuditStrategy::ContractionHierarchy => hierarchy.as_mut().and_then(|hierarchy| {
                    hierarchy_leg(hierarchy, graph, from_graph, to_graph, pair[0].depart)
                }),
            };
            let Some(leg) = leg else {
                audit.legs_without_path += 1;
                continue;
            };
            accumulate_leg(&mut per_route, graph, route_index, &leg);
        }
        for (key, row) in per_route {
            all_way_ids.insert(key);
            audit.rows.push(row);
        }
    }

    audit.distinct_ways = all_way_ids.len();
    let unlimited: std::collections::BTreeSet<WayKey> = audit
        .rows
        .iter()
        .filter(|row| !row.has_limit_data)
        .map(|row| match row.way_id {
            Some(way) => WayKey::Way(way),
            None => WayKey::Synthetic(row.edge_id),
        })
        .collect();
    audit.ways_without_limits = unlimited.len();
    audit
}

/// A leg along the hierarchy's time-optimal path: the node sequence comes
/// from the query, the exact per-hop edges from the prepared hierarchy.
fn hierarchy_leg(
    hierarchy: &mut crate::ch::ContractionHierarchy,
    graph: &Graph,
    from: NodeId,
    to: NodeId,
    depart: Minutes,
) -> Option<crate::timed::TimedLeg> {
    let route = hierarchy.shortest_path(from, to)?;
    let edges: Option<Vec<EdgeId>> = route
        .nodes
        .windows(2)
        .map(|pair| hierarchy.edge_between(pair[0], pair[1]))
        .collect();
    crate::timed::leg_from_edges(graph, &route.nodes, &edges?, depart)
}

/// Aggregates one computed leg into a route's way map, keyed per way (or per
/// synthetic edge when the graph carries no OSM way ids).
fn accumulate_leg(
    per_route: &mut BTreeMap<WayKey, CorridorRow>,
    graph: &Graph,
    route_index: usize,
    leg: &crate::timed::TimedLeg,
) {
    for step in &leg.steps {
        let Some(edge) = graph.edge(step.edge) else {
            continue;
        };
        let has_limit_data = edge.maxweight_tonnes.is_some()
            || edge.maxheight_m.is_some()
            || edge.hgv.is_some()
            || edge.access.is_some();
        let key = match edge.way_id {
            Some(way) => WayKey::Way(way),
            None => WayKey::Synthetic(step.edge),
        };
        let entry = per_route.entry(key).or_insert_with(|| CorridorRow {
            route: route_index,
            way_id: edge.way_id,
            edge_id: step.edge,
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

/// Quotes a CSV field when it contains a comma, quote or line break, and
/// neutralises spreadsheet formula prefixes (`=`, `+`, `-`, `@`).
///
/// Shared with the CLI route-sheet export (`cli::export`) so both CSVs
/// escape identically.
pub fn csv_field(value: &str) -> String {
    let prefixed = if matches!(value.chars().next(), Some('=' | '+' | '-' | '@')) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    if prefixed.contains(',')
        || prefixed.contains('"')
        || prefixed.contains('\n')
        || prefixed.contains('\r')
    {
        format!("\"{}\"", prefixed.replace('"', "\"\""))
    } else {
        prefixed
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
                address: None,
                timezone: None,
            }],
            stores: vec![
                Store {
                    id: StoreId(0),
                    chain: Chain::Coles,
                    name: "A".into(),
                    location: LatLon::new(-33.87, 151.30),
                    address: None,
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
                    address: None,
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
            AuditStrategy::default(),
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
            AuditStrategy::default(),
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
    fn audit_csv_neutralises_formula_prefixes() {
        assert_eq!(csv_field("=cmd"), "'=cmd");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("plain"), "plain");
    }

    #[test]
    fn audit_counts_legs_with_no_path() {
        let (problem, solution, _, node_ids) = fixture();
        // A graph with the depot->A and B->depot ways but nothing linking A
        // and B: exactly the middle leg has no path, and it is excluded from
        // the rows.
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
            None,
            None,
            None,
            None,
            false,
            Some(10),
        );
        graph.add_edge_with_way(
            NodeId(2),
            NodeId(0),
            18.0,
            18,
            Some("motorway".to_string()),
            None,
            None,
            None,
            None,
            false,
            Some(12),
        );
        let audit = audit_solution(
            &problem,
            &solution,
            &graph,
            &node_ids,
            TruckProfile::default(),
            AuditStrategy::default(),
        );
        assert_eq!(audit.legs_without_path, 1);
        assert_eq!(audit.distinct_ways, 2);
        assert!(audit.rows.iter().all(|row| row.way_id != Some(11)));
    }

    #[test]
    fn audit_can_replay_time_optimal_hierarchy_paths() {
        let (problem, solution, _, node_ids) = fixture();
        // Two parallel ways between the depot and store A: a short slow one
        // (way 10, distance-optimal) and a long fast one (way 20,
        // time-optimal). The audit must replay the objective the plan was
        // solved with, or it verifies corridors the plan never uses.
        let mut graph = Graph::new();
        for (node, lon) in [(0u64, 151.21), (1, 151.30), (2, 151.39)] {
            graph.add_node(NodeId(node), LatLon::new(-33.87, lon));
        }
        for (from, to, way, km, minutes) in [
            (0u64, 1u64, 10i64, 4.0, 20),
            (1, 0, 10, 4.0, 20),
            (0, 1, 20, 5.0, 10),
            (1, 0, 20, 5.0, 10),
            (1, 2, 11, 9.0, 9),
            (2, 1, 11, 9.0, 9),
            (2, 0, 12, 18.0, 18),
        ] {
            graph.add_edge_with_way(
                NodeId(from),
                NodeId(to),
                km,
                minutes,
                Some("primary".to_string()),
                None,
                None,
                None,
                None,
                false,
                Some(way),
            );
        }
        let dijkstra = audit_solution(
            &problem,
            &solution,
            &graph,
            &node_ids,
            TruckProfile::default(),
            AuditStrategy::Dijkstra,
        );
        assert!(dijkstra.rows.iter().any(|row| row.way_id == Some(10)));
        assert!(!dijkstra.rows.iter().any(|row| row.way_id == Some(20)));

        let hierarchy = audit_solution(
            &problem,
            &solution,
            &graph,
            &node_ids,
            TruckProfile::default(),
            AuditStrategy::ContractionHierarchy,
        );
        assert!(hierarchy.rows.iter().any(|row| row.way_id == Some(20)));
        assert!(!hierarchy.rows.iter().any(|row| row.way_id == Some(10)));
    }
}
