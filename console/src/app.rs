//! Main egui application state and UI.
//!
//! Provides a tabbed operator console:
//! - **Import**: paste a problem JSON and POST it to the API
//! - **Solve**: select a problem, choose solver options, kick off a solve
//! - **Solutions**: list and inspect solved plans
//! - **KPIs**: view KPI summary for a solution
//! - **Map**: see routes drawn on an OpenStreetMap layer
//! - **Live Events**: SSE stream of solve completions

use std::collections::HashSet;
use std::result::Result;
use std::sync::mpsc;
use std::time::Duration;

use crate::api_client::{
    ApiClient, Kpis, ProblemSummary, SolutionSummary, SolveRequest, SolveResponse, from_json,
};
use eframe::egui;
use tokio::runtime::Handle;
use tokio_stream::StreamExt;
use walkers::{Map, MapMemory, Plugin, Projector, lon_lat};

/// Messages passed from background tasks to the UI thread.
enum Msg {
    ProblemImported(Result<String, anyhow::Error>),
    SolutionSolved(Result<SolveResponse, anyhow::Error>),
    SolutionFetched(Result<Box<SolutionBundle>, anyhow::Error>),
    KpisFetched(Result<Box<KpisBundle>, anyhow::Error>),
    MapLoaded(Result<Box<SolutionBundle>, anyhow::Error>),
    SolveEvent(crate::api_client::SolveEvent),
    ProblemsLoaded(Result<Vec<ProblemSummary>, anyhow::Error>),
    SolutionsLoaded(Result<Vec<SolutionSummary>, anyhow::Error>),
}

/// A solution plus its problem, parsed for the structured view. Holds the
/// raw solution JSON too, for the collapsible debug section.
#[derive(Clone)]
struct SolutionBundle {
    id: String,
    problem_id: String,
    problem: domain::Problem,
    solution: domain::Solution,
    raw: serde_json::Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KpiMetric {
    Cost,
    Distance,
}

/// KPI summary plus the full solution/problem, so the per-route chart has
/// something to plot. Boxed to keep [`Msg::KpisFetched`] small.
#[derive(Clone)]
struct KpisBundle {
    kpis: Kpis,
    detail: SolutionBundle,
}

/// Tab selection for the console.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Import,
    Solve,
    Solutions,
    Kpis,
    Map,
    Events,
}

/// Main console application state.
pub struct ConsoleApp {
    runtime: Handle,
    client: ApiClient,
    api_base_url: String,

    // Navigation
    tab: Tab,

    // Import tab state
    import_json: String,
    import_path: String,
    import_status: Option<Result<String, anyhow::Error>>,

    // Solve tab state
    problem_id_input: String,
    solve_lns: bool,
    osm_path: String,
    maxweight: String,
    maxheight: String,
    hgv: bool,
    solve_status: Option<Result<SolveResponse, anyhow::Error>>,
    solve_in_flight: bool,

    // Solutions tab state
    solution_id_input: String,
    solution_status: Option<Result<Box<SolutionBundle>, anyhow::Error>>,
    selected_route: usize,

    // KPIs tab state
    kpis_id_input: String,
    kpis_status: Option<Result<Box<KpisBundle>, anyhow::Error>>,
    kpis_metric: KpiMetric,

    // Map tab state
    map_id_input: String,
    map_status: Option<Result<Box<SolutionBundle>, anyhow::Error>>,
    map_tiles: Option<walkers::HttpTiles>,
    map_memory: MapMemory,
    map_fit_pending: bool,
    hidden_routes: HashSet<usize>,

    // Events tab state
    events: Vec<SolveEventDisplay>,
    max_events: usize,

    // Background message channel
    msg_rx: mpsc::Receiver<Msg>,
    msg_tx: mpsc::Sender<Msg>,

    // True once the background SSE task has been started. The task owns the
    // connection and reconnects with backoff, instead of polling on a timer.
    sse_started: bool,

    // Recent-problem/solution dropdowns, fetched from the list endpoints.
    recent_problems: Vec<ProblemSummary>,
    recent_solutions: Vec<SolutionSummary>,
    problems_error: Option<String>,
    solutions_error: Option<String>,
}

/// A solved event for display in the events tab.
#[derive(Clone)]
struct SolveEventDisplay {
    time: String,
    solution_id: String,
    problem_id: String,
    served: usize,
    total: usize,
    distance_km: f64,
    cost_cents: i64,
}

impl ConsoleApp {
    /// Create a new console app backed by the given Tokio runtime handle,
    /// talking to the API at `api_base_url`.
    pub fn new(runtime: Handle, api_base_url: String) -> Self {
        let (tx, rx) = mpsc::channel();
        let app = Self {
            runtime,
            client: ApiClient::new(&api_base_url),
            api_base_url,
            tab: Tab::Import,
            import_json: String::new(),
            import_path: "data/demo/problem.json".to_string(),
            import_status: None,
            problem_id_input: String::new(),
            solve_lns: false,
            osm_path: String::new(),
            maxweight: String::new(),
            maxheight: String::new(),
            hgv: false,
            solve_status: None,
            solve_in_flight: false,
            solution_id_input: String::new(),
            solution_status: None,
            selected_route: 0,
            kpis_id_input: String::new(),
            kpis_status: None,
            kpis_metric: KpiMetric::Cost,
            map_id_input: String::new(),
            map_status: None,
            map_tiles: None,
            map_memory: MapMemory::default(),
            map_fit_pending: true,
            hidden_routes: HashSet::new(),
            events: Vec::new(),
            max_events: 50,
            msg_rx: rx,
            msg_tx: tx,
            sse_started: false,
            recent_problems: Vec::new(),
            recent_solutions: Vec::new(),
            problems_error: None,
            solutions_error: None,
        };
        app.refresh_lists();
        app
    }

    /// Fetch both recent-item lists in the background. The results arrive as
    /// messages; a missing/unreachable API just leaves the dropdowns empty.
    fn refresh_lists(&self) {
        let tx = self.msg_tx.clone();
        let client = self.client.clone();
        self.runtime.spawn(async move {
            let problems = client.list_problems(20).await;
            tx.send(Msg::ProblemsLoaded(problems)).ok();
        });
        let tx = self.msg_tx.clone();
        let client = self.client.clone();
        self.runtime.spawn(async move {
            let solutions = client.list_solutions(None, 20).await;
            tx.send(Msg::SolutionsLoaded(solutions)).ok();
        });
    }
}

