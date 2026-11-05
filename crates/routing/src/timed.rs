//! Time-dependent leg routing: curfew-aware travel between two graph nodes.
//!
//! This module answers the question the static matrix cannot: *given a
//! departure instant, when does the truck actually arrive, and what does it
//! wait for?* Curfew rules ([`domain::CurfewRules`]) name road classes (OSM
//! `highway` values) and local day/time windows; a traversal is blocked when
//! its **presence interval** `[departure, departure + duration)` overlaps a
//! window on a selected local day. A blocked truck waits at the node before
//! the edge until the traversal would be clear.
//!
//! Waiting makes the cost function FIFO-safe (departing later never arrives
//! earlier), so Dijkstra remains valid. The leg minimises arrival time;
//! `plain_leg` instead follows the distance-optimal path of
//! [`crate::router::Router`] for audits that must match the static matrix.
//!
//! Day/time resolution needs an absolute anchor and a network timezone; the
//! leg functions here refuse edges they cannot localise rather than guessing.

use std::collections::{BinaryHeap, HashMap};

use domain::{CurfewRule, CurfewRules, Minutes};
use jiff::Timestamp;
use jiff::civil::Weekday;
use jiff::tz::TimeZone;

use crate::graph::{EdgeId, Graph, NodeId};
use crate::router::Router;
use crate::truck_constraints::{TruckProfile, edge_allowed_for};

/// One edge of a computed leg with its scheduled times (horizon minutes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegStep {
    pub edge: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    /// Instant the truck enters the edge.
    pub depart: Minutes,
    /// Instant the truck leaves the edge (`depart + duration`).
    pub arrive: Minutes,
    /// Minutes waited at `from` before entering (curfew hold).
    pub wait_minutes: i64,
}

/// A computed leg from `source` to `target`.
#[derive(Clone, Debug, PartialEq)]
pub struct TimedLeg {
    /// Ordered node sequence, `source` first, `target` last.
    pub nodes: Vec<NodeId>,
    /// One step per traversed edge, in travel order.
    pub steps: Vec<LegStep>,
    pub distance_km: f64,
    /// Driving time only (excludes waits).
    pub drive_minutes: i64,
    /// Total curfew waiting time.
    pub wait_minutes: i64,
    /// Physical arrival instant (`depart + drive + wait`).
    pub arrive: Minutes,
}

impl TimedLeg {
    /// Total minutes the leg occupies (`drive + wait`).
    #[must_use]
    pub fn total_minutes(&self) -> i64 {
        self.drive_minutes + self.wait_minutes
    }

    fn zero(at: Minutes) -> Self {
        Self {
            nodes: Vec::new(),
            steps: Vec::new(),
            distance_km: 0.0,
            drive_minutes: 0,
            wait_minutes: 0,
            arrive: at,
        }
    }
}

/// Resolves horizon minutes to local weekday/time for curfew evaluation.
#[derive(Clone, Debug)]
pub struct CurfewContext {
    rules: CurfewRules,
    anchor: Timestamp,
    timezone: TimeZone,
}

impl CurfewContext {
    /// Builds a context from the problem's curfew rules, its horizon anchor
    /// and the network timezone (the zone the road network operates in).
    #[must_use]
    pub fn new(rules: CurfewRules, anchor: Timestamp, timezone: TimeZone) -> Self {
        Self {
            rules,
            anchor,
            timezone,
        }
    }

    /// The rules this context evaluates.
    #[must_use]
    pub fn rules(&self) -> &CurfewRules {
        &self.rules
    }

    /// Local `(minute_of_day, weekday)` for a horizon minute. `weekday` is
    /// 0 = Sunday. Returns `None` when the instant is not representable.
    #[must_use]
    pub fn local_parts(&self, at: Minutes) -> Option<(i64, usize)> {
        let seconds = at.minutes().checked_mul(60)?;
        let instant = self
            .anchor
            .checked_add(jiff::SignedDuration::from_secs(seconds))
            .ok()?;
        let zoned = instant.to_zoned(self.timezone.clone());
        let minute_of_day = i64::from(zoned.hour()) * 60 + i64::from(zoned.minute());
        Some((minute_of_day, sunday_zero(zoned.weekday())))
    }

