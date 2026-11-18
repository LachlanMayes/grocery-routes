//! Node information catalog, screen-space label collision avoidance, and hit testing
//! for the console Route Map tab.

use std::collections::{BTreeMap, HashSet};

use domain::{Chain, Load, Minutes, Node, Problem, Solution, Store, TimeWindow};
use eframe::egui;

/// Unassigned store highlight color (amber/orange).
pub(crate) const UNASSIGNED_COLOR: egui::Color32 = egui::Color32::from_rgb(220, 110, 20);
/// Depot accent color.
pub(crate) const DEPOT_COLOR: egui::Color32 = egui::Color32::from_rgb(30, 80, 190);
/// Idle/unrouted depot accent color.
pub(crate) const IDLE_DEPOT_COLOR: egui::Color32 = egui::Color32::from_rgb(95, 110, 130);

/// One scheduled visit at a node on a route.
#[derive(Clone, Debug)]
pub(crate) struct MapNodeVisit {
    pub route_index: usize,
    pub step_index: usize,
    pub stop_sequence: Option<usize>,
    pub total_steps: usize,
    pub vehicle_name: String,
    pub driver_names: String,
    pub arrive: Minutes,
    pub depart: Minutes,
    pub wait_minutes: i64,
    pub prev_leg_km: f64,
    pub cumulative_km: f64,
    pub cumulative_load: Load,
    pub vehicle_capacity: Load,
}

/// Kind-specific metadata for rendering tooltips, badges, and inspector cards.
#[derive(Clone, Debug)]
pub(crate) enum MapNodeKind {
    Depot {
        routes_using: Vec<usize>,
    },
    ServedStore {
        chain: Chain,
        demand: Load,
        service: Minutes,
        windows: Vec<TimeWindow>,
        from_chain_defaults: bool,
    },
    UnassignedStore {
        chain: Chain,
        demand: Load,
        service: Minutes,
        windows: Vec<TimeWindow>,
        from_chain_defaults: bool,
        reason: String,
    },
}

/// Complete display information for a depot or store node on the map.
#[derive(Clone, Debug)]
pub(crate) struct MapNodeInfo {
    pub node: Node,
    pub title: String,
    pub short_badge: String,
    pub detail_badge: String,
    pub address: Option<String>,
    pub lon: f64,
    pub lat: f64,
    pub timezone: Option<String>,
    pub accent: egui::Color32,
    pub kind: MapNodeKind,
    pub visits: Vec<MapNodeVisit>,
}

