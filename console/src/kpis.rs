//! KPI analytics, CSV/JSON/text exports, and interactive `egui_plot` charts
//! for the operator console's **KPIs** tab.
//!
//! All derived metrics are computed directly from the already-fetched
//! [`KpisBundle`] (`Kpis` + `domain::Problem` + `domain::Solution`) and
//! `recent_solutions` history without extra API calls.

use std::collections::BTreeMap;
use std::ops::AddAssign;

use domain::{Chain, CostModel, Load, Minutes, Node, Problem, RoutePlan};
use eframe::egui;
use egui_plot::{
    Bar, BarChart, Corner, GridInput, GridMark, HLine, HoverPosition, Legend, Line, LineStyle,
    MarkerShape, Plot, PlotPoint, PlotPoints, Points, Polygon, Text,
};
use serde::Serialize;

use crate::api_client::SolutionSummary;
use crate::app::{
    KpisBundle, copy_button, driver_names, route_color, stat_card, store_stop_count, vehicle_name,
};

/// Metric displayed in the main per-route KPI chart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum KpiMetric {
    #[default]
    CostBreakdown,
    Distance,
    Duration,
    Stops,
    Utilization,
}

impl KpiMetric {
    /// Human-readable label for selector pills and export descriptions.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::CostBreakdown => "Cost Breakdown ($)",
            Self::Distance => "Distance (km)",
            Self::Duration => "Duration (hrs)",
            Self::Stops => "Stops",
            Self::Utilization => "Load Utilization (%)",
        }
    }

    fn y_axis_label(self) -> &'static str {
        match self {
            Self::CostBreakdown => "Cost ($)",
            Self::Distance => "Distance (km)",
            Self::Duration => "Hours",
            Self::Stops => "Stops",
            Self::Utilization => "Capacity Used (%)",
        }
    }
}

/// Ordering applied to the per-route bar chart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum KpiChartSort {
    #[default]
    RouteOrder,
    HighestFirst,
}

/// Sortable column in the per-route KPI table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum KpiTableColumn {
    #[default]
    Route,
    Vehicle,
    Stops,
    Distance,
    Duration,
    PeakLoad,
    Cost,
    CostPerKm,
}

/// Metric plotted in the historical solve-comparison chart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum HistoryMetric {
    #[default]
    Cost,
    Distance,
    ServeRate,
}

/// UI state persisted for the KPIs tab inside [`crate::app::ConsoleApp`].
#[derive(Clone, Debug)]
pub(crate) struct KpisUiState {
    pub metric: KpiMetric,
    pub chart_sort: KpiChartSort,
    pub show_avg_line: bool,
    pub show_limit_lines: bool,
    pub history_metric: HistoryMetric,
    pub table_sort: KpiTableColumn,
    pub table_descending: bool,
    pub selected_route: Option<usize>,
    /// `(is_error, status_message)` from export actions.
    pub action_status: Option<(bool, String)>,
}

impl Default for KpisUiState {
    fn default() -> Self {
        Self {
            metric: KpiMetric::CostBreakdown,
            chart_sort: KpiChartSort::RouteOrder,
            show_avg_line: true,
            show_limit_lines: true,
            history_metric: HistoryMetric::Cost,
            table_sort: KpiTableColumn::Route,
            table_descending: false,
            selected_route: None,
            action_status: None,
        }
    }
}

/// Navigation actions triggered from the KPIs dashboard back into the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KpisAction {
    OpenMap {
        solution_id: String,
    },
    OpenSolution {
        solution_id: String,
        route_index: Option<usize>,
    },
}

/// Decomposed cost components in Australian cents, guaranteed to sum to `total_cents`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct CostBreakdown {
    pub fixed_cents: i64,
    pub distance_cents: i64,
    pub time_cents: i64,
    pub other_cents: i64,
    pub total_cents: i64,
}

impl CostBreakdown {
    /// Percentage share in `0.0..=100.0` of a component against `total_cents`.
    pub(crate) fn pct_of(self, component_cents: i64) -> f64 {
        if self.total_cents <= 0 {
            0.0
        } else {
            (component_cents as f64 / self.total_cents as f64) * 100.0
        }
    }
}

impl AddAssign for CostBreakdown {
    fn add_assign(&mut self, rhs: Self) {
        self.fixed_cents = self.fixed_cents.saturating_add(rhs.fixed_cents);
        self.distance_cents = self.distance_cents.saturating_add(rhs.distance_cents);
        self.time_cents = self.time_cents.saturating_add(rhs.time_cents);
        self.other_cents = self.other_cents.saturating_add(rhs.other_cents);
        self.total_cents = self.total_cents.saturating_add(rhs.total_cents);
    }
}

/// Reconciled per-route or aggregate cost breakdown matching solver rounding.
pub(crate) fn compute_cost_breakdown(
    model: CostModel,
    distance_km: f64,
    duration_min: i64,
    total_cost_cents: i64,
) -> CostBreakdown {
    let model_configured =
        model.fixed_per_route_cents != 0 || model.per_km_cents != 0 || model.per_hour_cents != 0;
    if !model_configured {
        return CostBreakdown {
            fixed_cents: 0,
            distance_cents: 0,
            time_cents: 0,
            other_cents: total_cost_cents,
            total_cents: total_cost_cents,
        };
    }

    // Match solver/src/greedy.rs rounding so fixed + distance + time equals the
    // solver total cleanly without float drift.
    let fixed_cents = model.fixed_per_route_cents;
    let distance_millikm = (distance_km * 1000.0).round() as i64;
    let distance_cents = (distance_millikm * model.per_km_cents + 500) / 1000;
    let time_cents = (duration_min * model.per_hour_cents + 30) / 60;
    let modeled_sum = fixed_cents
        .saturating_add(distance_cents)
        .saturating_add(time_cents);
    let other_cents = total_cost_cents.saturating_sub(modeled_sum);

    CostBreakdown {
        fixed_cents,
        distance_cents,
        time_cents,
        other_cents,
        total_cents: total_cost_cents,
    }
}

/// Compartment delivery vs capacity utilization metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub(crate) struct LoadUtilization {
    pub delivered: Load,
    pub capacity: Load,
    pub ambient_pct: f64,
    pub chilled_pct: f64,
    pub frozen_pct: f64,
    pub total_pct: f64,
    pub peak_pct: f64,
    pub bottleneck: &'static str,
}

impl LoadUtilization {
    /// Compute utilization percentages from delivered load and capacity.
    pub(crate) fn from_loads(delivered: Load, capacity: Load) -> Self {
        let pct = |del: u32, cap: u32| -> Option<f64> {
            (cap > 0).then(|| (del as f64 / cap as f64) * 100.0)
        };
        let amb = pct(delivered.ambient, capacity.ambient);
        let chl = pct(delivered.chilled, capacity.chilled);
        let frz = pct(delivered.frozen, capacity.frozen);

        let total_cap = capacity.total();
        let total_pct = if total_cap > 0 {
            (delivered.total() as f64 / total_cap as f64) * 100.0
        } else {
            0.0
        };

        let mut peak_pct = 0.0_f64;
        let mut bottleneck = "-";
        for (name, opt_pct) in [("Amb", amb), ("Chl", chl), ("Frz", frz)] {
            if let Some(p) = opt_pct
                && (bottleneck == "-" || p > peak_pct)
            {
                peak_pct = p;
                bottleneck = name;
            }
        }

        Self {
            delivered,
            capacity,
            ambient_pct: amb.unwrap_or(0.0),
            chilled_pct: chl.unwrap_or(0.0),
            frozen_pct: frz.unwrap_or(0.0),
            total_pct,
            peak_pct,
            bottleneck,
        }
    }
}

/// Full derived metrics for a single executed route.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct RouteKpiRow {
    pub route_index: usize,
    pub route_label: String,
    pub vehicle_name: String,
    pub depot_name: String,
    pub driver_names: String,
    pub stops: usize,
    pub distance_km: f64,
    pub duration_min: i64,
    pub service_min: i64,
    pub travel_and_wait_min: i64,
    pub max_duration_min: i64,
    pub duration_pct: f64,
    pub start_min: Option<i64>,
    pub end_min: Option<i64>,
    pub cost: CostBreakdown,
    pub cost_per_km_cents: f64,
    pub cost_per_stop_cents: f64,
    pub load: LoadUtilization,
}

/// Service & load breakdown grouped by store category / banner (`Chain`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CategoryKpiRow {
    pub category: String,
    pub total_stores: usize,
    pub served_stores: usize,
    pub unassigned_stores: usize,
    pub served_demand: Load,
    pub unassigned_demand: Load,
}

/// Unserved place detail row so the operator sees unmet demand & windows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct UnassignedStoreRow {
    pub store_id: u32,
    pub name: String,
    pub category: String,
    pub demand: Load,
    pub service_min: i64,
    pub windows: String,
}

/// Historical solve point on the same problem for trend/LNS comparison.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct RunComparisonPoint {
    pub run_index: usize,
    pub solution_id: String,
    pub solver_tag: String,
    pub solved_at: String,
    pub used_lns: bool,
    pub served: i64,
    pub total: i64,
    pub serve_rate_pct: f64,
    pub distance_km: f64,
    pub cost_cents: i64,
    pub is_current: bool,
}

/// Complete analytics report powering the KPIs dashboard and exports.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct KpisReport {
    pub solution_id: String,
    pub problem_id: String,
    pub problem_name: String,
    pub served: usize,
    pub unassigned: usize,
    pub total: usize,
    pub serve_rate: f64,
    pub active_routes: usize,
    pub total_vehicles: usize,
    pub fleet_utilization_pct: f64,
    pub total_distance_km: f64,
    pub avg_distance_per_route_km: f64,
    pub total_duration_min: i64,
    pub total_service_min: i64,
    pub total_travel_wait_min: i64,
    pub avg_duration_min: f64,
    pub cost: CostBreakdown,
    pub avg_cost_per_route_cents: f64,
    pub cost_per_km_cents: f64,
    pub cost_per_stop_cents: f64,
    pub total_delivered: Load,
    pub unassigned_demand: Load,
    pub active_capacity: Load,
    pub total_fleet_capacity: Load,
    pub overall_load: LoadUtilization,
    pub routes: Vec<RouteKpiRow>,
    pub categories: Vec<CategoryKpiRow>,
    pub unassigned_stores: Vec<UnassignedStoreRow>,
    pub history: Vec<RunComparisonPoint>,
}