    /// Minutes the truck must wait at a node before entering an edge of
    /// `road_class` so that its presence interval
    /// `[departure, departure + duration)` overlaps no curfew window.
    ///
    /// Returns `None` when no such departure exists within a week (treat the
    /// edge as unavailable) or when the instant cannot be localised.
    #[must_use]
    pub fn wait_before(&self, road_class: &str, at: Minutes, duration_minutes: i64) -> Option<i64> {
        let mut t = at.minutes();
        let elapsed_limit = self
            .rules
            .rules
            .len()
            .max(1)
            .saturating_mul(2880) // two days per rule, generously
            .saturating_add(7 * 1440) as i64;
        let mut waited = 0i64;
        loop {
            if self.traversal_allowed(road_class, t, duration_minutes)? {
                return Some(waited);
            }
            let jump = self.next_release_jump(road_class, t, duration_minutes)?;
            t = t.checked_add(jump)?;
            waited = waited.checked_add(jump)?;
            if waited > elapsed_limit {
                return None;
            }
        }
    }

    /// True when a traversal of `duration_minutes` entering at `t` can
    /// complete without overlapping any curfew window.
    fn traversal_allowed(&self, road_class: &str, t: i64, duration_minutes: i64) -> Option<bool> {
        let (start, start_day) = self.local_parts(Minutes::from_minutes(t))?;
        let end = start + duration_minutes.max(0);
        let (parts, part_count) = split_interval(start, end);
        for rule in self.matching_rules(road_class) {
            for &(day_offset, part_start, part_end) in &parts[..part_count] {
                let day = (start_day + day_offset) % 7;
                if occurrences(rule, day).any(|(window_start, window_end)| {
                    window_start < part_end && part_start < window_end
                }) {
                    return Some(false);
                }
            }
        }
        Some(true)
    }

    /// The largest jump (minutes) that could clear a currently intersecting
    /// window: every intersecting window occurrence's end, from which the
    /// caller re-checks.
    fn next_release_jump(&self, road_class: &str, t: i64, duration_minutes: i64) -> Option<i64> {
        let (start, start_day) = self.local_parts(Minutes::from_minutes(t))?;
        let end = start + duration_minutes.max(0);
        let (parts, part_count) = split_interval(start, end);
        let mut jump = 0i64;
        for rule in self.matching_rules(road_class) {
            for &(day_offset, part_start, part_end) in &parts[..part_count] {
                let day = (start_day + day_offset) % 7;
                for (window_start, window_end) in occurrences(rule, day) {
                    if window_start < part_end && part_start < window_end {
                        // Absolute offset of this window's end from `t`.
                        let offset = (day_offset as i64) * 1440 + window_end - start;
                        jump = jump.max(offset);
                    }
                }
            }
        }
        (jump > 0).then_some(jump)
    }

    fn matching_rules(&self, road_class: &str) -> impl Iterator<Item = &CurfewRule> {
        self.rules
            .rules
            .iter()
            .filter(move |rule| rule.road_class == road_class)
    }
}

/// Splits a traversal interval `[start, end)` in local minutes into day-local
/// parts `(day_offset, part_start, part_end)`, up to two parts. Allocation-free
/// because this runs inside Dijkstra relaxations.
fn split_interval(start: i64, end: i64) -> ([(usize, i64, i64); 2], usize) {
    let same_day_end = end.min(1440);
    let mut parts = [(0usize, start, same_day_end), (0usize, 0, 0)];
    let mut count = 1;
    if end > 1440 {
        parts[1] = (1, 0, end - 1440);
        count = 2;
    }
    (parts, count)
}

