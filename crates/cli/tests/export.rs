//! Export tests: HTML escaping, AUD formatting, and the two report renderers.
//!
//! The fixture mirrors the hand-built style of `crates/domain/tests/validation.rs`
//! and deliberately uses names that need HTML escaping.

use cli::export::{escape_html, format_aud_cents, route_sheet_html, routes_geojson};
use domain::{
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Node, PlannedStop, Problem, RoutePlan, Shift, Solution, Store, StoreId,
    TimeWindow, Vehicle, VehicleId,
};
use serde_json::Value;

/// A one-route, two-store fixture (one store unassigned) with hostile names.
fn tiny_fixture() -> (Problem, Solution) {
    let problem = Problem {
        name: "tiny fixture <&>".into(),
        depots: vec![Depot {
            id: DepotId(0),
            name: "Depot & Co <North>".into(),
            location: LatLon::new(-33.79, 150.90),
            timezone: None,
        }],
        stores: vec![
            Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "Coles O'Brien & Sons \"Fresh\"".into(),
                location: LatLon::new(-33.80, 150.95),
                timezone: None,
                demand: Load::new(2, 1, 0),
                service: Minutes::from_minutes(15),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(480),
                    Minutes::from_minutes(720),
                )],
            },
            Store {
                id: StoreId(1),
                chain: Chain::Woolworths,
                name: "Woolworths <unassigned>".into(),
                location: LatLon::new(-33.85, 151.00),
                timezone: None,
                demand: Load::new(1, 0, 1),
                service: Minutes::from_minutes(15),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(480),
                    Minutes::from_minutes(720),
                )],
            },
        ],
        vehicles: vec![Vehicle {
            id: VehicleId(0),
            name: "B-double & trailer".into(),
            depot: DepotId(0),
            capacity: Load::new(20, 10, 10),
            max_duration: Minutes::from_minutes(720),
            avg_speed_kmh: 60.0,
        }],
        drivers: vec![Driver {
            id: DriverId(0),
            name: "D'Souza & <Sam>".into(),
            shifts: vec![Shift::new(
                Minutes::from_minutes(420),
                Minutes::from_minutes(780),
            )],
            fatigue_rules: FatigueRules::default(),
        }],
        cost: CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 90,
            per_hour_cents: 4_500,
            late_per_min_cents: 0,
        },
        chain_rules: ChainRules::default(),
        curfews: CurfewRules::default(),
    };

    let solution = Solution {
        routes: vec![RoutePlan {
            vehicle: VehicleId(0),
            drivers: vec![DriverId(0)],
            steps: vec![
                PlannedStop {
                    node: Node::Depot(DepotId(0)),
                    arrive: Minutes::from_minutes(480),
                    depart: Minutes::from_minutes(480),
                },
                PlannedStop {
                    node: Node::Store(StoreId(0)),
                    arrive: Minutes::from_minutes(540),
                    depart: Minutes::from_minutes(555),
                },
                PlannedStop {
                    node: Node::Depot(DepotId(0)),
                    arrive: Minutes::from_minutes(600),
                    depart: Minutes::from_minutes(600),
                },
            ],
            distance_km: 42.5,
            duration: Minutes::from_minutes(120),
            cost_cents: 21_325,
        }],
        unassigned: vec![StoreId(1)],
        total_distance_km: 42.5,
        total_duration: Minutes::from_minutes(120),
        total_cost_cents: 21_325,
    };

    (problem, solution)
}

#[test]
fn escape_html_handles_edge_cases() {
    assert_eq!(escape_html(""), "");
    assert_eq!(escape_html("plain"), "plain");
    assert_eq!(escape_html("&<>\"'"), "&amp;&lt;&gt;&quot;&#39;");
}

#[test]
fn format_aud_cents_handles_edge_cases() {
    assert_eq!(format_aud_cents(0), "$0.00");
    assert_eq!(format_aud_cents(5), "$0.05");
    assert_eq!(format_aud_cents(99), "$0.99");
    assert_eq!(format_aud_cents(100), "$1.00");
    assert_eq!(format_aud_cents(123_456), "$1,234.56");
    assert_eq!(format_aud_cents(1_234_567), "$12,345.67");
    assert_eq!(format_aud_cents(100_000_000), "$1,000,000.00");
}