/// Build the full analytics [`KpisReport`] from a fetched [`KpisBundle`] and cached
/// recent solutions list.
pub(crate) fn build_kpis_report(
    bundle: &KpisBundle,
    recent_solutions: &[SolutionSummary],
) -> KpisReport {
    let kpis = &bundle.kpis;
    let problem = &bundle.detail.problem;
    let solution = &bundle.detail.solution;

    let mut routes = Vec::with_capacity(solution.routes.len());
    let mut total_cost = CostBreakdown::default();
    let mut total_service_min = 0_i64;
    let mut total_duration_min = 0_i64;
    let mut total_delivered = Load::ZERO;
    let mut active_capacity = Load::ZERO;

    for (index, route) in solution.routes.iter().enumerate() {
        let row = build_route_row(problem, route, index);
        total_cost += row.cost;
        total_service_min += row.service_min;
        total_duration_min += row.duration_min;
        total_delivered = total_delivered.saturating_add(row.load.delivered);
        active_capacity = active_capacity.saturating_add(row.load.capacity);
        routes.push(row);
    }

    // Reconcile solution total distance/cost from KPIs / Solution when routes exist,
    // falling back cleanly when a hand-crafted test bundle has synthetic route totals.
    if total_cost.total_cents == 0 && kpis.cost_cents > 0 {
        total_cost.other_cents = kpis.cost_cents;
        total_cost.total_cents = kpis.cost_cents;
    }

    let mut total_fleet_capacity = Load::ZERO;
    for vehicle in &problem.vehicles {
        total_fleet_capacity = total_fleet_capacity.saturating_add(vehicle.capacity);
    }

    let active_routes = routes.len();
    let total_vehicles = problem.vehicles.len();
    let fleet_utilization_pct = if total_vehicles > 0 {
        (active_routes as f64 / total_vehicles as f64) * 100.0
    } else {
        0.0
    };

    let total_travel_wait_min = total_duration_min.saturating_sub(total_service_min).max(0);
    let avg_distance_per_route_km = if active_routes > 0 {
        kpis.distance_km / active_routes as f64
    } else {
        0.0
    };
    let avg_duration_min = if active_routes > 0 {
        total_duration_min as f64 / active_routes as f64
    } else {
        0.0
    };
    let avg_cost_per_route_cents = if active_routes > 0 {
        kpis.cost_cents as f64 / active_routes as f64
    } else {
        0.0
    };
    let cost_per_km_cents = if kpis.distance_km > 0.001 {
        kpis.cost_cents as f64 / kpis.distance_km
    } else {
        0.0
    };
    let cost_per_stop_cents = if kpis.served > 0 {
        kpis.cost_cents as f64 / kpis.served as f64
    } else {
        0.0
    };

    let unassigned_stores = collect_unassigned_stores(problem, &solution.unassigned);
    let unassigned_demand = unassigned_stores
        .iter()
        .fold(Load::ZERO, |acc, s| acc.saturating_add(s.demand));
    let categories = build_category_breakdown(problem, &solution.unassigned);
    let history = build_history_points(bundle, recent_solutions);

    KpisReport {
        solution_id: bundle.detail.id.clone(),
        problem_id: bundle.detail.problem_id.clone(),
        problem_name: problem.name.clone(),
        served: kpis.served,
        unassigned: kpis.total.saturating_sub(kpis.served),
        total: kpis.total,
        serve_rate: kpis.serve_rate,
        active_routes,
        total_vehicles,
        fleet_utilization_pct,
        total_distance_km: kpis.distance_km,
        avg_distance_per_route_km,
        total_duration_min,
        total_service_min,
        total_travel_wait_min,
        avg_duration_min,
        cost: total_cost,
        avg_cost_per_route_cents,
        cost_per_km_cents,
        cost_per_stop_cents,
        total_delivered,
        unassigned_demand,
        active_capacity,
        total_fleet_capacity,
        overall_load: LoadUtilization::from_loads(total_delivered, active_capacity),
        routes,
        categories,
        unassigned_stores,
        history,
    }
}

fn build_route_row(problem: &Problem, route: &RoutePlan, index: usize) -> RouteKpiRow {
    let vehicle = problem.vehicle(route.vehicle);
    let depot_name = vehicle
        .and_then(|v| problem.depot(v.depot))
        .map_or_else(|| "?".to_string(), |d| d.name.clone());
    let capacity = vehicle.map_or(Load::ZERO, |v| v.capacity);
    let max_duration_min = vehicle.map_or(0, |v| v.max_duration.minutes());
    let duration_min = route.duration.minutes();
    let duration_pct = if max_duration_min > 0 {
        (duration_min as f64 / max_duration_min as f64) * 100.0
    } else {
        0.0
    };

    let mut delivered = Load::ZERO;
    let mut service_min = 0_i64;
    for step in &route.steps {
        if let Node::Store(store_id) = step.node
            && let Some(store) = problem.store(store_id)
        {
            delivered = delivered.saturating_add(store.demand);
            let step_svc = (step.depart.minutes() - step.arrive.minutes())
                .max(store.service.minutes())
                .max(0);
            service_min += step_svc;
        }
    }
    service_min = service_min.min(duration_min.max(0));
    let travel_and_wait_min = duration_min.saturating_sub(service_min).max(0);

    let start_min = route.steps.first().map(|s| s.depart.minutes());
    let end_min = route.steps.last().map(|s| s.depart.minutes());

    let stops = store_stop_count(route);
    let cost = compute_cost_breakdown(
        problem.cost,
        route.distance_km,
        duration_min,
        route.cost_cents,
    );
    let cost_per_km_cents = if route.distance_km > 0.001 {
        route.cost_cents as f64 / route.distance_km
    } else {
        0.0
    };
    let cost_per_stop_cents = if stops > 0 {
        route.cost_cents as f64 / stops as f64
    } else {
        0.0
    };

    RouteKpiRow {
        route_index: index,
        route_label: format!("R{}", index + 1),
        vehicle_name: vehicle_name(problem, route).to_string(),
        depot_name,
        driver_names: driver_names(problem, route),
        stops,
        distance_km: route.distance_km,
        duration_min,
        service_min,
        travel_and_wait_min,
        max_duration_min,
        duration_pct,
        start_min,
        end_min,
        cost,
        cost_per_km_cents,
        cost_per_stop_cents,
        load: LoadUtilization::from_loads(delivered, capacity),
    }
}

fn collect_unassigned_stores(
    problem: &Problem,
    unassigned_ids: &[domain::StoreId],
) -> Vec<UnassignedStoreRow> {
    unassigned_ids
        .iter()
        .map(|&store_id| match problem.store(store_id) {
            Some(store) => {
                let effective = problem.chain_rules.windows_for_store(store);
                let windows = match effective {
                    Some(wins) if !wins.is_empty() => wins
                        .iter()
                        .map(|w| format!("{}-{}", w.start, w.end))
                        .collect::<Vec<_>>()
                        .join(", "),
                    _ => "Anytime".to_string(),
                };
                UnassignedStoreRow {
                    store_id: store_id.0,
                    name: store.name.clone(),
                    category: store.chain.to_string(),
                    demand: store.demand,
                    service_min: store.service.minutes(),
                    windows,
                }
            }
            None => UnassignedStoreRow {
                store_id: store_id.0,
                name: format!("Store #{}", store_id.0),
                category: "Unknown".to_string(),
                demand: Load::ZERO,
                service_min: 0,
                windows: "-".to_string(),
            },
        })
        .collect()
}

fn chain_sort_key(chain: Chain) -> u8 {
    match chain {
        Chain::General => 0,
        Chain::Coles => 1,
        Chain::Woolworths => 2,
        Chain::Aldi => 3,
        Chain::Iga => 4,
    }
}

fn build_category_breakdown(
    problem: &Problem,
    unassigned: &[domain::StoreId],
) -> Vec<CategoryKpiRow> {
    let mut map: BTreeMap<u8, CategoryKpiRow> = BTreeMap::new();
    for store in &problem.stores {
        let is_unassigned = unassigned.contains(&store.id);
        let key = chain_sort_key(store.chain);
        let entry = map.entry(key).or_insert_with(|| CategoryKpiRow {
            category: store.chain.to_string(),
            total_stores: 0,
            served_stores: 0,
            unassigned_stores: 0,
            served_demand: Load::ZERO,
            unassigned_demand: Load::ZERO,
        });
        entry.total_stores += 1;
        if is_unassigned {
            entry.unassigned_stores += 1;
            entry.unassigned_demand = entry.unassigned_demand.saturating_add(store.demand);
        } else {
            entry.served_stores += 1;
            entry.served_demand = entry.served_demand.saturating_add(store.demand);
        }
    }
    map.into_values().collect()
}

fn build_history_points(
    bundle: &KpisBundle,
    recent_solutions: &[SolutionSummary],
) -> Vec<RunComparisonPoint> {
    // Filter runs targeting the same problem, keeping chronological oldest->newest order.
    let mut matching: Vec<&SolutionSummary> = recent_solutions
        .iter()
        .filter(|s| s.problem_id == bundle.detail.problem_id)
        .collect();
    matching.sort_by(|a, b| a.solved_at.cmp(&b.solved_at).then_with(|| a.id.cmp(&b.id)));

    let mut points: Vec<RunComparisonPoint> = matching
        .into_iter()
        .enumerate()
        .map(|(idx, s)| {
            let serve_rate_pct = if s.total > 0 {
                (s.served as f64 / s.total as f64) * 100.0
            } else {
                0.0
            };
            RunComparisonPoint {
                run_index: idx + 1,
                solution_id: s.id.clone(),
                solver_tag: if s.used_lns { "LNS" } else { "Greedy" }.to_string(),
                solved_at: s.solved_at.clone(),
                used_lns: s.used_lns,
                served: s.served,
                total: s.total,
                serve_rate_pct,
                distance_km: s.distance_km,
                cost_cents: s.cost_cents,
                is_current: s.id == bundle.detail.id,
            }
        })
        .collect();

    // Ensure the currently viewed solution is represented even if recent_solutions
    // was empty or capped below this older solution.
    if !points.iter().any(|p| p.is_current) {
        let total = bundle.kpis.total as i64;
        let served = bundle.kpis.served as i64;
        points.push(RunComparisonPoint {
            run_index: points.len() + 1,
            solution_id: bundle.detail.id.clone(),
            solver_tag: "Current".to_string(),
            solved_at: "current".to_string(),
            used_lns: false,
            served,
            total,
            serve_rate_pct: bundle.kpis.serve_rate * 100.0,
            distance_km: bundle.kpis.distance_km,
            cost_cents: bundle.kpis.cost_cents,
            is_current: true,
        });
    }

    points
}