impl eframe::App for ConsoleApp {
    // eframe 0.37 replaced `update(ctx, ..)` with `ui(ui, ..)`. The root Ui has
    // no frame of its own; the panel below supplies the standard background.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Drain any messages from background tasks
        while let Ok(msg) = self.msg_rx.try_recv() {
            self.handle_msg(msg);
        }

        // Start the single background SSE task. It owns the connection and
        // reconnects with backoff, instead of polling on a fixed timer.
        if !self.sse_started {
            self.sse_started = true;
            let tx = self.msg_tx.clone();
            let base_url = self.api_base_url.clone();
            self.runtime.spawn(sse_loop(base_url, tx));
        }

        egui::Panel::left("sidebar").show(ui, |ui| {
            self.render_sidebar(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            self.render_content(ui);
        });

        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}

impl ConsoleApp {
    /// Render the left-hand navigation sidebar.
    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("Grocery Routes");
        ui.separator();

        self.tab_button(ui, "Import", Tab::Import);
        self.tab_button(ui, "Solve", Tab::Solve);
        self.tab_button(ui, "Solutions", Tab::Solutions);
        self.tab_button(ui, "KPIs", Tab::Kpis);
        self.tab_button(ui, "Map", Tab::Map);
        self.tab_button(ui, "Events", Tab::Events);

        ui.separator();
        ui.label(format!("API: {}", self.client.base_url()));
        if ui.button("Reconnect").clicked() {
            self.client = ApiClient::new(&self.api_base_url);
        }
    }

    fn tab_button(&mut self, ui: &mut egui::Ui, label: &str, tab: Tab) {
        let selected = self.tab == tab;
        let text = if selected {
            format!("▲ {label}")
        } else {
            label.to_string()
        };
        if ui.selectable_label(selected, text).clicked() {
            self.tab = tab;
        }
    }

    /// Render the content area for the active tab.
    fn render_content(&mut self, ui: &mut egui::Ui) {
        match self.tab {
            Tab::Import => self.render_import_tab(ui),
            Tab::Solve => self.render_solve_tab(ui),
            Tab::Solutions => self.render_solutions_tab(ui),
            Tab::Kpis => self.render_kpis_tab(ui),
            Tab::Map => self.render_map_tab(ui),
            Tab::Events => self.render_events_tab(ui),
        }
    }