/// The curfew window occurrences (minute-of-day intervals, never wrapping)
/// that fall on local weekday `day`.
fn occurrences_on_rule(rule: &CurfewRule, day: usize) -> [(i64, i64); 2] {
    let selected = |index: usize| rule.days_of_week.is_none_or(|days| days[index % 7]);
    let start = rule.start.minutes();
    let end = rule.end.minutes();
    let mut windows = [(-1i64, -1i64); 2];
    if start < end {
        if selected(day) {
            windows[0] = (start, end);
        }
    } else if start > end {
        if selected(day) {
            // Evening part of the start day.
            windows[0] = (start, 1440);
        }
        if selected((day + 6) % 7) {
            // Morning part belongs to the previous day's window.
            windows[1] = (0, end);
        }
    }
    windows
}

/// The occupied window occurrences for a rule on a local day, skipping the
/// sentinel slots.
fn occurrences(rule: &CurfewRule, day: usize) -> impl Iterator<Item = (i64, i64)> {
    occurrences_on_rule(rule, day)
        .into_iter()
        .filter(|&(start, _end)| start >= 0)
}

fn sunday_zero(weekday: Weekday) -> usize {
    match weekday {
        Weekday::Sunday => 0,
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
    }
}

/// Computes the time-optimal leg from `source` to `target`, holding at nodes
/// until curfew-blocked edges become traversable.
///
/// Returns `None` when no path exists (profile-filtered, or every path is
/// blocked for longer than the wait budget).
#[must_use]
pub fn earliest_leg(
    graph: &Graph,
    source: NodeId,
    target: NodeId,
    depart: Minutes,
    profile: TruckProfile,
    curfews: &CurfewContext,
) -> Option<TimedLeg> {
    if source == target {
        return Some(TimedLeg::zero(depart));
    }

    // Label: earliest arrival time at a node (FIFO costs keep Dijkstra valid).
    let mut best: HashMap<NodeId, i64> = HashMap::new();
    let mut distance: HashMap<NodeId, f64> = HashMap::new();
    // node -> (predecessor, edge, wait before edge, edge departure)
    let mut prev: HashMap<NodeId, (NodeId, EdgeId, i64, i64)> = HashMap::new();
    let mut heap: BinaryHeap<std::cmp::Reverse<(i64, NodeId)>> = BinaryHeap::new();

    best.insert(source, depart.minutes());
    distance.insert(source, 0.0);
    heap.push(std::cmp::Reverse((depart.minutes(), source)));

    while let Some(std::cmp::Reverse((time, node))) = heap.pop() {
        if best.get(&node).is_some_and(|&known| time > known) {
            continue;
        }
        if node == target {
            break;
        }
        for (edge_id, next, edge) in graph.out_edges(node) {
            if edge_allowed_for(edge, profile).is_err() {
                continue;
            }
            let mut departure = time;
            let mut wait = 0i64;
            if let Some(class) = edge.highway.as_deref() {
                // An edge that cannot be localised, or a wait beyond the
                // budget, is treated as unusable for this departure — the
                // search simply tries other routes.
                let Some(waited) =
                    curfews.wait_before(class, Minutes::from_minutes(time), edge.duration_minutes)
                else {
                    continue;
                };
                departure = departure.checked_add(waited)?;
                wait = waited;
            }
            let arrive = departure.checked_add(edge.duration_minutes)?;
            if best.get(&next).is_none_or(|&known| arrive < known) {
                best.insert(next, arrive);
                distance.insert(
                    next,
                    distance.get(&node).copied().unwrap_or(0.0) + edge.distance_km,
                );
                prev.insert(next, (node, edge_id, wait, departure));
                heap.push(std::cmp::Reverse((arrive, next)));
            }
        }
    }

    reconstruct_leg(graph, source, target, best, distance, prev)
}

