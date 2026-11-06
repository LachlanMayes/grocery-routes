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
    // First pass: collect all nodes with coordinates.
    let nodes = collect_nodes(path)?;

    // Re-open for the ways pass (ElementReader is single-pass).
    let mut graph = Graph::new();
    for (id, (lat, lon)) in &nodes {
        if *id < 0 {
            return Err(anyhow::anyhow!(
                "negative OSM node id {} cannot be represented as NodeId (u64)",
                id
            ));
        }
        graph.add_node(NodeId(*id as u64), LatLon::new(*lat, *lon));
    }

    let reader = ElementReader::from_path(path)?;
    reader.for_each(|element| {
        if let Element::Way(w) = element
            && let Some(way) = drivable_way(&w)
        {
            add_way_to_graph(&mut graph, &nodes, way);
        }
    })?;

    Ok(graph)
}

/// Collects all OSM nodes with coordinates from a PBF file.
fn collect_nodes(path: &Path) -> anyhow::Result<HashMap<i64, (f64, f64)>> {
    let reader = ElementReader::from_path(path)?;
    let mut nodes: HashMap<i64, (f64, f64)> = HashMap::new();
    reader.for_each(|element| {
        if let Element::Node(n) = element {
            nodes.insert(n.id(), (n.lat(), n.lon()));
        }
    })?;
    Ok(nodes)
}

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

/// Parses `maxweight` tag value into tonnes. Handles "3.5t", "3500", "10 t", etc.
fn parse_maxweight(value: &str) -> Option<f64> {
    let v = value.trim();
    let stripped = v.strip_suffix('t').unwrap_or(v).trim();
    stripped.parse::<f64>().ok()
}

/// Parses `maxheight` tag value into metres. Handles "4.2m", "4200", "5 m", etc.
fn parse_maxheight(value: &str) -> Option<f64> {
    let v = value.trim();
    let stripped = v.strip_suffix('m').unwrap_or(v).trim();
    stripped.parse::<f64>().ok()
}

/// Returns `Some(way)` if `w` is a drivable way, otherwise `None`.
/// Returns the way plus parsed truck-relevant tags.
fn drivable_way(w: &osmpbf::Way) -> Option<DrivableWay> {
    let mut highway: Option<&str> = None;
    let mut maxweight_tonnes: Option<f64> = None;
    let mut maxheight_m: Option<f64> = None;
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

    let speed = highway_speed_kmh(&way.highway);

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
        let duration_minutes = (distance_km / speed * 60.0).round() as i64;

        graph.add_edge_with_way(
            NodeId(from_id as u64),
            NodeId(to_id as u64),
            distance_km,
            duration_minutes,
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
        assert_eq!(parse_maxheight("abc"), None);
    }
}