    /// Handle a message from a background task.
    fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::ProblemImported(res) => {
                // Pre-fill the solve tab so the ID does not have to be copied
                // by hand after every import, and refresh the dropdowns.
                if let Ok(id) = &res {
                    self.problem_id_input.clone_from(id);
                    self.refresh_lists();
                }
                self.import_status = Some(res);
            }
            Msg::SolutionSolved(res) => {
                // Pre-fill the solutions and KPIs tabs with the new solution.
                self.solve_in_flight = false;
                if let Ok(resp) = &res {
                    self.solution_id_input.clone_from(&resp.id);
                    self.kpis_id_input.clone_from(&resp.id);
                    self.map_id_input.clone_from(&resp.id);
                    self.refresh_lists();
                }
                self.solve_status = Some(res);
            }
            Msg::ProblemsLoaded(res) => match res {
                Ok(problems) => {
                    self.recent_problems = problems;
                    self.problems_error = None;
                }
                Err(e) => self.problems_error = Some(format!("{e}")),
            },
            Msg::SolutionsLoaded(res) => match res {
                Ok(solutions) => {
                    self.recent_solutions = solutions;
                    self.solutions_error = None;
                }
                Err(e) => self.solutions_error = Some(format!("{e}")),
            },
            Msg::SolutionFetched(res) => self.solution_status = Some(res),
            Msg::KpisFetched(res) => self.kpis_status = Some(res),
            Msg::MapLoaded(res) => {
                if let Ok(bundle) = &res {
                    // Fit the map to the solution's extent on first paint;
                    // the handler does not know the map width yet, so the
                    // actual camera move happens in the map's render pass.
                    self.map_fit_pending = true;
                    self.hidden_routes.clear();
                    self.map_id_input.clone_from(&bundle.id);
                }
                self.map_status = Some(res);
            }
            Msg::SolveEvent(ev) => {
                self.events.push(SolveEventDisplay {
                    time: chrono::Local::now().format("%H:%M:%S").to_string(),
                    solution_id: ev.solution_id,
                    problem_id: ev.problem_id,
                    served: ev.served,
                    total: ev.total,
                    distance_km: ev.distance_km,
                    cost_cents: ev.cost_cents,
                });
                if self.events.len() > self.max_events {
                    self.events.drain(..self.events.len() - self.max_events);
                }
            }
        }
    }

    // ── Import tab ──────────────────────────────────────────────────────

    fn render_import_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Import Problem");

        ui.label("Load a file on disk:");
        ui.horizontal(|ui| {
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose a problem JSON file")
                    .add_filter("JSON", &["json"])
                    .set_directory(self.browse_dir())
                    .pick_file()
            {
                self.import_path = path.display().to_string();
                self.start_import_from_file();
            }
            ui.label("Path:");
            ui.add(
                egui::TextEdit::singleline(&mut self.import_path)
                    .desired_width(320.0)
                    .hint_text("data/demo/problem.json"),
            );
            if ui.button("Import from file").clicked() {
                self.start_import_from_file();
            }
        });

        ui.add_space(8.0);
        ui.label("…or paste a problem JSON document (serialized [`domain::Problem`]):");

        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("import_scroll")
            .max_height(300.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.import_json)
                        .desired_width(f32::INFINITY)
                        .desired_rows(12),
                );
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Import Problem").clicked() {
                self.start_import();
            }
            if ui.button("Clear").clicked() {
                self.import_json.clear();
                self.import_status = None;
            }
        });

        if self.render_import_status(ui) {
            self.tab = Tab::Solve;
        }
    }

    /// Start the background import of the JSON currently in the editor.
    fn start_import(&mut self) {
        if self.import_json.trim().is_empty() {
            self.import_status = Some(Err(anyhow::anyhow!(
                "the paste box is empty — paste a JSON document or use 'Import from file'"
            )));
            return;
        }
        self.spawn_import(self.import_json.clone());
    }

    /// Read the JSON from `self.import_path`, load it into the editor and
    /// start the import.
    /// Directory the file dialog should open in: the parent of the current
    /// path if it exists, otherwise the repo root if it has a demo folder,
    /// otherwise the current directory.
    fn browse_dir(&self) -> std::path::PathBuf {
        let current = std::path::PathBuf::from(&self.import_path);
        if let Some(parent) = current
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty() && parent.exists())
        {
            return parent.to_path_buf();
        }
        if std::path::Path::new("data/demo").is_dir() {
            return std::path::PathBuf::from("data/demo");
        }
        std::path::PathBuf::from(".")
    }

    fn start_import_from_file(&mut self) {
        let path = self.import_path.trim().to_string();
        self.import_status = None;
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                self.import_json = contents.clone();
                self.spawn_import(contents);
            }
            Err(err) => {
                self.import_status = Some(Err(anyhow::anyhow!("{path}: {err}")));
            }
        }
    }

    fn spawn_import(&mut self, json: String) {
        let tx = self.msg_tx.clone();
        let client = self.client.clone();
        self.import_status = None;
        self.runtime.spawn(async move {
            let val: serde_json::Value = match serde_json::from_str(&json) {
                Ok(v) => v,
                Err(e) => {
                    tx.send(Msg::ProblemImported(Err(anyhow::anyhow!(
                        "Invalid JSON: {e}"
                    ))))
                    .ok();
                    return;
                }
            };
            match client.import_problem(&val).await {
                Ok(resp) => {
                    tx.send(Msg::ProblemImported(Ok(resp.id))).ok();
                }
                Err(e) => {
                    tx.send(Msg::ProblemImported(Err(e))).ok();
                }
            }
        });
    }

    /// Render the import result. Returns true when the operator asked to open
    /// the Solve tab.
    fn render_import_status(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(status) = &self.import_status else {
            return false;
        };
        ui.add_space(4.0);
        match status {
            Ok(id) => {
                let mut go_to_solve = false;
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::GREEN, format!("Imported: {id}"));
                    copy_button(ui, "Copy problem ID", id);
                    if ui.button("Go to Solve").clicked() {
                        go_to_solve = true;
                    }
                });
                go_to_solve
            }
            Err(e) => {
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
                false
            }
        }
    }

    // ── Solve tab ───────────────────────────────────────────────────────

    fn render_solve_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Solve Problem");
        ui.label("Enter a problem ID and choose solver options:");

        ui.add_space(8.0);
        ui.label("Problem ID:");
        if recent_problems_dropdown(
            ui,
            "recent_problems",
            &self.recent_problems,
            &mut self.problem_id_input,
        ) {
            self.refresh_lists();
        }
        if let Some(err) = &self.problems_error {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("problem list unavailable: {err}"),
            );
        }
        ui.text_edit_singleline(&mut self.problem_id_input);

        ui.add_space(8.0);
        ui.checkbox(&mut self.solve_lns, "Use LNS/ALNS metaheuristic");

        ui.add_space(8.0);
        ui.label("OSM PBF path (optional, M3 road routing):");
        ui.text_edit_singleline(&mut self.osm_path);

        ui.add_space(8.0);
        ui.label("Max weight (tonnes):");
        ui.text_edit_singleline(&mut self.maxweight);
        ui.label("Max height (metres):");
        ui.text_edit_singleline(&mut self.maxheight);
        ui.checkbox(&mut self.hgv, "Heavy goods vehicle");

        ui.add_space(8.0);
        if ui
            .add_enabled(!self.solve_in_flight, egui::Button::new("Solve"))
            .clicked()
        {
            self.start_solve();
        }
        if self.solve_in_flight {
            ui.spinner();
            ui.label("Solving…");
        }

        if let Some(tab) = self.render_solve_status(ui) {
            self.tab = tab;
        }
    }

    /// Start a background solve with the options currently in the form.
    fn start_solve(&mut self) {
        let req = SolveRequest {
            problem_id: self.problem_id_input.clone(),
            lns: self.solve_lns,
            osm_path: non_empty(&self.osm_path),
            maxweight: parse_optional(&self.maxweight),
            maxheight: parse_optional(&self.maxheight),
            hgv: self.hgv,
        };
        let tx = self.msg_tx.clone();
        let client = self.client.clone();
        self.solve_status = None;
        self.solve_in_flight = true;
        self.runtime.spawn(async move {
            match client.solve(&req).await {
                Ok(resp) => {
                    tx.send(Msg::SolutionSolved(Ok(resp))).ok();
                }
                Err(e) => {
                    tx.send(Msg::SolutionSolved(Err(e))).ok();
                }
            }
        });
    }

    /// Render the solve result. Returns the tab the operator asked to open.
    fn render_solve_status(&mut self, ui: &mut egui::Ui) -> Option<Tab> {
        let status = self.solve_status.as_ref()?;
        ui.add_space(8.0);
        match status {
            Ok(resp) => {
                let mut next_tab = None;
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::GREEN,
                        format!(
                            "Solved: {} routes, {} of {} served, {:.1} km",
                            resp.id, resp.served, resp.total, resp.distance_km
                        ),
                    );
                    copy_button(ui, "Copy solution ID", &resp.id);
                    if ui.button("View Solution").clicked() {
                        next_tab = Some(Tab::Solutions);
                    }
                    if ui.button("View KPIs").clicked() {
                        next_tab = Some(Tab::Kpis);
                    }
                    if ui.button("View Route Map").clicked() {
                        next_tab = Some(Tab::Map);
                    }
                });
                next_tab
            }
            Err(e) => {
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
                None
            }
        }
    }

    // ── Solutions tab ───────────────────────────────────────────────────

    fn render_solutions_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("View Solution");
        ui.label("Enter a solution ID to retrieve it:");

        ui.add_space(8.0);
        ui.label("Solution ID:");
        if solutions_picker(
            ui,
            "recent_solutions_solutions",
            &self.recent_solutions,
            &self.solutions_error,
            &mut self.solution_id_input,
        ) {
            self.refresh_lists();
        }
        ui.text_edit_singleline(&mut self.solution_id_input);

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Fetch Solution").clicked() {
                let id = self.solution_id_input.clone();
                let client = self.client.clone();
                let tx = self.msg_tx.clone();
                self.solution_status = None;
                self.selected_route = 0;
                self.runtime.spawn(async move {
                    let result = fetch_solution_bundle(client, id).await;
                    tx.send(Msg::SolutionFetched(result)).ok();
                });
            }
        });

        ui.add_space(8.0);
        match &self.solution_status {
            Some(Ok(bundle)) => Self::render_solution_view(ui, bundle, &mut self.selected_route),
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
            }
            None => {}
        }
    }

    /// Render the structured solution view: summary strip, route table with a
    /// per-route timeline, unassigned stores, and a collapsible raw JSON
    /// debug section.
    fn render_solution_view(
        ui: &mut egui::Ui,
        bundle: &SolutionBundle,
        selected_route: &mut usize,
    ) {
        ui.separator();

        // Summary strip of stat cards.
        ui.horizontal(|ui| {
            stat_card(
                ui,
                "Served",
                &format!(
                    "{}/{}",
                    served_store_count(&bundle.solution),
                    total_store_count(bundle)
                ),
            );
            stat_card(ui, "Routes", &bundle.solution.routes.len().to_string());
            stat_card(
                ui,
                "Distance",
                &format!("{:.1} km", bundle.solution.total_distance_km),
            );
            stat_card(
                ui,
                "Cost",
                &format!("${:.2}", bundle.solution.total_cost_cents as f64 / 100.0),
            );
            copy_button(ui, "Copy solution ID", &bundle.id);
            copy_button(ui, "Copy problem ID", &bundle.problem_id);
        });
        ui.weak(format!("problem: {}", bundle.problem.name));

        ui.add_space(8.0);

        // Route table.
        egui::ScrollArea::horizontal().show(ui, |ui| {
            egui::Grid::new("solution_routes_grid")
                .striped(true)
                .show(ui, |ui| {
                    ui.label("");
                    ui.strong("Vehicle");
                    ui.strong("Drivers");
                    ui.strong("Stops");
                    ui.strong("Distance");
                    ui.strong("Duration");
                    ui.strong("Cost");
                    ui.end_row();

                    for (index, route) in bundle.solution.routes.iter().enumerate() {
                        let selected = *selected_route == index;
                        if ui
                            .selectable_label(selected, format!("Route {}", index + 1))
                            .clicked()
                        {
                            *selected_route = index;
                        }
                        ui.label(vehicle_name(&bundle.problem, route));
                        ui.label(driver_names(&bundle.problem, route));
                        ui.label(store_stop_count(route).to_string());
                        ui.label(format!("{:.1} km", route.distance_km));
                        ui.label(route.duration.to_string());
                        ui.label(format!("${:.2}", route.cost_cents as f64 / 100.0));
                        ui.end_row();
                    }
                });
        });

        ui.add_space(8.0);

        // Timeline for the selected route.
        if let Some(route) = bundle.solution.routes.get(*selected_route) {
            ui.strong(format!(
                "Route {} timeline ({})",
                *selected_route + 1,
                vehicle_name(&bundle.problem, route)
            ));
            egui::Grid::new("route_timeline_grid")
                .striped(true)
                .show(ui, |ui| {
                    ui.strong("#");
                    ui.strong("Stop");
                    ui.strong("Arrive");
                    ui.strong("Depart");
                    ui.end_row();
                    for (index, stop) in route.steps.iter().enumerate() {
                        ui.label((index + 1).to_string());
                        ui.label(node_label(&bundle.problem, stop.node));
                        ui.label(stop.arrive.to_string());
                        ui.label(stop.depart.to_string());
                        ui.end_row();
                    }
                });
        }

        // Unassigned stores, if any.
        if !bundle.solution.unassigned.is_empty() {
            ui.add_space(8.0);
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 0),
                format!("Unassigned ({}):", bundle.solution.unassigned.len()),
            );
            for store_id in &bundle.solution.unassigned {
                ui.label(format!("• {}", store_name(&bundle.problem, *store_id)));
            }
        }

        // Raw JSON, collapsed by default.
        ui.add_space(8.0);
        egui::CollapsingHeader::new("Raw solution JSON (debug)")
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.label(serde_json::to_string_pretty(&bundle.raw).unwrap_or_default());
                    });
            });
    }

    // ── KPIs tab ────────────────────────────────────────────────────────

    fn render_kpis_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Solution KPIs");
        ui.label("Enter a solution ID to view KPIs:");

        ui.add_space(8.0);
        ui.label("Solution ID:");
        if solutions_picker(
            ui,
            "recent_solutions_kpis",
            &self.recent_solutions,
            &self.solutions_error,
            &mut self.kpis_id_input,
        ) {
            self.refresh_lists();
        }
        ui.text_edit_singleline(&mut self.kpis_id_input);

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Fetch KPIs").clicked() {
                let id = self.kpis_id_input.clone();
                let tx = self.msg_tx.clone();
                let client = self.client.clone();
                self.kpis_status = None;
                self.runtime.spawn(async move {
                    let result = async {
                        let bundle = fetch_solution_bundle(client.clone(), id.clone()).await?;
                        let kpis = client.get_kpis(&id).await?;
                        Ok::<_, anyhow::Error>(Box::new(KpisBundle {
                            kpis,
                            detail: *bundle,
                        }))
                    }
                    .await;
                    tx.send(Msg::KpisFetched(result)).ok();
                });
            }
        });

        ui.add_space(8.0);
        match &self.kpis_status {
            Some(Ok(bundle)) => Self::render_kpis_view(ui, bundle, &mut self.kpis_metric),
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
            }
            None => {}
        }
    }

    /// Render the KPI view: stat cards, a serve-rate progress bar and a
    /// per-route bar chart for the chosen metric.
    fn render_kpis_view(ui: &mut egui::Ui, bundle: &KpisBundle, metric: &mut KpiMetric) {
        ui.separator();
        let kpis = &bundle.kpis;

        ui.horizontal(|ui| {
            stat_card(ui, "Served", &format!("{}/{}", kpis.served, kpis.total));
            stat_card(
                ui,
                "Serve rate",
                &format!("{:.1}%", kpis.serve_rate * 100.0),
            );
            stat_card(ui, "Distance", &format!("{:.1} km", kpis.distance_km));
            stat_card(ui, "Routes", &kpis.routes.to_string());
            stat_card(
                ui,
                "Cost",
                &format!("${:.2}", kpis.cost_cents as f64 / 100.0),
            );
            copy_button(ui, "Copy solution ID", &bundle.detail.id);
        });

        ui.add_space(8.0);
        ui.label("Serve rate");
        ui.add(
            egui::ProgressBar::new(kpis.serve_rate as f32)
                .desired_width(400.0)
                .text(format!("{:.0}% of stores served", kpis.serve_rate * 100.0)),
        );

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Per-route chart:");
            ui.selectable_value(metric, KpiMetric::Cost, "Cost ($)");
            ui.selectable_value(metric, KpiMetric::Distance, "Distance (km)");
        });
        ui.add_space(4.0);

        let bars: Vec<egui_plot::Bar> = bundle
            .detail
            .solution
            .routes
            .iter()
            .enumerate()
            .map(|(index, route)| {
                let value = match metric {
                    KpiMetric::Cost => route.cost_cents as f64 / 100.0,
                    KpiMetric::Distance => route.distance_km,
                };
                egui_plot::Bar::new(index as f64, value)
                    .width(0.6)
                    .name(format!("Route {}", index + 1))
            })
            .collect();
        let y_label = match metric {
            KpiMetric::Cost => "Cost ($)",
            KpiMetric::Distance => "Distance (km)",
        };
        let chart = egui_plot::BarChart::new(y_label, bars)
            .vertical()
            .color(egui::Color32::from_rgb(200, 150, 60));
        egui_plot::Plot::new("kpis_per_route_chart")
            .height(180.0)
            .x_axis_label("Route")
            .x_axis_formatter(|mark, _| format!("R{}", mark.value.round() as i64 + 1))
            .y_axis_label(y_label)
            .legend(egui_plot::Legend::default())
            .show(ui, |plot_ui| plot_ui.bar_chart(chart));
    }

    // ── Map tab ──────────────────────────────────────────────────────

    fn render_map_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Route Map");
        ui.label("Draw a solution's routes on OpenStreetMap tiles.");

        ui.add_space(8.0);
        ui.label("Solution ID:");
        if recent_solutions_dropdown(
            ui,
            "recent_solutions_map",
            &self.recent_solutions,
            &mut self.map_id_input,
        ) {
            self.refresh_lists();
        }
        if let Some(err) = &self.solutions_error {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("solutions list unavailable: {err}"),
            );
        }
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.map_id_input);
            if ui.button("Load Map").clicked() {
                let id = self.map_id_input.clone();
                let tx = self.msg_tx.clone();
                let client = self.client.clone();
                self.map_status = None;
                self.map_fit_pending = true;
                self.hidden_routes.clear();
                self.runtime.spawn(async move {
                    let result = fetch_solution_bundle(client, id).await;
                    tx.send(Msg::MapLoaded(result)).ok();
                });
            }
        });

        ui.add_space(8.0);
        match &self.map_status {
            Some(Ok(bundle)) => render_map_body(
                ui,
                bundle,
                &mut self.map_tiles,
                &mut self.map_memory,
                &mut self.map_fit_pending,
                &mut self.hidden_routes,
            ),
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
            }
            None => {
                ui.weak("(no solution loaded - pick one above and click Load Map)");
            }
        }
    }
}