/// Computes a leg following the distance-optimal path of
/// [`crate::router::Router`] without applying curfews, with per-edge times.
///
/// Used by corridor audits that must describe the same path the static matrix
/// was built from.
#[must_use]
pub fn plain_leg(
    graph: &Graph,
    source: NodeId,
    target: NodeId,
    depart: Minutes,
    profile: TruckProfile,
) -> Option<TimedLeg> {
    if source == target {
        return Some(TimedLeg::zero(depart));
    }
    let route = Router::shortest_path(graph, source, target, profile)?;
    let mut steps = Vec::with_capacity(route.nodes.len().saturating_sub(1));
    let mut distance_km = 0.0;
    let mut time = depart.minutes();
    for pair in route.nodes.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let edge = best_edge(graph, from, to, profile)?;
        let arrive = time.checked_add(edge.duration_minutes)?;
        distance_km += edge.distance_km;
        steps.push(LegStep {
            edge: edge.id,
            from,
            to,
            depart: Minutes::from_minutes(time),
            arrive: Minutes::from_minutes(arrive),
            wait_minutes: 0,
        });
        time = arrive;
    }
    let drive_minutes = steps
        .iter()
        .map(|step| step.arrive.minutes() - step.depart.minutes() - step.wait_minutes)
        .sum();
    Some(TimedLeg {
        nodes: route.nodes,
        steps,
        distance_km,
        drive_minutes,
        wait_minutes: 0,
        arrive: Minutes::from_minutes(time),
    })
}

/// The allowed edge with the shortest travel time between two adjacent nodes
/// (ties broken by distance, then edge id for determinism).
fn best_edge<'g>(
    graph: &'g Graph,
    from: NodeId,
    to: NodeId,
    profile: TruckProfile,
) -> Option<&'g crate::graph::GraphEdge> {
    graph
        .out_edges(from)
        .into_iter()
        .filter(|(_, target, edge)| *target == to && edge_allowed_for(edge, profile).is_ok())
        .map(|(_, _, edge)| edge)
        .min_by(|a, b| {
            a.duration_minutes
                .cmp(&b.duration_minutes)
                .then_with(|| a.distance_km.total_cmp(&b.distance_km))
                .then_with(|| a.id.0.cmp(&b.id.0))
        })
}

