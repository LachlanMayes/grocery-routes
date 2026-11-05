//! Cross-layer test: `routing::timed` (time-dependent graph routing) feeding
//! `solver::enforce_curfews` over a solved plan.
//!
//! The CLI wires exactly these two pieces together; this test exercises the
//! integration without needing an OSM extract.

use domain::{
    Chain, ChainRules, CostModel, CurfewRule, CurfewRules, Depot, DepotId, Driver, DriverId,
    FatigueRules, LatLon, Load, Minutes, Node, Problem, Shift, Store, StoreId, TimeWindow, Vehicle,
    VehicleId,
};
use routing::{CurfewContext, Graph, NodeId, TimedLeg, TruckProfile};
use solver::{CurfewDrop, enforce_curfews, solve_greedy};

/// 2026-11-04T19:00:00Z = Thursday 06:00 AEDT.
const ANCHOR: &str = "2026-11-04T19:00:00Z";

fn graph_fixture() -> Graph {
    let mut graph = Graph::new();
    graph.add_node(NodeId(0), LatLon::new(-33.87, 151.21)); // depot
    graph.add_node(NodeId(1), LatLon::new(-33.85, 151.30)); // store A
    graph.add_node(NodeId(2), LatLon::new(-33.83, 151.39)); // store B
    // All roads residential; travel times chosen so the A->B leg crosses the
    // 06:30 curfew window.
    for (from, to, minutes) in [
        (0, 1, 25),
        (1, 2, 15),
        (2, 0, 30),
        (1, 0, 25),
        (2, 1, 15),
        (0, 2, 30),
    ] {
        graph.add_edge(
            NodeId(from),
            NodeId(to),
            20.0,
            minutes,
            Some("residential".to_string()),
            None,
            None,
            None,
            None,
            false,
        );
    }
    graph
}

fn problem_fixture() -> Problem {
    let anchor = ANCHOR.parse().expect("valid anchor");
    Problem {
        name: "curfew integration".into(),
        depots: vec![Depot {
            id: DepotId(0),
            name: "DC".into(),
            location: LatLon::new(-33.87, 151.21),
            timezone: Some("Australia/Sydney".into()),
        }],
        stores: vec![
            Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "A".into(),
                location: LatLon::new(-33.85, 151.30),
                demand: Load::new(2, 2, 1),
                service: Minutes::from_minutes(10),
                receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(120))],
                timezone: Some("Australia/Sydney".into()),
            },
            Store {
                id: StoreId(1),
                chain: Chain::Coles,
                name: "B".into(),
                location: LatLon::new(-33.83, 151.39),
                demand: Load::new(1, 1, 1),
                service: Minutes::from_minutes(10),
                receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(120))],
                timezone: Some("Australia/Sydney".into()),
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
        curfews: CurfewRules {
            rules: vec![CurfewRule {
                road_class: "residential".to_string(),
                // 06:30-08:00 local, every day.
                start: Minutes::from_minutes(390),
                end: Minutes::from_minutes(480),
                days_of_week: None,
            }],
        },
        horizon_start: Some(anchor),
    }
}

/// The same mapping the CLI builds: problem node -> graph node, depot first.
struct Provider {
    graph: Graph,
    node_ids: Vec<NodeId>,
    context: CurfewContext,
}

impl solver::LegTimes for Provider {
    fn leg(&mut self, from: Node, to: Node, depart: Minutes) -> Option<TimedLeg> {
        let index = |node: Node| match node {
            Node::Depot(id) => id.0 as usize,
            Node::Store(id) => 1 + id.0 as usize,
        };
        let from_id = self.node_ids[index(from)];
        let to_id = self.node_ids[index(to)];
        routing::earliest_leg(
            &self.graph,
            from_id,
            to_id,
            depart,
            TruckProfile::default(),
            &self.context,
        )
    }
}

#[test]
fn curfew_enforcement_drops_the_store_whose_window_the_wait_breaks() {
    let problem = problem_fixture();
    let solution = solve_greedy(&problem);
    assert_eq!(
        solution.validate(&problem),
        Ok(()),
        "the static plan must be feasible"
    );

    let context = CurfewContext::new(
        problem.curfews.clone(),
        problem.horizon_start.expect("anchor set"),
        jiff::tz::TimeZone::get("Australia/Sydney").expect("IANA zone"),
    );
    let mut provider = Provider {
        graph: graph_fixture(),
        node_ids: vec![NodeId(0), NodeId(1), NodeId(2)],
        context,
    };
    let (enforced, report) = enforce_curfews(&problem, &solution, &mut provider);

    assert_eq!(enforced.validate(&problem), Ok(()));
    // Whichever store the planner visits second, its leg leaves the first
    // stop inside the 06:30-08:00 window, so the wait pushes it past its
    // 120-minute receiving window and it is dropped.
    let last_visited = solution.routes[0]
        .steps
        .iter()
        .rev()
        .find_map(|step| match step.node {
            Node::Store(id) => Some(id),
            Node::Depot(_) => None,
        })
        .expect("the plan visits two stores");
    assert_eq!(
        report.dropped,
        vec![CurfewDrop {
            store: last_visited,
            route: 0,
            kind: solver::BreachKind::ReceivingWindowMissed,
        }],
        "the second-visited store cannot be reached before its window closes"
    );
    assert_eq!(enforced.unassigned, vec![last_visited]);
    let survivor = StoreId(1 - last_visited.0);
    assert!(
        enforced
            .routes
            .iter()
            .flat_map(|route| &route.steps)
            .any(|step| step.node == Node::Store(survivor)),
        "the first-visited store should remain served"
    );
}
