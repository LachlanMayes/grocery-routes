//! OSM PBF ingest: parse `.osm.pbf` extracts into a [`crate::graph::Graph`].
//!
//! Uses [`osmpbf`] to read nodes and ways, then builds a directed graph of
//! drivable way segments with truck-relevant tags.

use std::collections::HashMap;
use std::path::Path;

use osmpbf::Element;
use osmpbf::ElementReader;

use crate::graph::{Graph, NodeId};

use domain::LatLon;

/// Parses an OSM PBF file at `path` and builds a drivable road graph.
///
/// Only ways tagged as drivable (`highway` in [`DRIVABLE_HIGHWAY`]) are included.
/// One-way tags are respected: a one-way way adds a single directed edge; a
/// two-way way adds edges in both directions.
pub fn ingest_pbf(path: &Path) -> anyhow::Result<Graph> {
    // First pass: collect drivable ways and the set of node ids they reference.
    // We deliberately do NOT load every OSM node (there can be tens of
    // millions in a regional extract); only nodes that lie on a drivable way
    // are ever needed.
    let mut drivable_ways: Vec<DrivableWay> = Vec::new();
    let mut wanted: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let reader = ElementReader::from_path(path)?;
    reader.for_each(|element| {
        if let Element::Way(w) = element
            && let Some(way) = drivable_way(&w)
        {
            for &id in &way.nodes {
                wanted.insert(id);
            }
            drivable_ways.push(way);
        }
    })?;

    // Second pass: collect coordinates only for the wanted nodes.
    let mut all_nodes: HashMap<i64, (f64, f64)> = HashMap::with_capacity(wanted.len());
    let reader = ElementReader::from_path(path)?;
    reader.for_each(|element| match element {
        Element::Node(n) if wanted.contains(&n.id()) => {
            all_nodes.insert(n.id(), (n.lat(), n.lon()));
        }
        Element::DenseNode(n) if wanted.contains(&n.id()) => {
            all_nodes.insert(n.id(), (n.lat(), n.lon()));
        }
        _ => {}
    })?;

    // Materialise graph nodes (only those referenced by a drivable way).
    let mut graph = Graph::new();
    for way in &drivable_ways {
        for &id in &way.nodes {
            if id < 0 {
                continue;
            }
            if let Some(&(lat, lon)) = all_nodes.get(&id)
                && graph.node(NodeId(id as u64)).is_none()
            {
                graph.add_node(NodeId(id as u64), LatLon::new(lat, lon));
            }
        }
    }

    for way in drivable_ways {
        add_way_to_graph(&mut graph, &all_nodes, way);
    }

    Ok(graph)
}

// collect_nodes removed: ingest now collects coordinates lazily for nodes
// referenced by drivable ways only (see ingest_pbf).

/// OSM highway tags considered drivable for general truck routing.
const DRIVABLE_HIGHWAY: &[&str] = &[
    "motorway",
    "motorway_link",
    "trunk",
    "trunk_link",
    "primary",
    "primary_link",
    "secondary",
    "secondary_link",
    "tertiary",
    "tertiary_link",
    "residential",
    "unclassified",
    "service",
    "road",
];

/// Parses `maxweight` tag value into tonnes. Handles `"3.5t"`, `"3500"`,
/// `"10 t"`, `"10 tonnes"`, `"4500 kg"`, etc. Rejects non-positive values.
fn parse_maxweight(value: &str) -> Option<f64> {
    let lower = value.trim().to_ascii_lowercase();
    let parsed = if let Some(kg) = lower
        .strip_suffix("kgs")
        .or_else(|| lower.strip_suffix("kg"))
    {
        kg.trim().parse::<f64>().ok().map(|v| v / 1000.0)
    } else {
        let stripped = lower
            .strip_suffix("tonnes")
            .or_else(|| lower.strip_suffix("tonne"))
            .or_else(|| lower.strip_suffix("tons"))
            .or_else(|| lower.strip_suffix("ton"))
            .or_else(|| lower.strip_suffix('t'))
            .unwrap_or(&lower)
            .trim();
        stripped.parse::<f64>().ok()
    }?;
    (parsed.is_finite() && parsed > 0.0).then_some(parsed)
}