/// The per-route colour chips plus the Fit-extents button, above the map.
fn route_chips(
    ui: &mut egui::Ui,
    bundle: &SolutionBundle,
    map_fit_pending: &mut bool,
    hidden_routes: &mut HashSet<usize>,
) {
    ui.horizontal(|ui| {
        for index in 0..bundle.solution.routes.len() {
            route_chip(ui, index, hidden_routes);
        }
        if ui.button("Fit extents").clicked() {
            *map_fit_pending = true;
        }
    });
}

/// One coloured route-visibility chip; clicking it toggles the route.
fn route_chip(ui: &mut egui::Ui, index: usize, hidden_routes: &mut HashSet<usize>) {
    let hidden = hidden_routes.contains(&index);
    egui::Frame::default()
        .fill(route_color(index))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            let text = egui::RichText::new(format!("R{}", index + 1)).color(egui::Color32::WHITE);
            let text = if hidden { text } else { text.strong() };
            if ui.selectable_label(!hidden, text).clicked() {
                if hidden {
                    hidden_routes.remove(&index);
                } else {
                    hidden_routes.insert(index);
                }
            }
        });
}

/// Render the loaded map body: route chips, the map itself, attribution.
fn render_map_body(
    ui: &mut egui::Ui,
    bundle: &SolutionBundle,
    map_tiles: &mut Option<walkers::HttpTiles>,
    map_memory: &mut MapMemory,
    map_fit_pending: &mut bool,
    hidden_routes: &mut HashSet<usize>,
) {
    route_chips(ui, bundle, map_fit_pending, hidden_routes);
    zoom_controls(ui, map_memory);

    // Tiles are created lazily on first paint; downloading starts when
    // the tab is actually shown.
    let tiles = map_tiles.get_or_insert_with(|| {
        walkers::HttpTiles::new(walkers::sources::OpenStreetMap, ui.ctx().clone())
    });

    if *map_fit_pending {
        let available = ui.available_size();
        fit_map_to_extents(
            map_memory,
            bundle,
            f64::from(available.x),
            f64::from(available.y),
        );
        *map_fit_pending = false;
    }

    ui.separator();
    ui.add(
        Map::new(Some(tiles), map_memory, lon_lat(151.21, -33.87))
            // Wheel zooms without needing Ctrl; double-click zooms in,
            // right-double-click zooms out. Drag still pans.
            .zoom_with_ctrl(false)
            .double_click_to_zoom(true)
            .double_click_to_zoom_out(true)
            .with_plugin(RouteOverlay {
                bundle,
                hidden: hidden_routes,
            }),
    );
    ui.horizontal(|ui| {
        ui.weak("© OpenStreetMap contributors — tile.openstreetmap.org");
        ui.weak(format!(
            "· showing {} of {} routes",
            bundle.solution.routes.len() - hidden_routes.len(),
            bundle.solution.routes.len()
        ));
        ui.weak(format!("· zoom {:.1}", map_memory.zoom()));
    });
}