/// Collects deduplicated node metadata for all depots, served stores on visible
/// routes, and unassigned stores in `problem`/`solution`.
pub(crate) fn collect_map_nodes(
    problem: &Problem,
    solution: &Solution,
    hidden_routes: &HashSet<usize>,
    route_color: fn(usize) -> egui::Color32,
) -> Vec<MapNodeInfo> {
    let mut depot_visits: BTreeMap<u32, Vec<MapNodeVisit>> = BTreeMap::new();
    let mut store_infos: Vec<MapNodeInfo> = Vec::new();
    let mut seen_stores: HashSet<u32> = HashSet::new();

    for (route_index, route) in solution.routes.iter().enumerate() {
        if hidden_routes.contains(&route_index) {
            continue;
        }
        let color = route_color(route_index);
        let vehicle = problem.vehicles.get(route.vehicle.0 as usize);
        let veh_name = vehicle.map(|v| v.name.as_str()).unwrap_or("?").to_string();
        let veh_cap = vehicle.map(|v| v.capacity).unwrap_or_default();
        let drv_names = route
            .drivers
            .iter()
            .map(|d| {
                problem
                    .drivers
                    .get(d.0 as usize)
                    .map(|drv| drv.name.as_str())
                    .unwrap_or("?")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let total_steps = route.steps.len();
        let mut prev_loc: Option<domain::LatLon> = None;
        let mut cum_km = 0.0_f64;
        let mut cum_load = Load::ZERO;
        let mut customer_seq = 0usize;

        for (step_index, stop) in route.steps.iter().enumerate() {
            let step_loc = node_location(problem, stop.node);
            let prev_leg_km = match (prev_loc, step_loc) {
                (Some(a), Some(b)) => domain::haversine_km(a, b),
                _ => 0.0,
            };
            cum_km += prev_leg_km;
            if let Some(loc) = step_loc {
                prev_loc = Some(loc);
            }

            match stop.node {
                Node::Depot(depot_id) => {
                    let visit = MapNodeVisit {
                        route_index,
                        step_index,
                        stop_sequence: None,
                        total_steps,
                        vehicle_name: veh_name.clone(),
                        driver_names: drv_names.clone(),
                        arrive: stop.arrive,
                        depart: stop.depart,
                        wait_minutes: 0,
                        prev_leg_km,
                        cumulative_km: cum_km,
                        cumulative_load: cum_load,
                        vehicle_capacity: veh_cap,
                    };
                    depot_visits.entry(depot_id.0).or_default().push(visit);
                }
                Node::Store(store_id) => {
                    customer_seq += 1;
                    let store = problem.store(store_id);
                    if let Some(store) = store {
                        cum_load += store.demand;
                    }
                    if !seen_stores.insert(store_id.0) {
                        continue;
                    }
                    if let Some(store) = store {
                        let wait_minutes = (stop.depart.minutes()
                            - store.service.minutes()
                            - stop.arrive.minutes())
                        .max(0);
                        let visit = MapNodeVisit {
                            route_index,
                            step_index,
                            stop_sequence: Some(customer_seq),
                            total_steps,
                            vehicle_name: veh_name.clone(),
                            driver_names: drv_names.clone(),
                            arrive: stop.arrive,
                            depart: stop.depart,
                            wait_minutes,
                            prev_leg_km,
                            cumulative_km: cum_km,
                            cumulative_load: cum_load,
                            vehicle_capacity: veh_cap,
                        };
                        let (windows, from_chain_defaults) = effective_windows(problem, store);
                        let total_demand = store.demand.total();
                        let detail_badge = if wait_minutes > 0 {
                            format!(
                                "{}-{} (+{}m wait) · {}u ({}A/{}C/{}F)",
                                visit.arrive,
                                visit.depart,
                                wait_minutes,
                                total_demand,
                                store.demand.ambient,
                                store.demand.chilled,
                                store.demand.frozen
                            )
                        } else {
                            format!(
                                "{}-{} · {}u ({}A/{}C/{}F)",
                                visit.arrive,
                                visit.depart,
                                total_demand,
                                store.demand.ambient,
                                store.demand.chilled,
                                store.demand.frozen
                            )
                        };
                        store_infos.push(MapNodeInfo {
                            node: Node::Store(store_id),
                            title: format!("{} (#{})", store.name, store_id.0),
                            short_badge: format!(
                                "R{}#{} {} ({})",
                                route_index + 1,
                                customer_seq,
                                store.name,
                                visit.arrive
                            ),
                            detail_badge,
                            address: store.address.clone(),
                            lon: store.location.lon,
                            lat: store.location.lat,
                            timezone: store.timezone.clone(),
                            accent: color,
                            kind: MapNodeKind::ServedStore {
                                chain: store.chain,
                                demand: store.demand,
                                service: store.service,
                                windows,
                                from_chain_defaults,
                            },
                            visits: vec![visit],
                        });
                    }
                }
            }
        }
    }

    let mut nodes: Vec<MapNodeInfo> =
        Vec::with_capacity(problem.depots.len() + store_infos.len() + solution.unassigned.len());

    // Put depots first so label collision priority favors hubs.
    for depot in &problem.depots {
        let visits = depot_visits.remove(&depot.id.0).unwrap_or_default();
        let mut routes_using: Vec<usize> = visits.iter().map(|v| v.route_index).collect();
        routes_using.sort_unstable();
        routes_using.dedup();

        let route_summary = if routes_using.is_empty() {
            "idle".to_string()
        } else {
            routes_using
                .iter()
                .map(|r| format!("R{}", r + 1))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let accent = if routes_using.is_empty() {
            IDLE_DEPOT_COLOR
        } else {
            DEPOT_COLOR
        };
        nodes.push(MapNodeInfo {
            node: Node::Depot(depot.id),
            title: format!("Depot #{} — {}", depot.id.0, depot.name),
            short_badge: format!("D{} {} ({route_summary})", depot.id.0, depot.name),
            detail_badge: if routes_using.is_empty() {
                "No active routes shown".to_string()
            } else {
                format!("Routes: {route_summary}")
            },
            address: depot.address.clone(),
            lon: depot.location.lon,
            lat: depot.location.lat,
            timezone: depot.timezone.clone(),
            accent,
            kind: MapNodeKind::Depot { routes_using },
            visits,
        });
    }

    nodes.extend(store_infos);

    for &store_id in &solution.unassigned {
        if !seen_stores.insert(store_id.0) {
            continue;
        }
        if let Some(store) = problem.store(store_id) {
            let (windows, from_chain_defaults) = effective_windows(problem, store);
            let total_demand = store.demand.total();
            let reason = diagnose_unassigned_store(problem, store);
            nodes.push(MapNodeInfo {
                node: Node::Store(store_id),
                title: format!("{} (#{})", store.name, store_id.0),
                short_badge: format!("! {} (unassigned)", store.name),
                detail_badge: format!(
                    "Unassigned · {}u ({}A/{}C/{}F)",
                    total_demand, store.demand.ambient, store.demand.chilled, store.demand.frozen
                ),
                address: store.address.clone(),
                lon: store.location.lon,
                lat: store.location.lat,
                timezone: store.timezone.clone(),
                accent: UNASSIGNED_COLOR,
                kind: MapNodeKind::UnassignedStore {
                    chain: store.chain,
                    demand: store.demand,
                    service: store.service,
                    windows,
                    from_chain_defaults,
                    reason,
                },
                visits: Vec::new(),
            });
        }
    }

    nodes
}

fn node_location(problem: &Problem, node: Node) -> Option<domain::LatLon> {
    match node {
        Node::Depot(id) => problem.depots.get(id.0 as usize).map(|d| d.location),
        Node::Store(id) => problem.store(id).map(|s| s.location),
    }
}

/// Diagnoses the primary structural reason an unassigned store could not be routed.
pub(crate) fn diagnose_unassigned_store(problem: &Problem, store: &Store) -> String {
    if problem.vehicles.is_empty() {
        return "No vehicles configured".to_string();
    }

    let fits_any_vehicle = problem.vehicles.iter().any(|v| {
        v.capacity.ambient >= store.demand.ambient
            && v.capacity.chilled >= store.demand.chilled
            && v.capacity.frozen >= store.demand.frozen
    });
    if !fits_any_vehicle {
        let max_a = problem
            .vehicles
            .iter()
            .map(|v| v.capacity.ambient)
            .max()
            .unwrap_or(0);
        let max_c = problem
            .vehicles
            .iter()
            .map(|v| v.capacity.chilled)
            .max()
            .unwrap_or(0);
        let max_f = problem
            .vehicles
            .iter()
            .map(|v| v.capacity.frozen)
            .max()
            .unwrap_or(0);
        return format!(
            "Demand ({}A/{}C/{}F) exceeds vehicle capacity (max {}A/{}C/{}F)",
            store.demand.ambient, store.demand.chilled, store.demand.frozen, max_a, max_c, max_f
        );
    }

    let (windows, _) = effective_windows(problem, store);
    if windows.is_empty() {
        return "No receiving window configured".to_string();
    }

    let any_shift_overlaps = problem.drivers.iter().any(|driver| {
        driver.shifts.iter().any(|shift| {
            windows
                .iter()
                .any(|win| shift.start < win.end && win.start < shift.end)
        })
    });
    if !any_shift_overlaps {
        return "Receiving window(s) do not overlap any driver shift".to_string();
    }

    if !problem.depots.is_empty() {
        let min_depot_km = problem
            .depots
            .iter()
            .map(|d| domain::haversine_km(d.location, store.location))
            .fold(f64::INFINITY, f64::min);
        let max_speed_kmh = problem
            .vehicles
            .iter()
            .map(|v| v.avg_speed_kmh)
            .filter(|s| s.is_finite() && *s > 0.0)
            .fold(0.0_f64, f64::max);
        if min_depot_km.is_finite() && max_speed_kmh > 0.0 {
            let min_round_trip = (2.0 * min_depot_km / max_speed_kmh * 60.0).round() as i64
                + store.service.minutes();
            if problem
                .vehicles
                .iter()
                .all(|v| min_round_trip > v.max_duration.minutes())
            {
                return "Round-trip travel + service exceeds vehicle max duration".to_string();
            }
        }
    }

    "Fleet time or capacity saturated by other stops — add a vehicle, increase capacity, or widen windows"
        .to_string()
}

fn effective_windows(problem: &Problem, store: &Store) -> (Vec<TimeWindow>, bool) {
    if !store.receiving.is_empty() {
        return (store.receiving.clone(), false);
    }
    match problem.chain_rules.windows_for(store.chain) {
        Some(defaults) if !defaults.is_empty() => (defaults.clone(), true),
        _ => (Vec::new(), false),
    }
}

/// Formats a list of receiving windows cleanly for UI display.
pub(crate) fn format_windows(windows: &[TimeWindow], from_chain_defaults: bool) -> String {
    if windows.is_empty() {
        return "Anytime (none configured)".to_string();
    }
    let joined = windows
        .iter()
        .map(|w| format!("{}–{}", w.start, w.end))
        .collect::<Vec<_>>()
        .join(", ");
    if from_chain_defaults {
        format!("{joined} (chain default)")
    } else {
        joined
    }
}

/// Finds the closest node within `max_radius` pixels of `pointer`, plus any
/// sibling nodes stacked within `3.0` px of that closest node.
pub(crate) fn nodes_near_pointer(
    projected: &[(usize, egui::Pos2)],
    pointer: egui::Pos2,
    max_radius: f32,
) -> Vec<usize> {
    let max_sq = max_radius * max_radius;
    let mut best: Option<(usize, egui::Pos2, f32)> = None;
    for &(idx, pos) in projected {
        let dist_sq = pos.distance_sq(pointer);
        if dist_sq <= max_sq && best.is_none_or(|(_, _, best_sq)| dist_sq < best_sq) {
            best = Some((idx, pos, dist_sq));
        }
    }
    let Some((_, best_pos, _)) = best else {
        return Vec::new();
    };
    const STACK_RADIUS_SQ: f32 = 3.0 * 3.0;
    projected
        .iter()
        .filter(|&&(_, pos)| pos.distance_sq(best_pos) <= STACK_RADIUS_SQ)
        .map(|&(idx, _)| idx)
        .collect()
}

/// Four screen-space label placements around a marker: Right, Left, Above, Below.
fn candidate_label_rects(marker: egui::Pos2, size: egui::Vec2) -> [egui::Rect; 4] {
    let pad = 9.0;
    [
        egui::Rect::from_min_size(egui::pos2(marker.x + pad, marker.y - size.y * 0.5), size),
        egui::Rect::from_min_size(
            egui::pos2(marker.x - pad - size.x, marker.y - size.y * 0.5),
            size,
        ),
        egui::Rect::from_min_size(
            egui::pos2(marker.x - size.x * 0.5, marker.y - pad - size.y),
            size,
        ),
        egui::Rect::from_min_size(egui::pos2(marker.x - size.x * 0.5, marker.y + pad), size),
    ]
}

/// Chooses the first candidate rectangle inside `clip` that does not collide with
/// any previously placed label rectangle.
pub(crate) fn pick_non_overlapping_rect(
    marker: egui::Pos2,
    label_size: egui::Vec2,
    occupied: &mut Vec<egui::Rect>,
    clip: egui::Rect,
) -> Option<egui::Rect> {
    for rect in candidate_label_rects(marker, label_size) {
        if !clip.contains_rect(rect) {
            continue;
        }
        let expanded = rect.expand(2.0);
        if occupied
            .iter()
            .any(|existing| existing.intersects(expanded))
        {
            continue;
        }
        occupied.push(rect);
        return Some(rect);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules, LatLon,
        PlannedStop, RoutePlan, Shift, StoreId, Vehicle, VehicleId,
    };

    fn dummy_color(idx: usize) -> egui::Color32 {
        egui::Color32::from_rgb(20 + idx as u8, 120, 200)
    }

    #[test]
    fn collect_map_nodes_dedupes_depots_and_includes_unassigned() {
        let problem = Problem {
            name: "test".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Main DC".into(),
                location: LatLon::new(-33.87, 151.21),
                address: None,
                timezone: Some("Australia/Sydney".into()),
            }],
            stores: vec![
                Store {
                    id: StoreId(0),
                    chain: Chain::Coles,
                    name: "Coles City".into(),
                    location: LatLon::new(-33.88, 151.22),
                    address: None,
                    demand: Load::new(4, 2, 1),
                    service: Minutes::from_minutes(15),
                    receiving: vec![TimeWindow::new(
                        Minutes::from_minutes(360),
                        Minutes::from_minutes(720),
                    )],
                    timezone: None,
                },
                Store {
                    id: StoreId(1),
                    chain: Chain::Aldi,
                    name: "ALDI Missed".into(),
                    location: LatLon::new(-33.90, 151.18),
                    address: None,
                    demand: Load::new(3, 0, 0),
                    service: Minutes::from_minutes(10),
                    receiving: vec![],
                    timezone: None,
                },
            ],
            vehicles: vec![Vehicle {
                id: VehicleId(0),
                name: "Truck 1".into(),
                depot: DepotId(0),
                capacity: Load::new(10, 10, 10),
                max_duration: Minutes::from_minutes(600),
                avg_speed_kmh: 50.0,
            }],
            drivers: vec![Driver {
                id: DriverId(0),
                name: "Alex".into(),
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
                        arrive: Minutes::from_minutes(360),
                        depart: Minutes::from_minutes(360),
                    },
                    PlannedStop {
                        node: Node::Store(StoreId(0)),
                        arrive: Minutes::from_minutes(375),
                        depart: Minutes::from_minutes(390),
                    },
                    PlannedStop {
                        node: Node::Depot(DepotId(0)),
                        arrive: Minutes::from_minutes(405),
                        depart: Minutes::from_minutes(405),
                    },
                ],
                distance_km: 12.0,
                duration: Minutes::from_minutes(45),
                cost_cents: 1500,
            }],
            unassigned: vec![StoreId(1)],
            total_distance_km: 12.0,
            total_duration: Minutes::from_minutes(45),
            total_cost_cents: 1500,
        };

        let nodes = collect_map_nodes(&problem, &solution, &HashSet::new(), dummy_color);
        assert_eq!(nodes.len(), 3);
        assert!(matches!(nodes[0].kind, MapNodeKind::Depot { .. }));
        assert_eq!(nodes[0].visits.len(), 2);
        assert!(nodes[1].short_badge.contains("Coles City"));
        assert!(nodes[1].detail_badge.contains("7u"));
        assert_eq!(nodes[2].node, Node::Store(StoreId(1)));
        assert!(matches!(nodes[2].kind, MapNodeKind::UnassignedStore { .. }));
    }

    #[test]
    fn label_collision_picks_fallback_side_or_culls_when_blocked() {
        let clip = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 400.0));
        let mut occupied = Vec::new();
        let first = pick_non_overlapping_rect(
            egui::pos2(200.0, 200.0),
            egui::vec2(80.0, 20.0),
            &mut occupied,
            clip,
        );
        let second = pick_non_overlapping_rect(
            egui::pos2(200.0, 200.0),
            egui::vec2(80.0, 20.0),
            &mut occupied,
            clip,
        );
        assert!(first.is_some());
        assert!(second.is_some());
        assert_ne!(first, second);
    }

    #[test]
    fn nodes_near_pointer_groups_stacked_markers() {
        let pts = vec![
            (0, egui::pos2(100.0, 100.0)),
            (1, egui::pos2(101.5, 100.5)),
            (2, egui::pos2(160.0, 100.0)),
        ];
        assert_eq!(
            nodes_near_pointer(&pts, egui::pos2(102.0, 101.0), 12.0),
            vec![0, 1]
        );
        assert!(nodes_near_pointer(&pts, egui::pos2(250.0, 250.0), 12.0).is_empty());
    }

    #[test]
    fn visit_tracks_sequence_wait_leg_km_and_cum_load() {
        let problem = Problem {
            name: "metrics".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Main DC".into(),
                location: LatLon::new(-33.87, 151.21),
                address: None,
                timezone: None,
            }],
            stores: vec![Store {
                id: StoreId(0),
                chain: Chain::General,
                name: "Early Stop".into(),
                location: LatLon::new(-33.89, 151.21),
                address: None,
                demand: Load::new(3, 1, 0),
                service: Minutes::from_minutes(15),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(400),
                    Minutes::from_minutes(600),
                )],
                timezone: None,
            }],
            vehicles: vec![Vehicle {
                id: VehicleId(0),
                name: "Van 1".into(),
                depot: DepotId(0),
                capacity: Load::new(10, 5, 0),
                max_duration: Minutes::from_minutes(480),
                avg_speed_kmh: 50.0,
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
                        arrive: Minutes::from_minutes(360),
                        depart: Minutes::from_minutes(360),
                    },
                    PlannedStop {
                        node: Node::Store(StoreId(0)),
                        arrive: Minutes::from_minutes(380),
                        // Arrives at 380, window opens at 400 (+20m wait), serves 15m -> departs 415.
                        depart: Minutes::from_minutes(415),
                    },
                    PlannedStop {
                        node: Node::Depot(DepotId(0)),
                        arrive: Minutes::from_minutes(435),
                        depart: Minutes::from_minutes(435),
                    },
                ],
                distance_km: 4.5,
                duration: Minutes::from_minutes(75),
                cost_cents: 1000,
            }],
            unassigned: vec![],
            total_distance_km: 4.5,
            total_duration: Minutes::from_minutes(75),
            total_cost_cents: 1000,
        };

        let nodes = collect_map_nodes(&problem, &solution, &HashSet::new(), dummy_color);
        let store_node = &nodes[1];
        let visit = &store_node.visits[0];
        assert_eq!(visit.stop_sequence, Some(1));
        assert_eq!(visit.wait_minutes, 20);
        assert!(visit.prev_leg_km > 1.5);
        assert!((visit.cumulative_km - visit.prev_leg_km).abs() < 1e-9);
        assert_eq!(visit.cumulative_load, Load::new(3, 1, 0));
        assert_eq!(visit.vehicle_capacity, Load::new(10, 5, 0));
        assert!(store_node.detail_badge.contains("+20m wait"));
    }

    #[test]
    fn diagnose_unassigned_store_explains_capacity_and_shift_failures() {
        let base_problem = Problem {
            name: "diag".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Depot".into(),
                location: LatLon::new(-33.87, 151.21),
                address: None,
                timezone: None,
            }],
            stores: vec![],
            vehicles: vec![Vehicle {
                id: VehicleId(0),
                name: "Truck".into(),
                depot: DepotId(0),
                capacity: Load::new(10, 4, 2),
                max_duration: Minutes::from_minutes(480),
                avg_speed_kmh: 50.0,
            }],
            drivers: vec![Driver {
                id: DriverId(0),
                name: "Driver".into(),
                shifts: vec![Shift::new(
                    Minutes::from_minutes(360),
                    Minutes::from_minutes(720),
                )],
                fatigue_rules: FatigueRules::default(),
            }],
            cost: CostModel::default(),
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
        };

        let oversize_store = Store {
            id: StoreId(0),
            chain: Chain::General,
            name: "Heavy Stop".into(),
            location: LatLon::new(-33.88, 151.22),
            address: None,
            demand: Load::new(15, 0, 0),
            service: Minutes::from_minutes(15),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(400),
                Minutes::from_minutes(600),
            )],
            timezone: None,
        };
        let cap_reason = diagnose_unassigned_store(&base_problem, &oversize_store);
        assert!(
            cap_reason.contains("exceeds vehicle capacity"),
            "{cap_reason}"
        );

        let late_store = Store {
            demand: Load::new(4, 0, 0),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(900),
                Minutes::from_minutes(1100),
            )],
            ..oversize_store
        };
        let shift_reason = diagnose_unassigned_store(&base_problem, &late_store);
        assert!(
            shift_reason.contains("do not overlap any driver shift"),
            "{shift_reason}"
        );
    }
}