/// Format a positive/negative minute duration compactly (`2h 15m` or `45m`).
pub(crate) fn format_duration_hm(minutes: i64) -> String {
    let abs = minutes.unsigned_abs();
    let hours = abs / 60;
    let mins = abs % 60;
    let sign = if minutes < 0 { "-" } else { "" };
    if hours > 0 {
        format!("{sign}{hours}h {mins:02}m")
    } else {
        format!("{sign}{mins}m")
    }
}

/// Format horizon minutes using domain's `HH:MM` / `Nd HH:MM` convention.
pub(crate) fn format_horizon_time(minutes: i64) -> String {
    Minutes::from_minutes(minutes).to_string()
}

/// Format a crisp multi-line KPI summary suitable for clipboard sharing.
pub(crate) fn format_kpis_summary(report: &KpisReport) -> String {
    let mut out = String::new();
    let svc_pct = if report.total_duration_min > 0 {
        (report.total_service_min as f64 / report.total_duration_min as f64) * 100.0
    } else {
        0.0
    };
    let drive_pct = if report.total_duration_min > 0 {
        100.0 - svc_pct
    } else {
        0.0
    };

    out.push_str(&format!("KPI Summary — {}\n", report.problem_name));
    out.push_str(&format!(
        "Solution: {} (Problem: {})\n",
        report.solution_id, report.problem_id
    ));
    out.push_str(&format!(
        "Served: {}/{} stops ({:.1}% serve rate, {} unassigned)\n",
        report.served,
        report.total,
        report.serve_rate * 100.0,
        report.unassigned
    ));
    out.push_str(&format!(
        "Fleet: {}/{} active routes ({:.1}% fleet util)\n",
        report.active_routes, report.total_vehicles, report.fleet_utilization_pct
    ));
    out.push_str(&format!(
        "Distance: {:.1} km (avg {:.1} km/route)\n",
        report.total_distance_km, report.avg_distance_per_route_km
    ));
    out.push_str(&format!(
        "Duration: {} total (avg {} /route · {:.0}% service / {:.0}% drive+wait)\n",
        format_duration_hm(report.total_duration_min),
        format_duration_hm(report.avg_duration_min.round() as i64),
        svc_pct,
        drive_pct
    ));
    out.push_str(&format!(
        "Cost: ${:.2} total (${:.2}/km · ${:.2}/stop | Fixed ${:.2}, Dist ${:.2}, Time/Other ${:.2})\n",
        report.cost.total_cents as f64 / 100.0,
        report.cost_per_km_cents / 100.0,
        report.cost_per_stop_cents / 100.0,
        report.cost.fixed_cents as f64 / 100.0,
        report.cost.distance_cents as f64 / 100.0,
        (report.cost.time_cents + report.cost.other_cents) as f64 / 100.0,
    ));
    out.push_str(&format!(
        "Load Delivered: Amb {} / Chl {} / Frz {} (Overall {:.1}%, Unassigned Amb {} / Chl {} / Frz {})\n",
        report.total_delivered.ambient,
        report.total_delivered.chilled,
        report.total_delivered.frozen,
        report.overall_load.total_pct,
        report.unassigned_demand.ambient,
        report.unassigned_demand.chilled,
        report.unassigned_demand.frozen
    ));

    if !report.routes.is_empty() {
        out.push_str("\nPer-Route Breakdown:\n");
        for r in &report.routes {
            out.push_str(&format!(
                "- {} | {} | {} | {} stops | {:.1} km | {} (svc {}) | Load {:.0}% ({}) | ${:.2} (${:.2}/km)\n",
                r.route_label,
                r.vehicle_name,
                r.driver_names,
                r.stops,
                r.distance_km,
                format_duration_hm(r.duration_min),
                format_duration_hm(r.service_min),
                r.load.peak_pct,
                r.load.bottleneck,
                r.cost.total_cents as f64 / 100.0,
                r.cost_per_km_cents / 100.0
            ));
        }
    }

    out
}

fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Serialize per-route and fleet-total KPIs as RFC-4180 CSV.
pub(crate) fn format_kpis_csv(report: &KpisReport) -> String {
    let mut out = String::from(
        "route,vehicle,depot,drivers,stops,distance_km,duration_min,service_min,travel_wait_min,max_duration_min,duration_pct,delivered_ambient,delivered_chilled,delivered_frozen,cap_ambient,cap_chilled,cap_frozen,peak_load_pct,bottleneck_compartment,cost_fixed_dollars,cost_distance_dollars,cost_time_other_dollars,cost_total_dollars,cost_per_km_dollars,cost_per_stop_dollars\n",
    );

    for r in &report.routes {
        let time_other_cents = r.cost.time_cents + r.cost.other_cents;
        out.push_str(&format!(
            "{},{},{},{},{},{:.2},{},{},{},{},{:.1},{},{},{},{},{},{},{:.1},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}\n",
            csv_escape(&r.route_label),
            csv_escape(&r.vehicle_name),
            csv_escape(&r.depot_name),
            csv_escape(&r.driver_names),
            r.stops,
            r.distance_km,
            r.duration_min,
            r.service_min,
            r.travel_and_wait_min,
            r.max_duration_min,
            r.duration_pct,
            r.load.delivered.ambient,
            r.load.delivered.chilled,
            r.load.delivered.frozen,
            r.load.capacity.ambient,
            r.load.capacity.chilled,
            r.load.capacity.frozen,
            r.load.peak_pct,
            csv_escape(r.load.bottleneck),
            r.cost.fixed_cents as f64 / 100.0,
            r.cost.distance_cents as f64 / 100.0,
            time_other_cents as f64 / 100.0,
            r.cost.total_cents as f64 / 100.0,
            r.cost_per_km_cents / 100.0,
            r.cost_per_stop_cents / 100.0,
        ));
    }

    let total_time_other = report.cost.time_cents + report.cost.other_cents;
    out.push_str(&format!(
        "TOTAL,{},\"\",{},{},{:.2},{},{},{},0,0.0,{},{},{},{},{},{},{:.1},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}\n",
        csv_escape(&format!("{}/{} vehicles", report.active_routes, report.total_vehicles)),
        csv_escape(&format!("{}/{} stops", report.served, report.total)),
        report.served,
        report.total_distance_km,
        report.total_duration_min,
        report.total_service_min,
        report.total_travel_wait_min,
        report.total_delivered.ambient,
        report.total_delivered.chilled,
        report.total_delivered.frozen,
        report.active_capacity.ambient,
        report.active_capacity.chilled,
        report.active_capacity.frozen,
        report.overall_load.peak_pct,
        csv_escape(report.overall_load.bottleneck),
        report.cost.fixed_cents as f64 / 100.0,
        report.cost.distance_cents as f64 / 100.0,
        total_time_other as f64 / 100.0,
        report.cost.total_cents as f64 / 100.0,
        report.cost_per_km_cents / 100.0,
        report.cost_per_stop_cents / 100.0,
    ));

    out
}

/// Produce integer-aligned grid marks (`0..count`) for bar/lane axes so ticks
/// never land on fractional route indices like `R1.5`.
pub(crate) fn discrete_axis_marks(count: usize, input: GridInput) -> Vec<GridMark> {
    if count == 0 {
        return Vec::new();
    }
    let lo = (input.bounds.0.floor() as isize).max(0) as usize;
    let hi = (input.bounds.1.ceil() as isize).min(count.saturating_sub(1) as isize);
    if hi < lo as isize {
        return Vec::new();
    }
    let hi = hi as usize;
    let visible = hi - lo + 1;
    let step = if visible <= 18 {
        1
    } else if visible <= 36 {
        2
    } else {
        (visible / 16).max(1)
    };
    (lo..=hi)
        .filter(|i| (i - lo).is_multiple_of(step))
        .map(|i| GridMark {
            value: i as f64,
            step_size: step as f64,
        })
        .collect()
}

fn route_metric_sort_key(row: &RouteKpiRow, metric: KpiMetric) -> f64 {
    match metric {
        KpiMetric::CostBreakdown => row.cost.total_cents as f64,
        KpiMetric::Distance => row.distance_km,
        KpiMetric::Duration => row.duration_min as f64,
        KpiMetric::Stops => row.stops as f64,
        KpiMetric::Utilization => row.load.peak_pct,
    }
}

pub(crate) fn ordered_route_rows(
    routes: &[RouteKpiRow],
    metric: KpiMetric,
    sort: KpiChartSort,
) -> Vec<&RouteKpiRow> {
    let mut rows: Vec<&RouteKpiRow> = routes.iter().collect();
    if sort == KpiChartSort::HighestFirst {
        rows.sort_by(|a, b| {
            route_metric_sort_key(b, metric)
                .total_cmp(&route_metric_sort_key(a, metric))
                .then_with(|| a.route_index.cmp(&b.route_index))
        });
    }
    rows
}

fn sort_table_rows(
    routes: &[RouteKpiRow],
    col: KpiTableColumn,
    descending: bool,
) -> Vec<&RouteKpiRow> {
    let mut rows: Vec<&RouteKpiRow> = routes.iter().collect();
    rows.sort_by(|a, b| {
        let ord = match col {
            KpiTableColumn::Route => a.route_index.cmp(&b.route_index),
            KpiTableColumn::Vehicle => a.vehicle_name.cmp(&b.vehicle_name),
            KpiTableColumn::Stops => a.stops.cmp(&b.stops),
            KpiTableColumn::Distance => a.distance_km.total_cmp(&b.distance_km),
            KpiTableColumn::Duration => a.duration_min.cmp(&b.duration_min),
            KpiTableColumn::PeakLoad => a.load.peak_pct.total_cmp(&b.load.peak_pct),
            KpiTableColumn::Cost => a.cost.total_cents.cmp(&b.cost.total_cents),
            KpiTableColumn::CostPerKm => a.cost_per_km_cents.total_cmp(&b.cost_per_km_cents),
        }
        .then_with(|| a.route_index.cmp(&b.route_index));
        if descending { ord.reverse() } else { ord }
    });
    rows
}

fn serve_rate_color(serve_rate: f64) -> egui::Color32 {
    if serve_rate >= 0.999 {
        egui::Color32::from_rgb(44, 160, 72)
    } else if serve_rate >= 0.8 {
        egui::Color32::from_rgb(218, 145, 25)
    } else {
        egui::Color32::from_rgb(214, 55, 55)
    }
}