/// Zoom in/out buttons plus the current zoom level.
fn zoom_controls(ui: &mut egui::Ui, map_memory: &mut MapMemory) {
    ui.horizontal(|ui| {
        if ui.button("Zoom −").clicked() {
            let _ = map_memory.zoom_out();
        }
        if ui.button("Zoom +").clicked() {
            let _ = map_memory.zoom_in();
        }
        ui.weak(format!("zoom {:.1}", map_memory.zoom()));
    });
}

impl ConsoleApp {
    // ── Events tab ──────────────────────────────────────────────────────

    fn render_events_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Live Solve Events");
        ui.label("SSE stream of solve completions from the server.");

        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("events_grid").striped(true).show(ui, |ui| {
                ui.label("Time");
                ui.label("Solution ID");
                ui.label("Problem ID");
                ui.label("Served");
                ui.label("Total");
                ui.label("Distance (km)");
                ui.label("Cost (cents)");
                ui.end_row();
                for ev in self.events.iter().rev() {
                    ui.label(&ev.time);
                    ui.label(&ev.solution_id);
                    ui.label(&ev.problem_id);
                    ui.label(ev.served.to_string());
                    ui.label(ev.total.to_string());
                    ui.label(format!("{:.1}", ev.distance_km));
                    ui.label(ev.cost_cents.to_string());
                    ui.end_row();
                }
            });
        });

        ui.add_space(4.0);
        if ui.button("Clear Events").clicked() {
            self.events.clear();
        }
        if self.events.is_empty() {
            ui.label("(no events yet — connect to a running API server)");
        }
    }
}