fn reconstruct_leg(
    graph: &Graph,
    source: NodeId,
    target: NodeId,
    best: HashMap<NodeId, i64>,
    distance: HashMap<NodeId, f64>,
    prev: HashMap<NodeId, (NodeId, EdgeId, i64, i64)>,
) -> Option<TimedLeg> {
    let arrive = *best.get(&target)?;
    let mut nodes = vec![target];
    let mut steps: Vec<LegStep> = Vec::new();
    let mut current = target;
    while current != source {
        let &(previous, edge_id, wait, departure) = prev.get(&current)?;
        let edge = graph.edge(edge_id)?;
        steps.push(LegStep {
            edge: edge_id,
            from: previous,
            to: current,
            depart: Minutes::from_minutes(departure),
            arrive: Minutes::from_minutes(departure + edge.duration_minutes),
            wait_minutes: wait,
        });
        nodes.push(previous);
        current = previous;
    }
    nodes.reverse();
    steps.reverse();
    let drive_minutes = steps
        .iter()
        .map(|step| step.arrive.minutes() - step.depart.minutes())
        .sum();
    let wait_minutes = steps.iter().map(|step| step.wait_minutes).sum();
    Some(TimedLeg {
        nodes,
        steps,
        distance_km: distance.get(&target).copied().unwrap_or(0.0),
        drive_minutes,
        wait_minutes,
        arrive: Minutes::from_minutes(arrive),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{CurfewRule, LatLon};

    /// 2026-11-04T19:00:00Z = Thursday 06:00 AEDT (UTC+11).
    fn context_with(rules: Vec<CurfewRule>) -> CurfewContext {
        CurfewContext::new(
            CurfewRules { rules },
            "2026-11-04T19:00:00Z".parse().expect("valid anchor"),
            TimeZone::get("Australia/Sydney").expect("IANA zone"),
        )
    }

    fn residential_rule(start_minute: i64, end_minute: i64, days: Option<[bool; 7]>) -> CurfewRule {
        CurfewRule {
            road_class: "residential".to_string(),
            start: Minutes::from_minutes(start_minute),
            end: Minutes::from_minutes(end_minute),
            days_of_week: days,
        }
    }

    fn chain_graph() -> Graph {
        let mut graph = Graph::new();
        graph.add_node(NodeId(0), LatLon::new(-33.87, 151.21));
        graph.add_node(NodeId(1), LatLon::new(-33.87, 151.30));
        graph.add_node(NodeId(2), LatLon::new(-33.87, 151.39));
        // 0 -> 1 residential (10 min), 1 -> 2 primary (10 min).
        graph.add_edge(
            NodeId(0),
            NodeId(1),
            10.0,
            10,
            Some("residential".to_string()),
            None,
            None,
            None,
            None,
            false,
        );
        graph.add_edge(
            NodeId(1),
            NodeId(2),
            10.0,
            10,
            Some("primary".to_string()),
            None,
            None,
            None,
            None,
            false,
        );
        graph
    }

    #[test]
    fn local_parts_resolve_weekday_and_minute() {
        let ctx = context_with(Vec::new());
        // Horizon minute 0 = Thursday 06:00 local.
        assert_eq!(ctx.local_parts(Minutes::ZERO), Some((360, 4)));
        // + 3 days = Sunday 06:00.
        assert_eq!(
            ctx.local_parts(Minutes::from_minutes(3 * 1440)),
            Some((360, 0))
        );
    }

    #[test]
    fn plain_leg_matches_router_path() {
        let graph = chain_graph();
        let leg = plain_leg(
            &graph,
            NodeId(0),
            NodeId(2),
            Minutes::from_minutes(100),
            TruckProfile::default(),
        )
        .expect("path exists");
        assert_eq!(leg.nodes, vec![NodeId(0), NodeId(1), NodeId(2)]);
        assert_eq!(leg.distance_km, 20.0);
        assert_eq!(leg.drive_minutes, 20);
        assert_eq!(leg.wait_minutes, 0);
        assert_eq!(leg.arrive, Minutes::from_minutes(120));
        assert_eq!(leg.steps.len(), 2);
        assert_eq!(leg.steps[0].depart, Minutes::from_minutes(100));
    }

    #[test]
    fn traversal_clear_before_curfew_window_does_not_wait() {
        let ctx = context_with(vec![residential_rule(390, 480, None)]);
        let graph = chain_graph();
        // Depart 06:00; residential traversal 06:00-06:10, clear.
        let leg = earliest_leg(
            &graph,
            NodeId(0),
            NodeId(2),
            Minutes::ZERO,
            TruckProfile::default(),
            &ctx,
        )
        .expect("clear path exists");
        assert_eq!(leg.wait_minutes, 0);
        assert_eq!(leg.arrive, Minutes::from_minutes(20));
    }

    #[test]
    fn traversal_ending_exactly_at_window_start_is_allowed() {
        let ctx = context_with(vec![residential_rule(390, 480, None)]);
        let graph = chain_graph();
        // Depart 06:20; traversal 06:20-06:30 = window start, no overlap.
        let leg = earliest_leg(
            &graph,
            NodeId(0),
            NodeId(2),
            Minutes::from_minutes(20),
            TruckProfile::default(),
            &ctx,
        )
        .expect("path exists");
        assert_eq!(leg.wait_minutes, 0);
        assert_eq!(leg.arrive, Minutes::from_minutes(40));
    }

    #[test]
    fn traversal_overlapping_window_waits_until_window_end() {
        let ctx = context_with(vec![residential_rule(390, 480, None)]);
        let graph = chain_graph();
        // Depart 06:25; traversal would overlap 06:30, so wait until 08:00
        // (95 minutes), then drive 20 minutes.
        let leg = earliest_leg(
            &graph,
            NodeId(0),
            NodeId(2),
            Minutes::from_minutes(25),
            TruckProfile::default(),
            &ctx,
        )
        .expect("path exists");
        assert_eq!(leg.wait_minutes, 95);
        assert_eq!(leg.drive_minutes, 20);
        assert_eq!(leg.arrive, Minutes::from_minutes(140));
        // The wait is attributed to the residential edge (step 0).
        assert_eq!(leg.steps[0].wait_minutes, 95);
        assert_eq!(leg.steps[0].depart, Minutes::from_minutes(120));
    }

    #[test]
    fn curfew_wait_defers_to_an_open_alternate_route() {
        let mut graph = chain_graph();
        // Direct 0 -> 2 on primary, 25 minutes: slower than driving, faster
        // than waiting out the curfew.
        graph.add_edge(
            NodeId(0),
            NodeId(2),
            25.0,
            25,
            Some("primary".to_string()),
            None,
            None,
            None,
            None,
            false,
        );
        let ctx = context_with(vec![residential_rule(390, 480, None)]);
        let leg = earliest_leg(
            &graph,
            NodeId(0),
            NodeId(2),
            Minutes::from_minutes(25),
            TruckProfile::default(),
            &ctx,
        )
        .expect("alternate path exists");
        assert_eq!(leg.wait_minutes, 0);
        assert_eq!(leg.nodes, vec![NodeId(0), NodeId(2)]);
        assert_eq!(leg.arrive, Minutes::from_minutes(50));
    }

    #[test]
    fn day_mask_suppresses_window_on_other_days() {
        // Rule applies Fridays only; anchor is Saturday 06:00 local.
        let ctx = CurfewContext::new(
            CurfewRules {
                rules: vec![residential_rule(
                    390,
                    480,
                    Some([false, false, false, false, false, true, false]),
                )],
            },
            "2026-11-06T19:00:00Z".parse().expect("valid anchor"),
            TimeZone::get("Australia/Sydney").expect("IANA zone"),
        );
        let graph = chain_graph();
        let leg = earliest_leg(
            &graph,
            NodeId(0),
            NodeId(2),
            Minutes::from_minutes(25),
            TruckProfile::default(),
            &ctx,
        )
        .expect("path exists");
        assert_eq!(leg.wait_minutes, 0);
        assert_eq!(leg.arrive, Minutes::from_minutes(45));
    }

    #[test]
    fn wrap_midnight_window_covers_next_morning() {
        // Friday 22:00 -> 06:00; anchor Saturday 05:30 local (2026-11-06T18:30Z).
        let ctx = CurfewContext::new(
            CurfewRules {
                rules: vec![CurfewRule {
                    road_class: "residential".to_string(),
                    start: Minutes::from_minutes(22 * 60),
                    end: Minutes::from_minutes(6 * 60),
                    days_of_week: Some([false, false, false, false, false, true, false]),
                }],
            },
            "2026-11-06T18:30:00Z".parse().expect("valid anchor"),
            TimeZone::get("Australia/Sydney").expect("IANA zone"),
        );
        let graph = chain_graph();
        // Depart 05:55; traversal 05:55-06:05 overlaps the window's morning
        // part, so wait until 06:00 (5 minutes).
        let leg = earliest_leg(
            &graph,
            NodeId(0),
            NodeId(2),
            Minutes::from_minutes(25),
            TruckProfile::default(),
            &ctx,
        )
        .expect("path exists");
        assert_eq!(leg.wait_minutes, 5);
        assert_eq!(leg.arrive, Minutes::from_minutes(50));
    }

    #[test]
    fn untagged_road_class_is_never_curtailed() {
        let mut graph = Graph::new();
        graph.add_node(NodeId(0), LatLon::new(-33.87, 151.21));
        graph.add_node(NodeId(1), LatLon::new(-33.87, 151.30));
        graph.add_edge(
            NodeId(0),
            NodeId(1),
            10.0,
            10,
            None,
            None,
            None,
            None,
            None,
            false,
        );
        let ctx = context_with(vec![residential_rule(390, 480, None)]);
        let leg = earliest_leg(
            &graph,
            NodeId(0),
            NodeId(1),
            Minutes::from_minutes(25),
            TruckProfile::default(),
            &ctx,
        )
        .expect("untagged edge is not matched by class rules");
        assert_eq!(leg.wait_minutes, 0);
    }
}