/// Parses `maxheight` tag value into metres. Handles `"4.2m"`, `"4200"`,
/// `"5 m"`, `"4.3 metres"`, `"420 cm"`, etc. Rejects non-positive values.
fn parse_maxheight(value: &str) -> Option<f64> {
    let lower = value.trim().to_ascii_lowercase();
    let parsed = if let Some(cm) = lower.strip_suffix("cm") {
        cm.trim().parse::<f64>().ok().map(|v| v / 100.0)
    } else {
        let stripped = lower
            .strip_suffix("metres")
            .or_else(|| lower.strip_suffix("meters"))
            .or_else(|| lower.strip_suffix("metre"))
            .or_else(|| lower.strip_suffix("meter"))
            .or_else(|| lower.strip_suffix('m'))
            .unwrap_or(&lower)
            .trim();
        stripped.parse::<f64>().ok()
    }?;
    (parsed.is_finite() && parsed > 0.0).then_some(parsed)
}

/// Parses `maxspeed` tag value into km/h. Supports numeric (`"60"`), explicit
/// units (`"60 km/h"`, `"60 kph"`, `"40 mph"`), and common OSM symbolic speeds.
fn parse_maxspeed_kmh(value: &str) -> Option<f64> {
    let lower = value.trim().to_ascii_lowercase();
    let kmh = match lower.as_str() {
        "au:urban" | "urban" => return Some(50.0),
        "au:rural" | "rural" => return Some(100.0),
        "walk" => return Some(10.0),
        _ => {
            if let Some(mph) = lower.strip_suffix("mph") {
                mph.trim().parse::<f64>().ok().map(|v| v * 1.60934)
            } else {
                let stripped = lower
                    .strip_suffix("km/h")
                    .or_else(|| lower.strip_suffix("kmh"))
                    .or_else(|| lower.strip_suffix("kph"))
                    .unwrap_or(&lower)
                    .trim();
                stripped.parse::<f64>().ok()
            }
        }
    }?;
    (kmh.is_finite() && (1.0..=150.0).contains(&kmh)).then_some(kmh)
}

/// Returns `Some(way)` if `w` is a drivable way, otherwise `None`.
/// Returns the way plus parsed truck-relevant tags.
fn drivable_way(w: &osmpbf::Way) -> Option<DrivableWay> {
    let mut highway: Option<&str> = None;
    let mut maxweight_tonnes: Option<f64> = None;
    let mut maxheight_m: Option<f64> = None;
    let mut maxspeed_kmh: Option<f64> = None;
    let mut hgv: Option<String> = None;
    let mut access: Option<String> = None;
    let mut oneway = false;
    let mut reverse = false;

    for (key, value) in w.tags() {
        match key {
            "highway" => {
                highway = Some(value);
            }
            "maxweight" => {
                maxweight_tonnes = parse_maxweight(value);
            }
            "maxheight" => {
                maxheight_m = parse_maxheight(value);
            }
            "maxspeed" => {
                maxspeed_kmh = parse_maxspeed_kmh(value);
            }
            "hgv" => {
                hgv = Some(value.to_ascii_lowercase());
            }
            "access" => {
                access = Some(value.to_ascii_lowercase());
            }
            "oneway" => {
                oneway = matches!(value, "yes" | "true" | "1");
                reverse = value == "-1";
            }
            _ => {}
        }
    }

    let highway = highway?;
    if !DRIVABLE_HIGHWAY.contains(&highway) {
        return None;
    }

    Some(DrivableWay {
        way_id: w.id(),
        nodes: w.refs().collect(),
        oneway,
        reverse,
        highway: highway.to_string(),
        maxweight_tonnes,
        maxheight_m,
        maxspeed_kmh,
        hgv,
        access,
    })
}

/// A drivable way parsed from OSM.
struct DrivableWay {
    way_id: i64,
    nodes: Vec<i64>,
    oneway: bool,
    reverse: bool,
    highway: String,
    maxweight_tonnes: Option<f64>,
    maxheight_m: Option<f64>,
    maxspeed_kmh: Option<f64>,
    hgv: Option<String>,
    access: Option<String>,
}

/// Speed (km/h) assumed for a highway type when no `maxspeed` is tagged.
fn highway_speed_kmh(highway: &str) -> f64 {
    match highway {
        "motorway" | "motorway_link" => 90.0,
        "trunk" | "trunk_link" => 80.0,
        "primary" | "primary_link" => 60.0,
        "secondary" | "secondary_link" => 50.0,
        "tertiary" | "tertiary_link" => 40.0,
        "residential" | "unclassified" | "service" | "road" => 30.0,
        _ => 40.0,
    }
}