/// Truncate `text` for display in a dropdown, appending an ellipsis when
/// it does not fit.
fn truncate_for_dropdown(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut short: String = text.chars().take(max_chars).collect();
    short.push('…');
    short
}

/// ComboBox of recent imported problems; selecting one writes its id into
/// `target`. Returns true when the operator clicked Refresh.
fn recent_problems_dropdown(
    ui: &mut egui::Ui,
    salt: &str,
    problems: &[ProblemSummary],
    target: &mut String,
) -> bool {
    let current = target.trim().to_string();
    let selected = problems
        .iter()
        .find(|problem| problem.id == current)
        .map(|problem| truncate_for_dropdown(&problem.name, 48))
        .unwrap_or_else(|| format!("choose a recent problem ({})…", problems.len()));
    let mut refresh = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(salt)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if problems.is_empty() {
                    ui.label("(no imported problems yet)");
                }
                for problem in problems {
                    let label = format!(
                        "{} — {} stores, {} vehicles ({})",
                        truncate_for_dropdown(&problem.name, 40),
                        problem.stores,
                        problem.vehicles,
                        problem.created_at
                    );
                    ui.selectable_value(target, problem.id.clone(), label);
                }
            });
        if ui.small_button("Refresh").clicked() {
            refresh = true;
        }
    });
    refresh
}

/// Recent-solutions dropdown plus its error label. Used by the Solution-ID
/// inputs in the Solutions and KPIs tabs; returns true when the operator
/// clicked Refresh.
fn solutions_picker(
    ui: &mut egui::Ui,
    salt: &str,
    solutions: &[SolutionSummary],
    error: &Option<String>,
    target: &mut String,
) -> bool {
    let refresh = recent_solutions_dropdown(ui, salt, solutions, target);
    if let Some(err) = error {
        ui.colored_label(
            egui::Color32::YELLOW,
            format!("solutions list unavailable: {err}"),
        );
    }
    refresh
}

/// ComboBox of recent solved solutions; selecting one writes its id into
/// `target`. Returns true when the operator clicked Refresh.
fn recent_solutions_dropdown(
    ui: &mut egui::Ui,
    salt: &str,
    solutions: &[SolutionSummary],
    target: &mut String,
) -> bool {
    let current = target.trim().to_string();
    let selected = solutions
        .iter()
        .find(|solution| solution.id == current)
        .map(solution_option_label)
        .unwrap_or_else(|| format!("choose a recent solution ({})…", solutions.len()));
    let mut refresh = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(salt)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if solutions.is_empty() {
                    ui.label("(no solutions yet)");
                }
                for solution in solutions {
                    let label = solution_option_label(solution);
                    ui.selectable_value(target, solution.id.clone(), label);
                }
            });
        if ui.small_button("Refresh").clicked() {
            refresh = true;
        }
    });
    refresh
}

/// Distinct colours per route index, cycling if there are more than ten.
const ROUTE_COLORS: [egui::Color32; 10] = [
    egui::Color32::from_rgb(31, 119, 180),
    egui::Color32::from_rgb(255, 127, 14),
    egui::Color32::from_rgb(44, 160, 44),
    egui::Color32::from_rgb(214, 39, 40),
    egui::Color32::from_rgb(148, 103, 189),
    egui::Color32::from_rgb(140, 86, 75),
    egui::Color32::from_rgb(227, 119, 194),
    egui::Color32::from_rgb(127, 127, 127),
    egui::Color32::from_rgb(188, 189, 34),
    egui::Color32::from_rgb(23, 190, 207),
];

fn route_color(index: usize) -> egui::Color32 {
    ROUTE_COLORS[index % ROUTE_COLORS.len()]
}

/// The (lon, lat) of a depot or store node, if it exists in the problem.
fn node_lon_lat(problem: &domain::Problem, node: domain::Node) -> Option<(f64, f64)> {
    match node {
        domain::Node::Depot(id) => problem
            .depots
            .get(id.0 as usize)
            .map(|depot| (depot.location.lon, depot.location.lat)),
        domain::Node::Store(id) => problem
            .stores
            .get(id.0 as usize)
            .map(|store| (store.location.lon, store.location.lat)),
    }
}