fn utilization_color(pct: f64) -> egui::Color32 {
    if pct > 95.0 {
        egui::Color32::from_rgb(214, 55, 55)
    } else if pct >= 75.0 {
        egui::Color32::from_rgb(218, 145, 25)
    } else {
        egui::Color32::from_rgb(44, 160, 72)
    }
}

fn metric_card_with_sub(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    sub: &str,
    accent: Option<egui::Color32>,
) {
    egui::Frame::default()
        .inner_margin(egui::Margin::same(8))
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(label).small().strong());
                let val_text = egui::RichText::new(value).strong().monospace().size(15.0);
                ui.label(match accent {
                    Some(color) => val_text.color(color),
                    None => val_text,
                });
                if !sub.is_empty() {
                    ui.label(egui::RichText::new(sub).small().weak());
                }
            });
        });
}

/// Render the upgraded KPIs analytics dashboard inside a vertical scroll area.
pub(crate) fn render_kpis_dashboard(
    ui: &mut egui::Ui,
    bundle: &KpisBundle,
    recent_solutions: &[SolutionSummary],
    state: &mut KpisUiState,
) -> Option<KpisAction> {
    let report = build_kpis_report(bundle, recent_solutions);
    let mut action = None;

    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("kpis_dashboard_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Action bar
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("Problem: {}", report.problem_name));
                ui.separator();
                copy_button(ui, "Copy solution ID", &report.solution_id);
                if ui.button("Copy Summary").clicked() {
                    ui.ctx().copy_text(format_kpis_summary(&report));
                    state.action_status = Some((false, "Copied KPI summary to clipboard".into()));
                }
                if ui.button("Export CSV…").clicked() {
                    let csv = format_kpis_csv(&report);
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("kpis-{}.csv", report.solution_id))
                        .add_filter("CSV", &["csv"])
                        .save_file()
                    {
                        state.action_status = Some(match std::fs::write(&path, csv) {
                            Ok(()) => (false, format!("Exported CSV to {}", path.display())),
                            Err(e) => (true, format!("CSV export failed: {e}")),
                        });
                    }
                }
                if ui.button("Export JSON…").clicked()
                    && let Ok(json) = serde_json::to_string_pretty(&report)
                    && let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("kpis-{}.json", report.solution_id))
                        .add_filter("JSON", &["json"])
                        .save_file()
                {
                    state.action_status = Some(match std::fs::write(&path, json) {
                        Ok(()) => (false, format!("Exported JSON to {}", path.display())),
                        Err(e) => (true, format!("JSON export failed: {e}")),
                    });
                }
                ui.separator();
                if ui.button("View on Map").clicked() {
                    action = Some(KpisAction::OpenMap {
                        solution_id: report.solution_id.clone(),
                    });
                }
                if ui.button("View Solution").clicked() {
                    action = Some(KpisAction::OpenSolution {
                        solution_id: report.solution_id.clone(),
                        route_index: state.selected_route,
                    });
                }
            });

            if let Some((is_err, msg)) = &state.action_status {
                let color = if *is_err {
                    egui::Color32::RED
                } else {
                    egui::Color32::from_rgb(44, 160, 72)
                };
                ui.colored_label(color, msg);
            }

            ui.add_space(6.0);

            // Executive KPI summary cards
            render_executive_cards(ui, &report);

            ui.add_space(10.0);

            // Primary per-route interactive chart
            render_per_route_chart(ui, &report, state);

            ui.add_space(12.0);

            // Secondary side-by-side analytics charts
            ui.columns(2, |cols| {
                render_dispatch_gantt_chart(&mut cols[0], &report, state.selected_route);
                render_cost_and_load_breakdown(&mut cols[1], &report);
            });

            ui.add_space(12.0);

            // Run comparison chart (Greedy vs LNS / multi-run improvement)
            render_history_comparison(ui, &report, state);

            ui.add_space(12.0);

            // Sortable Per-Route Performance Leaderboard
            if let Some(route_action) = render_routes_leaderboard(ui, &report, state) {
                action = Some(route_action);
            }

            // Unassigned Stops Impact (when any places were unserved)
            if !report.unassigned_stores.is_empty() {
                ui.add_space(12.0);
                render_unassigned_panel(ui, &report);
            }
        });

    action
}

fn render_executive_cards(ui: &mut egui::Ui, report: &KpisReport) {
    let serve_color = serve_rate_color(report.serve_rate);
    let svc_pct = if report.total_duration_min > 0 {
        (report.total_service_min as f64 / report.total_duration_min as f64) * 100.0
    } else {
        0.0
    };

    ui.horizontal_wrapped(|ui| {
        metric_card_with_sub(
            ui,
            "Serve Rate",
            &format!("{:.1}%", report.serve_rate * 100.0),
            &format!(
                "{}/{} served · {} unserved",
                report.served, report.total, report.unassigned
            ),
            Some(serve_color),
        );
        metric_card_with_sub(
            ui,
            "Total Cost",
            &format!("${:.2}", report.cost.total_cents as f64 / 100.0),
            &format!(
                "${:.2}/km · ${:.2}/stop",
                report.cost_per_km_cents / 100.0,
                report.cost_per_stop_cents / 100.0
            ),
            None,
        );
        metric_card_with_sub(
            ui,
            "Total Distance",
            &format!("{:.1} km", report.total_distance_km),
            &format!("avg {:.1} km / route", report.avg_distance_per_route_km),
            None,
        );
        metric_card_with_sub(
            ui,
            "Active Fleet",
            &format!("{}/{} routes", report.active_routes, report.total_vehicles),
            &format!("{:.0}% fleet used", report.fleet_utilization_pct),
            None,
        );
        metric_card_with_sub(
            ui,
            "Total Duration",
            &format_duration_hm(report.total_duration_min),
            &format!(
                "avg {} · {:.0}% service",
                format_duration_hm(report.avg_duration_min.round() as i64),
                svc_pct
            ),
            None,
        );
        metric_card_with_sub(
            ui,
            "Load Utilization",
            &format!("{:.1}%", report.overall_load.peak_pct),
            &format!(
                "peak {} · overall {:.1}%",
                report.overall_load.bottleneck, report.overall_load.total_pct
            ),
            Some(utilization_color(report.overall_load.peak_pct)),
        );
    });

    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::ProgressBar::new(report.serve_rate.clamp(0.0, 1.0) as f32)
                .desired_width(360.0)
                .fill(serve_color)
                .text(format!(
                    "{}/{} places served ({:.1}%)",
                    report.served,
                    report.total,
                    report.serve_rate * 100.0
                )),
        );
        ui.weak(format!(
            "Delivered units — Amb: {}, Chl: {}, Frz: {}",
            report.total_delivered.ambient,
            report.total_delivered.chilled,
            report.total_delivered.frozen
        ));
    });
}

fn render_per_route_chart(ui: &mut egui::Ui, report: &KpisReport, state: &mut KpisUiState) {
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Per-Route Analytics:");
            for metric in [
                KpiMetric::CostBreakdown,
                KpiMetric::Distance,
                KpiMetric::Duration,
                KpiMetric::Stops,
                KpiMetric::Utilization,
            ] {
                ui.selectable_value(&mut state.metric, metric, metric.label());
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Order:");
            ui.selectable_value(&mut state.chart_sort, KpiChartSort::RouteOrder, "R1..Rn");
            ui.selectable_value(
                &mut state.chart_sort,
                KpiChartSort::HighestFirst,
                "Highest first",
            );
            ui.separator();
            ui.checkbox(&mut state.show_avg_line, "Average line");
            ui.checkbox(&mut state.show_limit_lines, "Capacity / limit lines");
        });

        let ordered = ordered_route_rows(&report.routes, state.metric, state.chart_sort);
        if ordered.is_empty() {
            ui.weak("No active routes in this solution.");
            return;
        }

        let num_routes = ordered.len();
        let x_labels: Vec<String> = ordered.iter().map(|r| r.route_label.clone()).collect();
        let tooltips: Vec<String> = ordered
            .iter()
            .map(|r| {
                format!(
                    "{} · {} ({})\nStops: {} | {:.1} km | {} (svc {})\nCost: ${:.2} (Fixed ${:.2}, Dist ${:.2}, Time/Other ${:.2})\nLoad: A {} / C {} / F {} (Peak {:.0}% {})",
                    r.route_label,
                    r.vehicle_name,
                    r.driver_names,
                    r.stops,
                    r.distance_km,
                    format_duration_hm(r.duration_min),
                    format_duration_hm(r.service_min),
                    r.cost.total_cents as f64 / 100.0,
                    r.cost.fixed_cents as f64 / 100.0,
                    r.cost.distance_cents as f64 / 100.0,
                    (r.cost.time_cents + r.cost.other_cents) as f64 / 100.0,
                    r.load.delivered.ambient,
                    r.load.delivered.chilled,
                    r.load.delivered.frozen,
                    r.load.peak_pct,
                    r.load.bottleneck,
                )
            })
            .collect();

        let metric = state.metric;
        let show_avg = state.show_avg_line;
        let show_limit = state.show_limit_lines;
        let selected_route = state.selected_route;

        let plot = Plot::new((
            "kpis_per_route_plot",
            report.solution_id.clone(),
            metric,
            state.chart_sort,
        ))
        .height(250.0)
        .include_x(-0.6)
        .include_x(num_routes as f64 - 0.4)
        .include_y(0.0)
        .x_axis_label("Route")
        .y_axis_label(metric.y_axis_label())
        .x_grid_spacer(move |input| discrete_axis_marks(num_routes, input))
        .x_axis_formatter(move |mark, _| {
            let idx = mark.value.round() as isize;
            if (mark.value - idx as f64).abs() < 0.05 && idx >= 0 {
                x_labels.get(idx as usize).cloned().unwrap_or_default()
            } else {
                String::new()
            }
        })
        .y_axis_formatter(move |mark, _| match metric {
            KpiMetric::CostBreakdown => format!("${:.0}", mark.value),
            KpiMetric::Distance => format!("{:.0} km", mark.value),
            KpiMetric::Duration => format!("{:.1}h", mark.value),
            KpiMetric::Stops => format!("{:.0}", mark.value),
            KpiMetric::Utilization => format!("{:.0}%", mark.value),
        })
        .legend(Legend::default().position(Corner::RightTop));

        plot.show(ui, |plot_ui| match metric {
            KpiMetric::CostBreakdown => {
                plot_cost_breakdown_bars(plot_ui, &ordered, &tooltips, show_avg);
            }
            KpiMetric::Distance => {
                plot_single_metric_bars(
                    plot_ui,
                    "Distance (km)",
                    &ordered,
                    &tooltips,
                    selected_route,
                    |r| r.distance_km,
                    show_avg.then_some(("Avg distance", report.avg_distance_per_route_km)),
                );
            }
            KpiMetric::Duration => {
                plot_duration_bars(plot_ui, &ordered, &tooltips, report, show_avg, show_limit);
            }
            KpiMetric::Stops => {
                let avg_stops = report.served as f64 / num_routes.max(1) as f64;
                plot_single_metric_bars(
                    plot_ui,
                    "Stops",
                    &ordered,
                    &tooltips,
                    selected_route,
                    |r| r.stops as f64,
                    show_avg.then_some(("Avg stops", avg_stops)),
                );
            }
            KpiMetric::Utilization => {
                plot_utilization_bars(plot_ui, &ordered, &tooltips, report, show_avg, show_limit);
            }
        });
    });
}