#[test]
fn route_sheet_html_escapes_names_and_renders_every_step() {
    let (problem, solution) = tiny_fixture();
    let html = route_sheet_html(&problem, &solution);

    // Self-contained document skeleton with inline CSS.
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("<style>"));
    assert!(html.contains("Route sheet: tiny fixture &lt;&amp;&gt;"));

    // Every interpolated name is escaped.
    assert!(html.contains("Depot &amp; Co &lt;North&gt;"));
    assert!(html.contains("Coles O&#39;Brien &amp; Sons &quot;Fresh&quot;"));
    assert!(html.contains("B-double &amp; trailer"));
    assert!(html.contains("D&#39;Souza &amp; &lt;Sam&gt;"));
    assert!(!html.contains("<North>"));

    // One route section; one table row per step plus the header row.
    assert!(html.contains("Route 1"));
    assert_eq!(html.matches("<tr>").count(), 4);
    assert!(html.contains("<td>1</td>"));
    assert!(html.contains("<td class=\"tag\">depot</td>"));
    assert!(html.contains("<td class=\"tag\">store</td>"));
    assert!(html.contains("<td>2 / 1 / 0</td>"));

    // Times use `Minutes`' Display.
    assert!(html.contains("<td>08:00</td>"));
    assert!(html.contains("<td>09:00</td>"));
    assert!(html.contains("<td>09:15</td>"));

    // The unassigned store gets its own section, with its name escaped.
    assert!(html.contains("<h2>Unassigned</h2>"));
    assert!(html.contains("Woolworths &lt;unassigned&gt;"));

    // Totals and route cost are formatted as AUD.
    assert!(html.contains("$213.25"));
}

#[test]
fn routes_geojson_has_route_lines_and_store_points() {
    let (problem, solution) = tiny_fixture();
    let geojson = routes_geojson(&problem, &solution);
    let document: Value = serde_json::from_str(&geojson).expect("GeoJSON parses");

    assert_eq!(document["type"], "FeatureCollection");
    let features = document["features"].as_array().expect("features array");
    assert_eq!(features.len(), solution.routes.len() + problem.stores.len());

    // One LineString per route, one coordinate per known step.
    let route = &features[0];
    assert_eq!(route["geometry"]["type"], "LineString");
    let coordinates = route["geometry"]["coordinates"]
        .as_array()
        .expect("coordinates array");
    assert_eq!(coordinates.len(), 3);
    for coordinate in coordinates {
        let pair = coordinate.as_array().expect("coordinate pair");
        assert_eq!(pair.len(), 2, "coordinate must be [lon, lat]: {coordinate}");
        assert!(pair[0].is_number(), "longitude: {coordinate}");
        assert!(pair[1].is_number(), "latitude: {coordinate}");
    }
    // Longitude comes first.
    assert!((coordinates[0][0].as_f64().expect("lon") - 150.90).abs() < 1e-9);
    assert!((coordinates[0][1].as_f64().expect("lat") - -33.79).abs() < 1e-9);

    let properties = &route["properties"];
    assert_eq!(properties["route"], 0);
    assert_eq!(properties["vehicle"], "B-double & trailer");
    assert_eq!(
        properties["drivers"],
        serde_json::json!(["D'Souza & <Sam>"])
    );
    assert!((properties["distance_km"].as_f64().expect("distance") - 42.5).abs() < 1e-9);
    assert_eq!(properties["duration"], "02:00");
    assert_eq!(properties["stop_count"], 3);

    // One Point per store, serving the unassigned one too.
    let points: Vec<&Value> = features
        .iter()
        .filter(|feature| feature["geometry"]["type"] == "Point")
        .collect();
    assert_eq!(points.len(), problem.stores.len());
    let woolworths = points
        .iter()
        .find(|feature| feature["properties"]["store_id"] == 1)
        .expect("store 1 feature");
    assert_eq!(woolworths["properties"]["chain"], "Woolworths");
    assert_eq!(woolworths["properties"]["name"], "Woolworths <unassigned>");
    let point_coordinates = woolworths["geometry"]["coordinates"]
        .as_array()
        .expect("point coordinates");
    assert_eq!(point_coordinates.len(), 2);
    assert!((point_coordinates[0].as_f64().expect("lon") - 151.00).abs() < 1e-9);
    assert!((point_coordinates[1].as_f64().expect("lat") - -33.85).abs() < 1e-9);
}