/// Fit the camera to the solution's extent. Runs at map-paint time, when
/// the available width is known.
fn fit_map_to_extents(memory: &mut MapMemory, bundle: &SolutionBundle, width: f64, height: f64) {
    let mut min_lon = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut min_lat = f64::INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    let mut include = |lon: f64, lat: f64| {
        min_lon = min_lon.min(lon);
        max_lon = max_lon.max(lon);
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
    };
    for route in &bundle.solution.routes {
        for stop in &route.steps {
            if let Some((lon, lat)) = node_lon_lat(&bundle.problem, stop.node) {
                include(lon, lat);
            }
        }
    }
    for store_id in &bundle.solution.unassigned {
        if let Some(store) = bundle.problem.stores.get(store_id.0 as usize) {
            include(store.location.lon, store.location.lat);
        }
    }
    if !min_lon.is_finite() || width <= 0.0 || height <= 0.0 {
        return;
    }

    let center_lon = (min_lon + max_lon) / 2.0;
    let center_lat = (min_lat + max_lat) / 2.0;
    memory.center_at(lon_lat(center_lon, center_lat));

    let lon_span = (max_lon - min_lon).max(0.01);
    let lat_span = (max_lat - min_lat).max(0.01);
    let zoom_lon = (width * 360.0 / (256.0 * lon_span)).log2();
    let lat_span_eff = lat_span / center_lat.to_radians().cos().abs().max(0.1);
    let zoom_lat = (height * 170.1 / (256.0 * lat_span_eff)).log2();
    let zoom = (zoom_lon.min(zoom_lat) - 0.4).clamp(3.0, 14.0);
    let _ = memory.set_zoom(zoom);
}

/// Draws depot markers, stop dots and per-route polylines on the map.
struct RouteOverlay<'a> {
    bundle: &'a SolutionBundle,
    hidden: &'a HashSet<usize>,
}

impl Plugin for RouteOverlay<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        _response: &egui::Response,
        projector: &Projector,
        _memory: &MapMemory,
    ) {
        let painter = ui.painter();
        for (index, route) in self.bundle.solution.routes.iter().enumerate() {
            if self.hidden.contains(&index) {
                continue;
            }
            let color = route_color(index);
            let stroke = egui::Stroke::new(2.5_f32, color);
            let mut previous = None;
            for stop in &route.steps {
                let Some((lon, lat)) = node_lon_lat(&self.bundle.problem, stop.node) else {
                    continue;
                };
                let projected = projector.project(lon_lat(lon, lat));
                let pos = egui::pos2(projected.x, projected.y);
                if let Some(p) = previous {
                    painter.line_segment([p, pos], stroke);
                }
                previous = Some(pos);
                match stop.node {
                    domain::Node::Depot(_) => {
                        painter.circle(
                            pos,
                            6.0,
                            egui::Color32::from_rgb(30, 60, 160),
                            egui::Stroke::new(2.0, egui::Color32::WHITE),
                        );
                    }
                    domain::Node::Store(_) => {
                        painter.circle_filled(pos, 4.0, color);
                        painter.circle_stroke(
                            pos,
                            4.0,
                            egui::Stroke::new(1.0, egui::Color32::WHITE),
                        );
                    }
                }
            }
        }
    }
}

/// A stat card for summary strips (solution view and the KPIs tab).
fn stat_card(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::default()
        .inner_margin(egui::Margin::same(8))
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(label).small().strong());
                ui.label(egui::RichText::new(value).strong().monospace());
            });
        });
}

fn served_store_count(solution: &domain::Solution) -> usize {
    solution
        .routes
        .iter()
        .flat_map(|route| &route.steps)
        .filter(|stop| matches!(stop.node, domain::Node::Store(_)))
        .count()
}

fn total_store_count(bundle: &SolutionBundle) -> usize {
    served_store_count(&bundle.solution) + bundle.solution.unassigned.len()
}

fn store_stop_count(route: &domain::RoutePlan) -> usize {
    route
        .steps
        .iter()
        .filter(|stop| matches!(stop.node, domain::Node::Store(_)))
        .count()
}

fn vehicle_name<'a>(problem: &'a domain::Problem, route: &domain::RoutePlan) -> &'a str {
    problem
        .vehicles
        .get(route.vehicle.0 as usize)
        .map(|vehicle| vehicle.name.as_str())
        .unwrap_or("?")
}