fn bar_tooltip(tooltips: &[String], bar: &Bar) -> String {
    let idx = bar.argument.round() as isize;
    if idx >= 0
        && let Some(tip) = tooltips.get(idx as usize)
    {
        return tip.clone();
    }
    format!("{}: {:.1}", bar.name, bar.value)
}

fn plot_cost_breakdown_bars(
    plot_ui: &mut egui_plot::PlotUi<'_>,
    ordered: &[&RouteKpiRow],
    tooltips: &[String],
    show_avg: bool,
) {
    let fixed_bars: Vec<Bar> = ordered
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Bar::new(i as f64, r.cost.fixed_cents as f64 / 100.0)
                .width(0.62)
                .name(r.route_label.clone())
        })
        .collect();
    let dist_bars: Vec<Bar> = ordered
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Bar::new(i as f64, r.cost.distance_cents as f64 / 100.0)
                .width(0.62)
                .name(r.route_label.clone())
        })
        .collect();
    let time_other_bars: Vec<Bar> = ordered
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let cents = r.cost.time_cents + r.cost.other_cents;
            Bar::new(i as f64, cents as f64 / 100.0)
                .width(0.62)
                .name(r.route_label.clone())
        })
        .collect();

    let tips_a = tooltips.to_vec();
    let tips_b = tooltips.to_vec();
    let tips_c = tooltips.to_vec();

    let fixed_chart = BarChart::new("Fixed ($)", fixed_bars)
        .vertical()
        .color(egui::Color32::from_rgb(78, 121, 167))
        .element_formatter(Box::new(move |bar, _| bar_tooltip(&tips_a, bar)));
    let dist_chart = BarChart::new("Distance ($)", dist_bars)
        .vertical()
        .color(egui::Color32::from_rgb(89, 161, 79))
        .stack_on(&[&fixed_chart])
        .element_formatter(Box::new(move |bar, _| bar_tooltip(&tips_b, bar)));
    let time_chart = BarChart::new("Time & Other ($)", time_other_bars)
        .vertical()
        .color(egui::Color32::from_rgb(237, 201, 72))
        .stack_on(&[&fixed_chart, &dist_chart])
        .element_formatter(Box::new(move |bar, _| bar_tooltip(&tips_c, bar)));

    plot_ui.bar_chart(fixed_chart);
    plot_ui.bar_chart(dist_chart);
    plot_ui.bar_chart(time_chart);

    if show_avg && !ordered.is_empty() {
        let avg_dollars = ordered
            .iter()
            .map(|r| r.cost.total_cents as f64 / 100.0)
            .sum::<f64>()
            / ordered.len() as f64;
        plot_ui.hline(
            HLine::new(format!("Avg ${avg_dollars:.2}"), avg_dollars)
                .color(egui::Color32::from_rgb(255, 175, 64))
                .style(LineStyle::dashed_loose())
                .width(1.6),
        );
    }
}

fn plot_single_metric_bars(
    plot_ui: &mut egui_plot::PlotUi<'_>,
    series_name: &str,
    ordered: &[&RouteKpiRow],
    tooltips: &[String],
    selected_route: Option<usize>,
    value_fn: impl Fn(&RouteKpiRow) -> f64,
    avg_line: Option<(&str, f64)>,
) {
    let bars: Vec<Bar> = ordered
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let color = route_color(r.route_index);
            let is_selected = selected_route == Some(r.route_index);
            let fill = if is_selected {
                color.linear_multiply(0.85)
            } else {
                color.linear_multiply(0.55)
            };
            let stroke = if is_selected {
                egui::Stroke::new(2.5, egui::Color32::WHITE)
            } else {
                egui::Stroke::new(1.5, color)
            };
            Bar::new(i as f64, value_fn(r))
                .width(0.62)
                .fill(fill)
                .stroke(stroke)
                .name(r.route_label.clone())
        })
        .collect();

    let tips = tooltips.to_vec();
    let chart = BarChart::new(series_name, bars)
        .vertical()
        .element_formatter(Box::new(move |bar, _| bar_tooltip(&tips, bar)));
    plot_ui.bar_chart(chart);

    if let Some((label, avg)) = avg_line {
        plot_ui.hline(
            HLine::new(format!("{label}: {avg:.1}"), avg)
                .color(egui::Color32::from_rgb(255, 175, 64))
                .style(LineStyle::dashed_loose())
                .width(1.6),
        );
    }
}

fn plot_duration_bars(
    plot_ui: &mut egui_plot::PlotUi<'_>,
    ordered: &[&RouteKpiRow],
    tooltips: &[String],
    report: &KpisReport,
    show_avg: bool,
    show_limit: bool,
) {
    let svc_bars: Vec<Bar> = ordered
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Bar::new(i as f64, r.service_min as f64 / 60.0)
                .width(0.62)
                .name(r.route_label.clone())
        })
        .collect();
    let travel_bars: Vec<Bar> = ordered
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Bar::new(i as f64, r.travel_and_wait_min as f64 / 60.0)
                .width(0.62)
                .name(r.route_label.clone())
        })
        .collect();

    let tips_a = tooltips.to_vec();
    let tips_b = tooltips.to_vec();

    let svc_chart = BarChart::new("Service / unload (h)", svc_bars)
        .vertical()
        .color(egui::Color32::from_rgb(242, 142, 43))
        .element_formatter(Box::new(move |bar, _| bar_tooltip(&tips_a, bar)));
    let travel_chart = BarChart::new("Drive & wait (h)", travel_bars)
        .vertical()
        .color(egui::Color32::from_rgb(78, 121, 167))
        .stack_on(&[&svc_chart])
        .element_formatter(Box::new(move |bar, _| bar_tooltip(&tips_b, bar)));

    plot_ui.bar_chart(svc_chart);
    plot_ui.bar_chart(travel_chart);

    if show_avg {
        let avg_h = report.avg_duration_min / 60.0;
        plot_ui.hline(
            HLine::new(format!("Avg {avg_h:.1}h"), avg_h)
                .color(egui::Color32::from_rgb(255, 175, 64))
                .style(LineStyle::dashed_loose())
                .width(1.6),
        );
    }

    if show_limit
        && let Some(max_limit_h) = ordered
            .iter()
            .filter(|r| r.max_duration_min > 0)
            .map(|r| r.max_duration_min as f64 / 60.0)
            .reduce(f64::max)
    {
        plot_ui.hline(
            HLine::new(format!("Max vehicle limit {max_limit_h:.1}h"), max_limit_h)
                .color(egui::Color32::from_rgb(214, 55, 55))
                .style(LineStyle::dotted_dense())
                .width(1.6),
        );
    }
}

struct CompartmentPlotSpec {
    name: &'static str,
    color: egui::Color32,
    pct: fn(&RouteKpiRow) -> f64,
}

fn plot_utilization_bars(
    plot_ui: &mut egui_plot::PlotUi<'_>,
    ordered: &[&RouteKpiRow],
    tooltips: &[String],
    report: &KpisReport,
    show_avg: bool,
    show_limit: bool,
) {
    // Only render bars for compartments with non-zero fleet capacity so plots
    // stay focused for ambient-only general routes.
    let mut specs: Vec<CompartmentPlotSpec> = Vec::new();
    if report.active_capacity.ambient > 0 {
        specs.push(CompartmentPlotSpec {
            name: "Ambient %",
            color: egui::Color32::from_rgb(78, 121, 167),
            pct: |r| r.load.ambient_pct,
        });
    }
    if report.active_capacity.chilled > 0 {
        specs.push(CompartmentPlotSpec {
            name: "Chilled %",
            color: egui::Color32::from_rgb(89, 161, 79),
            pct: |r| r.load.chilled_pct,
        });
    }
    if report.active_capacity.frozen > 0 {
        specs.push(CompartmentPlotSpec {
            name: "Frozen %",
            color: egui::Color32::from_rgb(176, 122, 161),
            pct: |r| r.load.frozen_pct,
        });
    }

    if specs.is_empty() {
        return;
    }

    let k = specs.len();
    let group_span = 0.72;
    let width = (group_span / k as f64) * 0.85;

    for (slot, spec) in specs.into_iter().enumerate() {
        let offset = if k == 1 {
            0.0
        } else {
            let half = (k - 1) as f64 / 2.0;
            (slot as f64 - half) * (group_span / k as f64)
        };
        let bars: Vec<Bar> = ordered
            .iter()
            .enumerate()
            .map(|(i, r)| {
                Bar::new(i as f64 + offset, (spec.pct)(r))
                    .width(width)
                    .name(r.route_label.clone())
            })
            .collect();
        let tips = tooltips.to_vec();
        let chart = BarChart::new(spec.name, bars)
            .vertical()
            .color(spec.color)
            .element_formatter(Box::new(move |bar, _| bar_tooltip(&tips, bar)));
        plot_ui.bar_chart(chart);
    }

    if show_avg && !ordered.is_empty() {
        let avg_peak = ordered.iter().map(|r| r.load.peak_pct).sum::<f64>() / ordered.len() as f64;
        plot_ui.hline(
            HLine::new(format!("Avg peak {avg_peak:.0}%"), avg_peak)
                .color(egui::Color32::from_rgb(255, 175, 64))
                .style(LineStyle::dashed_loose())
                .width(1.6),
        );
    }

    if show_limit {
        plot_ui.hline(
            HLine::new("100% capacity", 100.0)
                .color(egui::Color32::from_rgb(214, 55, 55))
                .style(LineStyle::dotted_dense())
                .width(1.6),
        );
    }
}