/// Adds a drivable way's segments to the graph.
///
/// Consecutive node pairs become directed edges. Distance is haversine;
/// duration is distance / speed. One-way ways add a single edge; two-way ways
/// add edges in both directions (respecting `oneway=-1` reversal).
fn add_way_to_graph(graph: &mut Graph, nodes: &HashMap<i64, (f64, f64)>, way: DrivableWay) {
    if way.nodes.len() < 2 {
        return;
    }

    let speed = way
        .maxspeed_kmh
        .unwrap_or_else(|| highway_speed_kmh(&way.highway));

    // OSM oneway semantics:
    // - oneway=yes: forward direction only, in node order.
    // - oneway=-1: reverse direction only, against node order.
    // - otherwise: both directions.
    if !way.reverse {
        add_chain(graph, nodes, &way, false, way.oneway, speed);
    }
    if !way.oneway {
        add_chain(graph, nodes, &way, true, way.reverse, speed);
    }
}

fn add_chain(
    graph: &mut Graph,
    nodes: &HashMap<i64, (f64, f64)>,
    way: &DrivableWay,
    reverse: bool,
    oneway_edge: bool,
    speed: f64,
) {
    let chain: Vec<i64> = if reverse {
        way.nodes.iter().rev().copied().collect()
    } else {
        way.nodes.clone()
    };

    for pair in chain.windows(2) {
        let (from_id, to_id) = (pair[0], pair[1]);
        let Some((from_lat, from_lon)) = nodes.get(&from_id) else {
            continue;
        };
        let Some((to_lat, to_lon)) = nodes.get(&to_id) else {
            continue;
        };
        let distance_km = haversine_segment(*from_lat, *from_lon, *to_lat, *to_lon);
        let duration_seconds = (distance_km / speed) * 3600.0;
        let duration_minutes = (duration_seconds / 60.0).round() as i64;

        graph.add_edge_with_way_and_duration_secs(
            NodeId(from_id as u64),
            NodeId(to_id as u64),
            distance_km,
            duration_minutes,
            duration_seconds,
            Some(way.highway.clone()),
            way.maxweight_tonnes,
            way.maxheight_m,
            way.hgv.clone(),
            way.access.clone(),
            oneway_edge,
            Some(way.way_id),
        );
    }
}

/// Great-circle distance between two points in kilometres.
fn haversine_segment(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_KM: f64 = 6371.0088;
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let delta_phi = (lat2 - lat1).to_radians();
    let delta_lambda = (lon2 - lon1).to_radians();
    let h = (delta_phi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (delta_lambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().atan2((1.0 - h).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_maxweight_handles_tonnes_suffix() {
        assert_eq!(parse_maxweight("3.5t"), Some(3.5));
        assert_eq!(parse_maxweight("3500"), Some(3500.0));
        assert_eq!(parse_maxweight("10 t"), Some(10.0));
        assert_eq!(parse_maxweight("abc"), None);
    }

    #[test]
    fn parse_maxheight_handles_metres_suffix() {
        assert_eq!(parse_maxheight("4.2m"), Some(4.2));
        assert_eq!(parse_maxheight("4200"), Some(4200.0));
        assert_eq!(parse_maxheight("5 m"), Some(5.0));
        assert_eq!(parse_maxheight("430 cm"), Some(4.3));
        assert_eq!(parse_maxheight("none"), None);
        assert_eq!(parse_maxheight("abc"), None);
    }

    #[test]
    fn parse_maxweight_handles_additional_units() {
        assert_eq!(parse_maxweight("4500 kg"), Some(4.5));
        assert_eq!(parse_maxweight("12 tonnes"), Some(12.0));
        assert_eq!(parse_maxweight("none"), None);
        assert_eq!(parse_maxweight("-5"), None);
    }

    #[test]
    fn parse_maxspeed_handles_units_and_keywords() {
        assert_eq!(parse_maxspeed_kmh("60"), Some(60.0));
        assert_eq!(parse_maxspeed_kmh("80 km/h"), Some(80.0));
        assert_eq!(parse_maxspeed_kmh("100 kph"), Some(100.0));
        assert_eq!(parse_maxspeed_kmh("AU:urban"), Some(50.0));
        assert!((parse_maxspeed_kmh("50 mph").unwrap() - 80.467).abs() < 0.01);
        assert_eq!(parse_maxspeed_kmh("signals"), None);
    }
}