fn driver_names(problem: &domain::Problem, route: &domain::RoutePlan) -> String {
    route
        .drivers
        .iter()
        .map(|driver_id| {
            problem
                .drivers
                .get(driver_id.0 as usize)
                .map(|driver| driver.name.as_str())
                .unwrap_or("?")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn store_name(problem: &domain::Problem, id: domain::StoreId) -> String {
    problem
        .stores
        .get(id.0 as usize)
        .map(|store| format!("{} (#{})", store.name, id.0))
        .unwrap_or_else(|| format!("Store #{}", id.0))
}

fn depot_label(problem: &domain::Problem, id: domain::DepotId) -> String {
    problem
        .depots
        .get(id.0 as usize)
        .map(|depot| format!("Depot — {}", depot.name))
        .unwrap_or_else(|| format!("Depot #{}", id.0))
}

fn node_label(problem: &domain::Problem, node: domain::Node) -> String {
    match node {
        domain::Node::Depot(id) => depot_label(problem, id),
        domain::Node::Store(id) => store_name(problem, id),
    }
}

fn solution_option_label(solution: &SolutionSummary) -> String {
    format!(
        "{}/{} served · {:.1} km · ${:.2} ({})",
        solution.served,
        solution.total,
        solution.distance_km,
        solution.cost_cents as f64 / 100.0,
        solution.solved_at
    )
}

/// A button that copies `text` to the system clipboard, labelled `label`.
fn copy_button(ui: &mut egui::Ui, label: &str, text: &str) {
    if ui
        .button(label)
        .on_hover_text("Copy to the clipboard")
        .clicked()
    {
        ui.ctx().copy_text(text.to_owned());
    }
}

/// `Some(text)` when a text field holds something.
fn non_empty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// Parse an optional numeric solver option; empty or invalid means unset.
fn parse_optional(text: &str) -> Option<f64> {
    if text.is_empty() {
        None
    } else {
        text.parse().ok()
    }
}

/// Fetch a solution and its problem into a [`SolutionBundle`], for the
/// Solutions, KPIs and Map tabs (which all need the same pair).
async fn fetch_solution_bundle(
    client: ApiClient,
    id: String,
) -> Result<Box<SolutionBundle>, anyhow::Error> {
    let resp = client.get_solution(&id).await?;
    let problem_raw = client.get_problem(&resp.problem_id).await?;
    let problem: domain::Problem = from_json(&problem_raw)?;
    let solution: domain::Solution = from_json(&resp.solution)?;
    Ok(Box::new(SolutionBundle {
        id,
        problem_id: resp.problem_id,
        problem,
        solution,
        raw: resp.solution,
    }))
}

/// Runs a single background SSE connection for the lifetime of the console.
///
/// It connects to the API's `/events` stream, reads `data: ` lines
/// incrementally, and reconnects only when the stream ends or fails. Each
/// reconnect waits with exponential backoff (starting at 1 second, capped at
/// 30 seconds), so a slow or unavailable server does not lead to duplicate
/// connections or unbounded polling.
async fn sse_loop(base_url: String, tx: mpsc::Sender<Msg>) {
    let client = ApiClient::new(base_url);
    let mut backoff = Duration::from_secs(1);

    loop {
        if let Ok(resp) = client.events_stream().await {
            let mut stream = resp.bytes_stream();
            let mut buf = String::new();
            while let Some(chunk) = stream.next().await {
                let Ok(bytes) = chunk else {
                    break;
                };
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf.drain(..=pos);
                    if let Some(data) = line.strip_prefix("data: ")
                        && let Ok(event) =
                            serde_json::from_str::<crate::api_client::SolveEvent>(data)
                    {
                        tx.send(Msg::SolveEvent(event)).ok();
                    }
                }
            }
            backoff = Duration::from_secs(1);
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handles_import_success() {
        let mut app = ConsoleApp::new(Handle::current(), "http://127.0.0.1:8080".to_string());
        app.handle_msg(Msg::ProblemImported(Ok("problem-123".into())));

        match app.import_status {
            Some(Ok(id)) => assert_eq!(id, "problem-123"),
            other => panic!("unexpected import status: {other:?}"),
        }
        // The solve tab is pre-filled so the ID does not have to be copied.
        assert_eq!(app.problem_id_input, "problem-123");
    }

    #[tokio::test]
    async fn handles_import_error() {
        let mut app = ConsoleApp::new(Handle::current(), "http://127.0.0.1:8080".to_string());
        app.problem_id_input = "previous-problem".into();
        app.handle_msg(Msg::ProblemImported(Err(anyhow::anyhow!("server said no"))));

        match app.import_status {
            Some(Err(error)) => assert!(error.to_string().contains("server said no")),
            other => panic!("unexpected import status: {other:?}"),
        }
        // A failed import must not clobber the previous problem ID.
        assert_eq!(app.problem_id_input, "previous-problem");
    }

    #[tokio::test]
    async fn handles_solve_response() {
        let mut app = ConsoleApp::new(Handle::current(), "http://127.0.0.1:8080".to_string());
        let response = SolveResponse {
            id: "solution-123".into(),
            served: 2,
            total: 2,
            distance_km: 12.5,
            cost_cents: 1000,
        };

        app.handle_msg(Msg::SolutionSolved(Ok(response)));

        match app.solve_status {
            Some(Ok(response)) => assert_eq!(response.id, "solution-123"),
            other => panic!("unexpected solve status: {other:?}"),
        }
        // The solutions and KPIs tabs are pre-filled with the new solution.
        assert_eq!(app.solution_id_input, "solution-123");
        assert_eq!(app.kpis_id_input, "solution-123");
    }

    // Fixture mirrors the Problem literals shared by the domain/routing test
    // suites (see the code-quality skill on test-fixture duplication); marked
    // explicitly rather than restructured.
    // jscpd:ignore-start
    fn test_solution_bundle() -> SolutionBundle {
        use domain::{
            Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId,
            FatigueRules, Load, Minutes, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
        };
        let problem = domain::Problem {
            name: "test".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Depot".into(),
                location: domain::LatLon::new(-33.87, 151.21),
                timezone: None,
            }],
            stores: vec![Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "Store 0".into(),
                location: domain::LatLon::new(-33.92, 151.23),
                timezone: None,
                demand: Load::new(1, 0, 0),
                service: Minutes::from_minutes(15),
                receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
            }],
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
            cost: CostModel {
                fixed_per_route_cents: 0,
                per_km_cents: 0,
                per_hour_cents: 0,
                late_per_min_cents: 0,
            },
            chain_rules: ChainRules::default(),
            curfews: CurfewRules::default(),
            horizon_start: None,
        };
        let solution = domain::Solution {
            routes: vec![domain::RoutePlan {
                vehicle: VehicleId(0),
                drivers: vec![DriverId(0)],
                steps: vec![],
                distance_km: 12.5,
                duration: Minutes::from_minutes(125),
                cost_cents: 1000,
            }],
            unassigned: vec![],
            total_distance_km: 12.5,
            total_duration: Minutes::from_minutes(125),
            total_cost_cents: 1000,
        };
        SolutionBundle {
            id: "solution-123".into(),
            problem_id: "problem-123".into(),
            problem,
            solution,
            raw: serde_json::json!({}),
        }
    }
    // jscpd:ignore-end

    #[tokio::test]
    async fn handles_kpis_response() {
        let mut app = ConsoleApp::new(Handle::current(), "http://127.0.0.1:8080".to_string());
        let kpis = Kpis {
            served: 2,
            total: 2,
            serve_rate: 1.0,
            distance_km: 12.5,
            cost_cents: 1000,
            routes: 1,
        };

        app.handle_msg(Msg::KpisFetched(Ok(Box::new(KpisBundle {
            kpis,
            detail: test_solution_bundle(),
        }))));

        match app.kpis_status {
            Some(Ok(bundle)) => {
                assert_eq!(bundle.kpis.routes, 1);
                assert_eq!(bundle.detail.solution.routes.len(), 1);
                assert_eq!(bundle.detail.id, "solution-123");
            }
            _ => panic!("unexpected KPI status (expected Ok with 1 route)"),
        }
    }

    #[tokio::test]
    async fn events_are_bounded_to_max_events() {
        let mut app = ConsoleApp::new(Handle::current(), "http://127.0.0.1:8080".to_string());

        for index in 0..60 {
            app.handle_msg(Msg::SolveEvent(crate::api_client::SolveEvent {
                solution_id: format!("solution-{index}"),
                problem_id: "problem-123".into(),
                served: 2,
                total: 2,
                distance_km: 12.5,
                cost_cents: 1000,
            }));
        }

        assert_eq!(app.events.len(), 50);
        assert_eq!(app.events.first().unwrap().solution_id, "solution-10");
        assert_eq!(app.events.last().unwrap().solution_id, "solution-59");
    }
}