fn render_dispatch_gantt_chart(
    ui: &mut egui::Ui,
    report: &KpisReport,
    selected_route: Option<usize>,
) {
    ui.group(|ui| {
        ui.strong("Dispatch Timeline (Horizon Clock)");
        ui.weak("Start-to-return span per active route (drag/scroll to pan or zoom)");

        let scheduled: Vec<&RouteKpiRow> = report
            .routes
            .iter()
            .filter(|r| r.start_min.is_some() && r.end_min.is_some())
            .collect();

        if scheduled.is_empty() {
            ui.add_space(16.0);
            ui.weak("Route step timestamps are not present in this solution.");
            return;
        }

        let num_lanes = scheduled.len();
        let lane_labels: Vec<String> = scheduled
            .iter()
            .rev()
            .map(|r| format!("{} · {}", r.route_label, r.vehicle_name))
            .collect();

        let min_h = scheduled
            .iter()
            .filter_map(|r| r.start_min)
            .min()
            .unwrap_or(0) as f64
            / 60.0;
        let max_h = scheduled
            .iter()
            .filter_map(|r| r.end_min)
            .max()
            .unwrap_or(60) as f64
            / 60.0;
        let span_pad = ((max_h - min_h).max(1.0)) * 0.08;

        let plot = Plot::new(("kpis_dispatch_gantt", report.solution_id.clone(), num_lanes))
            .height(225.0)
            .default_x_bounds(min_h - span_pad, max_h + span_pad)
            .include_y(-0.6)
            .include_y(num_lanes as f64 - 0.4)
            .x_axis_label("Horizon Clock")
            .y_grid_spacer(move |input| discrete_axis_marks(num_lanes, input))
            .x_axis_formatter(|mark, _| format_horizon_time((mark.value * 60.0).round() as i64))
            .y_axis_formatter(move |mark, _| {
                let idx = mark.value.round() as isize;
                if (mark.value - idx as f64).abs() < 0.05 && idx >= 0 {
                    lane_labels.get(idx as usize).cloned().unwrap_or_default()
                } else {
                    String::new()
                }
            })
            .label_formatter(|pos| match pos {
                HoverPosition::NearDataPoint {
                    plot_name,
                    position,
                    ..
                } if !plot_name.is_empty() => Some(format!(
                    "{plot_name}\nTime: {}",
                    format_horizon_time((position.x * 60.0).round() as i64)
                )),
                HoverPosition::Elsewhere { position } => Some(format!(
                    "Time: {}",
                    format_horizon_time((position.x * 60.0).round() as i64)
                )),
                _ => None,
            });

        let text_color = ui.visuals().strong_text_color();
        plot.show(ui, |plot_ui| {
            let half_h = 0.34;
            for (pos, row) in scheduled.iter().enumerate() {
                let lane_y = (num_lanes - 1 - pos) as f64;
                let s_min = row.start_min.unwrap_or(0);
                let e_min = row.end_min.unwrap_or(s_min);
                let s_h = s_min as f64 / 60.0;
                // Ensure zero-duration synthetic test routes still render visibly.
                let e_h = (e_min as f64 / 60.0).max(s_h + 0.15);
                let color = route_color(row.route_index);
                let name = format!(
                    "{} · {} ({} -> {}, {} stops)",
                    row.route_label,
                    row.vehicle_name,
                    format_horizon_time(s_min),
                    format_horizon_time(e_min),
                    row.stops
                );

                let pts = vec![
                    [s_h, lane_y - half_h],
                    [e_h, lane_y - half_h],
                    [e_h, lane_y + half_h],
                    [s_h, lane_y + half_h],
                ];
                let is_selected = selected_route == Some(row.route_index);
                plot_ui.polygon(
                    Polygon::new(name.clone(), PlotPoints::new(pts))
                        .fill_color(if is_selected {
                            color.linear_multiply(0.85)
                        } else {
                            color.linear_multiply(0.55)
                        })
                        .stroke(if is_selected {
                            egui::Stroke::new(2.5, egui::Color32::WHITE)
                        } else {
                            egui::Stroke::new(1.5, color)
                        }),
                );
                let mid_h = (s_h + e_h) * 0.5;
                plot_ui.text(
                    Text::new(
                        name,
                        PlotPoint::new(mid_h, lane_y),
                        format!("{} ({} stops)", row.route_label, row.stops),
                    )
                    .color(text_color),
                );
            }
        });
    });
}

fn render_cost_and_load_breakdown(ui: &mut egui::Ui, report: &KpisReport) {
    ui.group(|ui| {
        ui.strong("Cost Drivers & Fleet Load Composition");
        ui.weak("Reconciled fixed/distance/time cost split and compartment utilization");
        ui.add_space(6.0);

        let cost = report.cost;
        let time_other_cents = cost.time_cents + cost.other_cents;
        let cost_items = [
            (
                "Fixed route fees",
                cost.fixed_cents,
                egui::Color32::from_rgb(78, 121, 167),
            ),
            (
                "Distance charges",
                cost.distance_cents,
                egui::Color32::from_rgb(89, 161, 79),
            ),
            (
                "Time & other charges",
                time_other_cents,
                egui::Color32::from_rgb(237, 201, 72),
            ),
        ];

        for (label, cents, color) in cost_items {
            let pct = cost.pct_of(cents);
            ui.horizontal(|ui| {
                ui.add_sized([145.0, 18.0], egui::Label::new(label));
                ui.add(
                    egui::ProgressBar::new((pct as f32 / 100.0).clamp(0.0, 1.0))
                        .desired_width(150.0)
                        .fill(color)
                        .text(format!("${:.2} ({pct:.0}%)", cents as f64 / 100.0)),
                );
            });
        }

        ui.add_space(10.0);
        ui.strong("Compartment Load vs Active Route Capacity");
        let load_items = [
            (
                "Ambient",
                report.total_delivered.ambient,
                report.active_capacity.ambient,
                report.unassigned_demand.ambient,
                report.overall_load.ambient_pct,
                egui::Color32::from_rgb(78, 121, 167),
            ),
            (
                "Chilled",
                report.total_delivered.chilled,
                report.active_capacity.chilled,
                report.unassigned_demand.chilled,
                report.overall_load.chilled_pct,
                egui::Color32::from_rgb(89, 161, 79),
            ),
            (
                "Frozen",
                report.total_delivered.frozen,
                report.active_capacity.frozen,
                report.unassigned_demand.frozen,
                report.overall_load.frozen_pct,
                egui::Color32::from_rgb(176, 122, 161),
            ),
        ];

        for (name, del, cap, unserved, pct, color) in load_items {
            if cap == 0 && del == 0 && unserved == 0 {
                continue;
            }
            ui.horizontal(|ui| {
                ui.add_sized([70.0, 18.0], egui::Label::new(name));
                ui.add(
                    egui::ProgressBar::new((pct as f32 / 100.0).clamp(0.0, 1.0))
                        .desired_width(160.0)
                        .fill(color)
                        .text(format!("{del}/{cap} ({pct:.0}%)")),
                );
                if unserved > 0 {
                    ui.colored_label(
                        egui::Color32::from_rgb(218, 145, 25),
                        format!("+{unserved} unmet"),
                    );
                }
            });
        }

        if report.categories.len() > 1 {
            ui.add_space(8.0);
            ui.strong("By Place / Banner Category");
            ui.horizontal_wrapped(|ui| {
                for cat in &report.categories {
                    let rate = if cat.total_stores > 0 {
                        (cat.served_stores as f64 / cat.total_stores as f64) * 100.0
                    } else {
                        0.0
                    };
                    stat_card(
                        ui,
                        &cat.category,
                        &format!("{}/{} ({:.0}%)", cat.served_stores, cat.total_stores, rate),
                    );
                }
            });
        }
    });
}

fn render_history_comparison(ui: &mut egui::Ui, report: &KpisReport, state: &mut KpisUiState) {
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!(
                "Solve History Comparison ({} run{} for this problem)",
                report.history.len(),
                if report.history.len() == 1 { "" } else { "s" }
            ));
            ui.separator();
            ui.selectable_value(&mut state.history_metric, HistoryMetric::Cost, "Cost ($)");
            ui.selectable_value(
                &mut state.history_metric,
                HistoryMetric::Distance,
                "Distance (km)",
            );
            ui.selectable_value(
                &mut state.history_metric,
                HistoryMetric::ServeRate,
                "Serve Rate (%)",
            );
        });

        if report.history.len() < 2 {
            ui.weak(
                "Run another solve on this problem (e.g., compare Greedy vs Local Search / LNS / CH) to visualize optimization progress here.",
            );
            return;
        }

        // Baseline delta callout comparing current run vs earliest and best run.
        if let (Some(first), Some(curr)) = (
            report.history.first(),
            report.history.iter().find(|p| p.is_current),
        ) {
            let best_cost = report
                .history
                .iter()
                .map(|p| p.cost_cents)
                .min()
                .unwrap_or(curr.cost_cents);
            let cost_delta_pct = if first.cost_cents > 0 && curr.solution_id != first.solution_id {
                Some(((curr.cost_cents - first.cost_cents) as f64 / first.cost_cents as f64) * 100.0)
            } else {
                None
            };
            let dist_delta_km = curr.distance_km - first.distance_km;

            ui.horizontal_wrapped(|ui| {
                if let Some(pct) = cost_delta_pct {
                    let color = if pct <= 0.0 {
                        egui::Color32::from_rgb(44, 160, 72)
                    } else {
                        egui::Color32::from_rgb(218, 145, 25)
                    };
                    ui.colored_label(
                        color,
                        format!(
                            "Vs first run (#{} {}): {pct:+.1}% cost, {dist_delta_km:+.1} km",
                            first.run_index, first.solver_tag
                        ),
                    );
                }
                if curr.cost_cents == best_cost {
                    ui.colored_label(
                        egui::Color32::from_rgb(44, 160, 72),
                        "· Best cost achieved across recorded runs",
                    );
                } else {
                    let gap = (curr.cost_cents - best_cost) as f64 / 100.0;
                    ui.weak(format!("· Best recorded run is ${gap:.2} lower"));
                }
            });
        }

        let metric = state.history_metric;
        let val_of = |p: &RunComparisonPoint| -> f64 {
            match metric {
                HistoryMetric::Cost => p.cost_cents as f64 / 100.0,
                HistoryMetric::Distance => p.distance_km,
                HistoryMetric::ServeRate => p.serve_rate_pct,
            }
        };

        let n = report.history.len();
        let line_pts: Vec<[f64; 2]> = report
            .history
            .iter()
            .enumerate()
            .map(|(i, p)| [i as f64, val_of(p)])
            .collect();
        let greedy_pts: Vec<[f64; 2]> = report
            .history
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.used_lns)
            .map(|(i, p)| [i as f64, val_of(p)])
            .collect();
        let lns_pts: Vec<[f64; 2]> = report
            .history
            .iter()
            .enumerate()
            .filter(|(_, p)| p.used_lns)
            .map(|(i, p)| [i as f64, val_of(p)])
            .collect();
        let current_pts: Vec<[f64; 2]> = report
            .history
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_current)
            .map(|(i, p)| [i as f64, val_of(p)])
            .collect();

        let tick_labels: Vec<String> = report
            .history
            .iter()
            .map(|p| format!("#{} {}", p.run_index, p.solver_tag))
            .collect();
        let hover_details: Vec<String> = report
            .history
            .iter()
            .map(|p| {
                format!(
                    "#{} {} ({})\nCost: ${:.2} | Distance: {:.1} km | Served: {}/{} ({:.0}%)",
                    p.run_index,
                    p.solver_tag,
                    p.solved_at,
                    p.cost_cents as f64 / 100.0,
                    p.distance_km,
                    p.served,
                    p.total,
                    p.serve_rate_pct
                )
            })
            .collect();

        let y_label = match metric {
            HistoryMetric::Cost => "Cost ($)",
            HistoryMetric::Distance => "Distance (km)",
            HistoryMetric::ServeRate => "Serve Rate (%)",
        };

        let plot = Plot::new(("kpis_history_plot", metric, report.solution_id.clone(), n))
            .height(175.0)
            .include_x(-0.4)
            .include_x(n as f64 - 0.6)
            .include_y(0.0)
            .x_axis_label("Solve Run (oldest -> newest)")
            .y_axis_label(y_label)
            .x_grid_spacer(move |input| discrete_axis_marks(n, input))
            .x_axis_formatter(move |mark, _| {
                let idx = mark.value.round() as isize;
                if (mark.value - idx as f64).abs() < 0.05 && idx >= 0 {
                    tick_labels.get(idx as usize).cloned().unwrap_or_default()
                } else {
                    String::new()
                }
            })
            .y_axis_formatter(move |mark, _| match metric {
                HistoryMetric::Cost => format!("${:.0}", mark.value),
                HistoryMetric::Distance => format!("{:.0} km", mark.value),
                HistoryMetric::ServeRate => format!("{:.0}%", mark.value),
            })
            .label_formatter(move |pos| match pos {
                HoverPosition::NearDataPoint { position, .. } => {
                    let idx = position.x.round() as isize;
                    if idx >= 0 {
                        hover_details.get(idx as usize).cloned()
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .legend(Legend::default().position(Corner::RightTop));

        plot.show(ui, |plot_ui| {
            plot_ui.line(
                Line::new("Trend", PlotPoints::new(line_pts))
                    .color(egui::Color32::from_rgb(130, 150, 175))
                    .width(2.0),
            );
            if !greedy_pts.is_empty() {
                plot_ui.points(
                    Points::new("Greedy / LS", PlotPoints::new(greedy_pts))
                        .shape(MarkerShape::Circle)
                        .color(egui::Color32::from_rgb(78, 121, 167))
                        .radius(5.0),
                );
            }
            if !lns_pts.is_empty() {
                plot_ui.points(
                    Points::new("LNS", PlotPoints::new(lns_pts))
                        .shape(MarkerShape::Diamond)
                        .color(egui::Color32::from_rgb(44, 160, 72))
                        .radius(6.0),
                );
            }
            if !current_pts.is_empty() {
                plot_ui.points(
                    Points::new("Selected Run", PlotPoints::new(current_pts))
                        .shape(MarkerShape::Asterisk)
                        .color(egui::Color32::from_rgb(255, 200, 40))
                        .radius(8.0),
                );
            }
        });
    });
}

fn sort_header_button(
    ui: &mut egui::Ui,
    label: &str,
    col: KpiTableColumn,
    state: &mut KpisUiState,
) {
    let active = state.table_sort == col;
    let suffix = if active {
        if state.table_descending {
            " ▼"
        } else {
            " ▲"
        }
    } else {
        ""
    };
    if ui
        .selectable_label(active, format!("{label}{suffix}"))
        .clicked()
    {
        if active {
            state.table_descending = !state.table_descending;
        } else {
            state.table_sort = col;
            state.table_descending = col != KpiTableColumn::Route;
        }
    }
}

fn render_routes_leaderboard(
    ui: &mut egui::Ui,
    report: &KpisReport,
    state: &mut KpisUiState,
) -> Option<KpisAction> {
    let mut action = None;
    ui.group(|ui| {
        ui.strong("Per-Route KPI Breakdown (click column header to sort)");
        ui.add_space(4.0);

        if report.routes.is_empty() {
            ui.weak("No routes to tabulate.");
            return;
        }

        let sorted = sort_table_rows(&report.routes, state.table_sort, state.table_descending);

        egui::ScrollArea::horizontal()
            .id_salt("kpis_route_table_scroll")
            .show(ui, |ui| {
                egui::Grid::new("kpis_routes_leaderboard_grid")
                    .striped(true)
                    .min_col_width(60.0)
                    .show(ui, |ui| {
                        sort_header_button(ui, "Route", KpiTableColumn::Route, state);
                        sort_header_button(ui, "Vehicle / Depot", KpiTableColumn::Vehicle, state);
                        ui.strong("Drivers");
                        sort_header_button(ui, "Stops", KpiTableColumn::Stops, state);
                        sort_header_button(ui, "Distance", KpiTableColumn::Distance, state);
                        sort_header_button(ui, "Duration", KpiTableColumn::Duration, state);
                        sort_header_button(ui, "Peak Load", KpiTableColumn::PeakLoad, state);
                        sort_header_button(ui, "Cost", KpiTableColumn::Cost, state);
                        sort_header_button(ui, "$ / km", KpiTableColumn::CostPerKm, state);
                        ui.strong("Inspect");
                        ui.end_row();

                        for row in sorted {
                            let selected = state.selected_route == Some(row.route_index);
                            let color = route_color(row.route_index);
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(10.0, 10.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(rect, 2.0, color);
                                if ui.selectable_label(selected, &row.route_label).clicked() {
                                    state.selected_route = if selected {
                                        None
                                    } else {
                                        Some(row.route_index)
                                    };
                                }
                            });
                            ui.label(format!("{} ({})", row.vehicle_name, row.depot_name));
                            ui.label(&row.driver_names);
                            ui.label(row.stops.to_string());
                            ui.label(format!("{:.1} km", row.distance_km));
                            ui.label(format!(
                                "{} (svc {})",
                                format_duration_hm(row.duration_min),
                                format_duration_hm(row.service_min)
                            ));
                            ui.add(
                                egui::ProgressBar::new(
                                    (row.load.peak_pct as f32 / 100.0).clamp(0.0, 1.0),
                                )
                                .desired_width(95.0)
                                .fill(utilization_color(row.load.peak_pct))
                                .text(format!("{:.0}% {}", row.load.peak_pct, row.load.bottleneck)),
                            );
                            ui.label(format!("${:.2}", row.cost.total_cents as f64 / 100.0));
                            ui.label(format!("${:.2}", row.cost_per_km_cents / 100.0));
                            if ui.small_button("Timeline ->").clicked() {
                                action = Some(KpisAction::OpenSolution {
                                    solution_id: report.solution_id.clone(),
                                    route_index: Some(row.route_index),
                                });
                            }
                            ui.end_row();
                        }
                    });
            });
    });
    action
}

fn render_unassigned_panel(ui: &mut egui::Ui, report: &KpisReport) {
    ui.group(|ui| {
        ui.colored_label(
            egui::Color32::from_rgb(218, 145, 25),
            format!(
                "Unassigned Places Impact ({} unserved — Demand Amb: {}, Chl: {}, Frz: {})",
                report.unassigned_stores.len(),
                report.unassigned_demand.ambient,
                report.unassigned_demand.chilled,
                report.unassigned_demand.frozen
            ),
        );
        ui.add_space(4.0);
        egui::ScrollArea::horizontal()
            .id_salt("kpis_unassigned_scroll")
            .show(ui, |ui| {
                egui::Grid::new("kpis_unassigned_grid")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("Place");
                        ui.strong("Category");
                        ui.strong("Demand (A/C/F)");
                        ui.strong("Service");
                        ui.strong("Receiving Window");
                        ui.end_row();
                        for s in &report.unassigned_stores {
                            ui.label(format!("{} (#{})", s.name, s.store_id));
                            ui.label(&s.category);
                            ui.label(format!(
                                "{}/{}/{}",
                                s.demand.ambient, s.demand.chilled, s.demand.frozen
                            ));
                            ui.label(format_duration_hm(s.service_min));
                            ui.label(&s.windows);
                            ui.end_row();
                        }
                    });
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        ChainRules, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules, LatLon, Minutes,
        PlannedStop, Problem, Shift, Solution, Store, StoreId, TimeWindow, Vehicle, VehicleId,
    };

    use crate::api_client::Kpis;
    use crate::app::SolutionBundle;

    fn sample_problem_and_solution() -> (KpisBundle, Vec<SolutionSummary>) {
        let depot = Depot {
            id: DepotId(0),
            name: "Main Hub".into(),
            location: LatLon::new(-33.86, 151.20),
            address: Some("100 Hub Rd".into()),
            timezone: None,
        };
        let stores = vec![
            Store {
                id: StoreId(0),
                chain: Chain::General,
                name: "Stop Alpha".into(),
                location: LatLon::new(-33.87, 151.21),
                address: None,
                demand: Load::new(6, 2, 0),
                service: Minutes::from_minutes(20),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(360),
                    Minutes::from_minutes(720),
                )],
                timezone: None,
            },
            Store {
                id: StoreId(1),
                chain: Chain::Coles,
                name: "Stop Beta".into(),
                location: LatLon::new(-33.88, 151.22),
                address: None,
                demand: Load::new(4, 6, 2),
                service: Minutes::from_minutes(25),
                receiving: vec![],
                timezone: None,
            },
            Store {
                id: StoreId(2),
                chain: Chain::General,
                name: "Unserved Gamma".into(),
                location: LatLon::new(-33.92, 151.28),
                address: None,
                demand: Load::new(3, 1, 1),
                service: Minutes::from_minutes(15),
                receiving: vec![TimeWindow::new(
                    Minutes::from_minutes(500),
                    Minutes::from_minutes(560),
                )],
                timezone: None,
            },
        ];
        let vehicles = vec![
            Vehicle {
                id: VehicleId(0),
                name: "Van 1".into(),
                depot: DepotId(0),
                capacity: Load::new(12, 10, 4),
                max_duration: Minutes::from_minutes(480),
                avg_speed_kmh: 60.0,
            },
            Vehicle {
                id: VehicleId(1),
                name: "Spare Van".into(),
                depot: DepotId(0),
                capacity: Load::new(10, 10, 4),
                max_duration: Minutes::from_minutes(480),
                avg_speed_kmh: 60.0,
            },
        ];
        let drivers = vec![Driver {
            id: DriverId(0),
            name: "Alex".into(),
            shifts: vec![Shift::new(
                Minutes::from_minutes(300),
                Minutes::from_minutes(900),
            )],
            fatigue_rules: FatigueRules::default(),
        }];
        let cost = CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 200,
            per_hour_cents: 6_000,
            late_per_min_cents: 0,
        };
        // Route duration = 120 min (360..480). Distance = 30.0 km.
        // Cost = 15000 + 6000 + 12000 = 33000 cents ($330.00).
        let route = RoutePlan {
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
                    depart: Minutes::from_minutes(400),
                },
                PlannedStop {
                    node: Node::Store(StoreId(1)),
                    arrive: Minutes::from_minutes(425),
                    depart: Minutes::from_minutes(450),
                },
                PlannedStop {
                    node: Node::Depot(DepotId(0)),
                    arrive: Minutes::from_minutes(480),
                    depart: Minutes::from_minutes(480),
                },
            ],
            distance_km: 30.0,
            duration: Minutes::from_minutes(120),
            cost_cents: 33_000,
        };

        let problem = Problem {
            name: "Metro Test, Hub".into(),
            depots: vec![depot],
            stores,
            vehicles,
            drivers,
            cost,
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
        };
        let solution = Solution {
            routes: vec![route],
            unassigned: vec![StoreId(2)],
            total_distance_km: 30.0,
            total_duration: Minutes::from_minutes(120),
            total_cost_cents: 33_000,
        };
        let bundle = KpisBundle {
            kpis: Kpis {
                served: 2,
                total: 3,
                serve_rate: 2.0 / 3.0,
                distance_km: 30.0,
                cost_cents: 33_000,
                routes: 1,
            },
            detail: SolutionBundle {
                id: "sol-lns".into(),
                problem_id: "prob-1".into(),
                problem,
                solution,
                raw: serde_json::json!({}),
            },
        };
        let history = vec![
            SolutionSummary {
                id: "sol-other".into(),
                problem_id: "prob-other".into(),
                served: 1,
                total: 1,
                distance_km: 5.0,
                cost_cents: 5_000,
                solved_at: "2026-11-12T08:00:00Z".into(),
                used_lns: false,
            },
            SolutionSummary {
                id: "sol-greedy".into(),
                problem_id: "prob-1".into(),
                served: 2,
                total: 3,
                distance_km: 38.0,
                cost_cents: 36_000,
                solved_at: "2026-11-13T08:00:00Z".into(),
                used_lns: false,
            },
            SolutionSummary {
                id: "sol-lns".into(),
                problem_id: "prob-1".into(),
                served: 2,
                total: 3,
                distance_km: 30.0,
                cost_cents: 33_000,
                solved_at: "2026-11-13T08:05:00Z".into(),
                used_lns: true,
            },
        ];
        (bundle, history)
    }

    #[test]
    fn cost_breakdown_reconciles_with_solver_cost_formula() {
        let model = CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 185,
            per_hour_cents: 6_000,
            late_per_min_cents: 0,
        };
        let cb = compute_cost_breakdown(model, 42.5, 90, 15_000 + 7_863 + 9_000);
        assert_eq!(cb.fixed_cents, 15_000);
        assert_eq!(cb.distance_cents, 7_863);
        assert_eq!(cb.time_cents, 9_000);
        assert_eq!(cb.other_cents, 0);
        assert_eq!(cb.total_cents, 31_863);

        // Zero-configured model attributes residual cleanly to `other_cents`.
        let fallback = compute_cost_breakdown(CostModel::default(), 10.0, 30, 1_234);
        assert_eq!(fallback.other_cents, 1_234);
        assert_eq!(fallback.total_cents, 1_234);
    }

    #[test]
    fn load_utilization_finds_peak_bottleneck_and_handles_zero_cap() {
        let util = LoadUtilization::from_loads(Load::new(10, 8, 0), Load::new(12, 10, 0));
        assert!((util.ambient_pct - 83.3333).abs() < 0.01);
        assert!((util.chilled_pct - 80.0).abs() < 0.01);
        assert_eq!(util.frozen_pct, 0.0);
        assert_eq!(util.bottleneck, "Amb");
        assert!((util.peak_pct - 83.3333).abs() < 0.01);
        assert!((util.total_pct - (18.0 / 22.0) * 100.0).abs() < 0.01);
    }

    #[test]
    fn report_aggregates_routes_unassigned_and_filtered_history() {
        let (bundle, history) = sample_problem_and_solution();
        let report = build_kpis_report(&bundle, &history);

        assert_eq!(report.served, 2);
        assert_eq!(report.unassigned, 1);
        assert_eq!(report.active_routes, 1);
        assert_eq!(report.total_vehicles, 2);
        assert!((report.fleet_utilization_pct - 50.0).abs() < 1e-6);

        assert_eq!(report.cost.fixed_cents, 15_000);
        assert_eq!(report.cost.distance_cents, 6_000);
        assert_eq!(report.cost.time_cents, 12_000);
        assert_eq!(report.cost.other_cents, 0);
        assert_eq!(report.total_service_min, 45);
        assert_eq!(report.total_travel_wait_min, 75);

        // Delivered: Store 0 (6,2,0) + Store 1 (4,6,2) = (10,8,2) against cap (12,10,4)
        // Amb = 83.3%, Chl = 80%, Frz = 50% -> peak Amb 83.3%
        assert_eq!(report.total_delivered, Load::new(10, 8, 2));
        assert_eq!(report.unassigned_demand, Load::new(3, 1, 1));
        assert_eq!(report.overall_load.bottleneck, "Amb");

        assert_eq!(report.routes.len(), 1);
        let r = &report.routes[0];
        assert_eq!(r.stops, 2);
        assert_eq!(r.start_min, Some(360));
        assert_eq!(r.end_min, Some(480));
        assert!((r.cost_per_km_cents - 1_100.0).abs() < 1e-6);
        assert!((r.cost_per_stop_cents - 16_500.0).abs() < 1e-6);

        // Categories include General (1 served, 1 unassigned) and Coles (1 served, 0 unassigned)
        assert_eq!(report.categories.len(), 2);
        assert_eq!(report.categories[0].category, "General");
        assert_eq!(report.categories[0].served_stores, 1);
        assert_eq!(report.categories[0].unassigned_stores, 1);
        assert_eq!(report.categories[1].category, "Coles");
        assert_eq!(report.categories[1].served_stores, 1);

        // History excludes prob-other and marks sol-lns as current
        assert_eq!(report.history.len(), 2);
        assert_eq!(report.history[0].solution_id, "sol-greedy");
        assert!(!report.history[0].is_current);
        assert_eq!(report.history[1].solution_id, "sol-lns");
        assert!(report.history[1].is_current);
        assert!(report.history[1].used_lns);
    }

    #[test]
    fn csv_and_summary_exports_include_totals_and_escape_commas() {
        let (mut bundle, history) = sample_problem_and_solution();
        bundle.detail.problem.vehicles[0].name = "Van, \"Alpha\"".into();
        let report = build_kpis_report(&bundle, &history);

        let csv = format_kpis_csv(&report);
        assert!(csv.starts_with("route,vehicle,depot,"));
        assert!(csv.contains("\"Van, \"\"Alpha\"\"\""));
        assert!(csv.contains("\nTOTAL,"));
        assert!(csv.contains("330.00"));

        let summary = format_kpis_summary(&report);
        assert!(summary.contains("Metro Test, Hub"));
        assert!(summary.contains("2/3 stops"));
        assert!(summary.contains("$330.00 total"));
        assert!(summary.contains("Unassigned Amb 3 / Chl 1 / Frz 1"));
    }

    #[test]
    fn discrete_axis_marks_generates_only_integer_indices() {
        let marks = discrete_axis_marks(
            5,
            GridInput {
                bounds: (-0.6, 4.6),
                base_step_size: 0.1,
            },
        );
        let values: Vec<f64> = marks.into_iter().map(|m| m.value).collect();
        assert_eq!(values, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn sorting_and_empty_solution_edge_cases_work_cleanly() {
        let (mut bundle, history) = sample_problem_and_solution();
        // Add a second shorter/cheaper route to exercise sorting order.
        let mut route_b = bundle.detail.solution.routes[0].clone();
        route_b.distance_km = 12.0;
        route_b.cost_cents = 18_000;
        bundle.detail.solution.routes.push(route_b);

        let report = build_kpis_report(&bundle, &history);
        assert_eq!(report.routes.len(), 2);

        let high_first = ordered_route_rows(
            &report.routes,
            KpiMetric::Distance,
            KpiChartSort::HighestFirst,
        );
        assert_eq!(high_first[0].route_label, "R1");
        assert_eq!(high_first[1].route_label, "R2");

        let asc_cost = sort_table_rows(&report.routes, KpiTableColumn::Cost, false);
        assert_eq!(asc_cost[0].route_label, "R2");
        assert_eq!(asc_cost[1].route_label, "R1");

        // Empty solution must yield finite zeros without panics or NaN.
        bundle.detail.solution.routes.clear();
        bundle.kpis.routes = 0;
        bundle.kpis.served = 0;
        bundle.kpis.distance_km = 0.0;
        bundle.kpis.cost_cents = 0;
        bundle.kpis.serve_rate = 0.0;
        let empty_report = build_kpis_report(&bundle, &[]);
        assert_eq!(empty_report.active_routes, 0);
        assert!(empty_report.cost_per_km_cents.is_finite());
        assert!(empty_report.cost_per_stop_cents.is_finite());
        assert!(empty_report.overall_load.peak_pct.is_finite());
    }
}
