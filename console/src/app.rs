//! Main egui application state and UI.
//!
//! Provides a tabbed operator console:
//! - **Import**: paste a problem JSON, build custom routes, and POST to the API
//! - **Truck Load**: configure truck dimensions/volume/payload and visualize packing
//! - **Solve**: select a problem, choose solver options, kick off a solve
//! - **Solutions**: list and inspect solved plans
//! - **KPIs**: view KPI summary for a solution
//! - **Map**: see routes drawn on an OpenStreetMap layer
//! - **Live Events**: SSE stream of solve completions

use std::collections::HashSet;
use std::path::PathBuf;
use std::result::Result;
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

use crate::api_client::{
    ApiClient, Kpis, ProblemSummary, SolutionGeometryData, SolutionSummary, SolveRequest,
    SolveResponse, from_json,
};
use crate::geocode::{GeocodeMatch, GeocodeResponse, GeocodeTarget};
use crate::kpis::{KpisAction, KpisUiState, csv_escape, render_kpis_dashboard};
use crate::map_nodes::{
    MapNodeInfo, MapNodeKind, collect_map_nodes, diagnose_unassigned_store, format_windows,
    nodes_near_pointer, pick_non_overlapping_rect,
};
use crate::problem_builder::{
    ALL_CHAINS, EditorSection, MapPickMode, ProblemBuilderState, city_center, city_label,
};
use crate::truck_load::{LoadTargetSource, TruckLoadAction, TruckLoadState, render_truck_load_tab};
use eframe::egui;
use tokio::runtime::Handle;
use tokio_stream::StreamExt;
use walkers::{Map, MapMemory, Plugin, Projector, lon_lat};

/// Messages passed from background tasks to the UI thread.
enum Msg {
    ProblemImported(Result<String, anyhow::Error>),
    ProblemFetchedForBuilder(Result<Box<domain::Problem>, anyhow::Error>),
    GeocodeCompleted(Result<GeocodeResponse, anyhow::Error>),
    SolutionSolved(Result<SolveResponse, anyhow::Error>),
    SolutionFetched(Result<Box<SolutionBundle>, anyhow::Error>),
    KpisFetched(Result<Box<KpisBundle>, anyhow::Error>),
    MapLoaded(Result<Box<SolutionBundle>, anyhow::Error>),
    MapGeometryLoaded(Result<SolutionGeometryData, anyhow::Error>),
    SolveEvent(crate::api_client::SolveEvent),
    ProblemsLoaded(Result<Vec<ProblemSummary>, anyhow::Error>),
    SolutionsLoaded(Result<Vec<SolutionSummary>, anyhow::Error>),
}

/// A solution plus its problem, parsed for the structured view. Holds the
/// raw solution JSON too, for the collapsible debug section.
#[derive(Clone)]
pub(crate) struct SolutionBundle {
    pub(crate) id: String,
    pub(crate) problem_id: String,
    pub(crate) problem: domain::Problem,
    pub(crate) solution: domain::Solution,
    pub(crate) raw: serde_json::Value,
}

/// KPI summary plus the full solution/problem, so analytics and charts have
/// full route/stop/vehicle detail to compute without extra API calls. Boxed to
/// keep [`Msg::KpisFetched`] small.
#[derive(Clone)]
pub(crate) struct KpisBundle {
    pub(crate) kpis: Kpis,
    pub(crate) detail: SolutionBundle,
}

/// Tab selection for the console.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Import,
    Truck,
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
    problem_builder: ProblemBuilderState,
    builder_map_memory: MapMemory,
    builder_map_fit_pending: bool,
    geocode_client: reqwest::Client,

    // Truck Load tab state
    truck_ui: TruckLoadState,

    // Solve tab state
    problem_id_input: String,
    solve_lns: bool,
    solve_ls: bool,
    solve_ch: bool,
    osm_path: String,
    maxweight: String,
    maxheight: String,
    hgv: bool,
    solve_status: Option<Result<SolveResponse, anyhow::Error>>,
    solve_in_flight: bool,

    // Solutions tab state
    solution_id_input: String,
    solution_status: Option<Result<Box<SolutionBundle>, anyhow::Error>>,
    solution_export_status: Option<(bool, String)>,
    selected_route: usize,
    auto_solve_after_import: bool,
    open_map_after_solve: bool,

    // KPIs tab state
    kpis_id_input: String,
    kpis_status: Option<Result<Box<KpisBundle>, anyhow::Error>>,
    kpis_ui: KpisUiState,

    // Map tab state
    map_id_input: String,
    map_status: Option<Result<Box<SolutionBundle>, anyhow::Error>>,
    map_geometry: SolutionGeometryData,
    map_geometry_loading: bool,
    map_geometry_error: Option<String>,
    map_tiles: Option<walkers::HttpTiles>,
    map_memory: MapMemory,
    map_fit_pending: bool,
    hidden_routes: HashSet<usize>,
    show_map_labels: bool,
    selected_map_node: Option<domain::Node>,

    // Events tab state
    events: Vec<SolveEventDisplay>,
    max_events: usize,

    // Background message channel + egui repaint wake handle
    msg_rx: mpsc::Receiver<Msg>,
    msg_tx: mpsc::Sender<Msg>,
    repaint_ctx: Arc<OnceLock<egui::Context>>,

    // True once the background SSE task has been started. The task owns the
    // connection and reconnects with backoff, instead of polling on a timer.
    sse_started: bool,

    // Recent-problem/solution dropdowns, fetched from the list endpoints.
    recent_problems: Vec<ProblemSummary>,
    recent_solutions: Vec<SolutionSummary>,
    problems_error: Option<String>,
    solutions_error: Option<String>,
}

impl std::fmt::Debug for ConsoleApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleApp")
            .field("api_base_url", &self.api_base_url)
            .field("tab", &self.tab)
            .finish_non_exhaustive()
    }
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
        let default_import_path = discover_repo_path("data/demo/problem.json")
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "data/demo/problem.json".to_string());
        let default_osm_path = discover_repo_path("data/osm/nsw-latest.osm.pbf")
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let mut problem_builder = ProblemBuilderState::default();
        if let Some(gnaf_path) = discover_repo_path("data/gnaf.sqlite") {
            problem_builder.gnaf_db_path = gnaf_path.display().to_string();
        }
        let initial_json = problem_builder.build_validated_json().unwrap_or_default();
        let app = Self {
            runtime,
            client: ApiClient::new(&api_base_url),
            api_base_url,
            tab: Tab::Import,
            import_json: initial_json,
            import_path: default_import_path,
            import_status: None,
            problem_builder,
            builder_map_memory: MapMemory::default(),
            builder_map_fit_pending: true,
            geocode_client: reqwest::Client::new(),
            truck_ui: TruckLoadState::default(),
            problem_id_input: String::new(),
            solve_lns: false,
            solve_ls: true,
            solve_ch: false,
            osm_path: default_osm_path,
            maxweight: String::new(),
            maxheight: String::new(),
            hgv: false,
            solve_status: None,
            solve_in_flight: false,
            solution_id_input: String::new(),
            solution_status: None,
            solution_export_status: None,
            selected_route: 0,
            auto_solve_after_import: false,
            open_map_after_solve: false,
            kpis_id_input: String::new(),
            kpis_status: None,
            kpis_ui: KpisUiState::default(),
            map_id_input: String::new(),
            map_status: None,
            map_geometry: SolutionGeometryData::default(),
            map_geometry_loading: false,
            map_geometry_error: None,
            map_tiles: None,
            map_memory: MapMemory::default(),
            map_fit_pending: true,
            hidden_routes: HashSet::new(),
            show_map_labels: true,
            selected_map_node: None,
            events: Vec::new(),
            max_events: 50,
            msg_rx: rx,
            msg_tx: tx,
            repaint_ctx: Arc::new(OnceLock::new()),
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
        let repaint = Arc::clone(&self.repaint_ctx);
        let client = self.client.clone();
        self.runtime.spawn(async move {
            let problems = client.list_problems(20).await;
            send_msg(&tx, &repaint, Msg::ProblemsLoaded(problems));
        });
        let tx = self.msg_tx.clone();
        let repaint = Arc::clone(&self.repaint_ctx);
        let client = self.client.clone();
        self.runtime.spawn(async move {
            let solutions = client.list_solutions(None, 20).await;
            send_msg(&tx, &repaint, Msg::SolutionsLoaded(solutions));
        });
    }
}

impl eframe::App for ConsoleApp {
    // eframe 0.37 replaced `update(ctx, ..)` with `ui(ui, ..)`. The root Ui has
    // no frame of its own; the panel below supplies the standard background.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let _ = self.repaint_ctx.set(ui.ctx().clone());

        // Drain any messages from background tasks
        while let Ok(msg) = self.msg_rx.try_recv() {
            self.handle_msg(msg);
        }

        // Start the single background SSE task. It owns the connection and
        // reconnects with backoff, instead of polling on a fixed timer.
        if !self.sse_started {
            self.sse_started = true;
            let tx = self.msg_tx.clone();
            let repaint = Arc::clone(&self.repaint_ctx);
            let base_url = self.api_base_url.clone();
            self.runtime.spawn(sse_loop(base_url, tx, repaint));
        }

        egui::Panel::left("sidebar").show(ui, |ui| {
            self.render_sidebar(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            self.render_content(ui);
        });
    }
}

impl ConsoleApp {
    /// Render the left-hand navigation sidebar.
    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("TMS Control Tower");
        ui.weak("Orders · Cubing · Dispatch · ESG");
        ui.separator();

        self.tab_button(ui, "Orders & Network", Tab::Import);
        self.tab_button(ui, "Load Planner", Tab::Truck);
        self.tab_button(ui, "Optimizer", Tab::Solve);
        self.tab_button(ui, "Dispatch & BOL", Tab::Solutions);
        self.tab_button(ui, "Freight KPIs", Tab::Kpis);
        self.tab_button(ui, "Fleet Map", Tab::Map);
        self.tab_button(ui, "Control Tower", Tab::Events);

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
            Tab::Truck => self.render_truck_tab(ui),
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
                let auto_solve = std::mem::take(&mut self.auto_solve_after_import);
                match &res {
                    Ok(id) => {
                        self.problem_id_input.clone_from(id);
                        self.refresh_lists();
                        if auto_solve {
                            self.start_solve();
                        }
                    }
                    Err(_) => {
                        self.open_map_after_solve = false;
                    }
                }
                self.import_status = Some(res);
            }
            Msg::ProblemFetchedForBuilder(res) => match res {
                Ok(problem) => match serde_json::to_string_pretty(&problem) {
                    Ok(json) => {
                        if let Err(err) = self.problem_builder.load_from_json(&json) {
                            self.problem_builder.editor_error = Some(err.to_string());
                        } else {
                            self.import_json = json;
                            self.builder_map_fit_pending = true;
                            self.problem_builder.lookup_status =
                                Some(format!("Loaded problem '{}' into builder.", problem.name));
                        }
                    }
                    Err(err) => {
                        self.problem_builder.editor_error = Some(err.to_string());
                    }
                },
                Err(err) => {
                    self.problem_builder.editor_error = Some(format!("Loading problem: {err}"));
                }
            },
            Msg::GeocodeCompleted(res) => {
                self.problem_builder.lookup_in_flight = None;
                let auto_add = std::mem::take(&mut self.problem_builder.lookup_pending_add_stop);
                match res {
                    Ok(resp) => {
                        if resp.matches.is_empty() {
                            match resp.target {
                                GeocodeTarget::Depot => self.problem_builder.depot_matches.clear(),
                                GeocodeTarget::Stop => self.problem_builder.stop_matches.clear(),
                            }
                            self.problem_builder.lookup_status = Some(format!(
                                "No match found for '{}'. Try including suburb/city or enter \"lat, lon\".",
                                resp.query
                            ));
                        } else {
                            let top = resp.matches[0].clone();
                            let match_count = resp.matches.len();
                            match resp.target {
                                GeocodeTarget::Depot => {
                                    self.problem_builder.depot_matches = resp.matches;
                                }
                                GeocodeTarget::Stop => {
                                    self.problem_builder.stop_matches = resp.matches;
                                }
                            }
                            self.problem_builder.apply_geocode_match(resp.target, &top);
                            if auto_add && resp.target == GeocodeTarget::Stop {
                                if match_count == 1 {
                                    if let Err(err) = self.problem_builder.add_custom_stop_at(
                                        top.lat,
                                        top.lon,
                                        (top.source != "Coordinates").then_some(top.label),
                                    ) {
                                        self.problem_builder.editor_error = Some(err.to_string());
                                    }
                                } else {
                                    self.problem_builder.lookup_status = Some(format!(
                                        "Found {match_count} matches; top applied to preview. Confirm match or click '+ Add Place'."
                                    ));
                                }
                            }
                            self.builder_map_fit_pending = true;
                            self.sync_builder_json();
                        }
                    }
                    Err(err) => {
                        self.problem_builder.lookup_status = Some(format!("Lookup failed: {err}"));
                        self.problem_builder.editor_error = Some(err.to_string());
                    }
                }
            }
            Msg::SolutionSolved(res) => {
                // Pre-fill and preload Solutions, KPIs, and Map tabs with the new solution.
                self.solve_in_flight = false;
                let open_map = std::mem::take(&mut self.open_map_after_solve);
                if let Ok(resp) = &res {
                    self.solution_id_input.clone_from(&resp.id);
                    self.kpis_id_input.clone_from(&resp.id);
                    self.map_id_input.clone_from(&resp.id);
                    self.load_solution(resp.id.clone());
                    self.load_kpis(resp.id.clone());
                    self.load_map(resp.id.clone());
                    self.refresh_lists();
                    if open_map {
                        self.tab = Tab::Map;
                    }
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
            Msg::KpisFetched(res) => {
                self.kpis_ui.selected_route = None;
                self.kpis_ui.action_status = None;
                self.kpis_status = Some(res);
            }
            Msg::MapLoaded(res) => {
                if let Ok(bundle) = &res {
                    // Fit the map to the solution's extent on first paint;
                    // the handler does not know the map width yet, so the
                    // actual camera move happens in the map's render pass.
                    self.map_fit_pending = true;
                    self.hidden_routes.clear();
                    self.selected_map_node = None;
                    self.map_id_input.clone_from(&bundle.id);
                    // Fetch road geometry (empty if solved without an OSM
                    // extract); the map falls back to straight legs.
                    let tx = self.msg_tx.clone();
                    let repaint = Arc::clone(&self.repaint_ctx);
                    let client = self.client.clone();
                    let id = bundle.id.clone();
                    self.map_geometry = SolutionGeometryData::default();
                    self.map_geometry_loading = true;
                    self.map_geometry_error = None;
                    self.runtime.spawn(async move {
                        let result = client.get_solution_geometry_detail(&id).await;
                        send_msg(&tx, &repaint, Msg::MapGeometryLoaded(result));
                    });
                } else {
                    self.map_geometry_loading = false;
                }
                self.map_status = Some(res);
            }
            Msg::MapGeometryLoaded(res) => {
                self.map_geometry_loading = false;
                match res {
                    Ok(geom) => {
                        self.map_geometry_error = None;
                        if geom.routes.iter().any(|r| !r.is_empty()) {
                            self.map_fit_pending = true;
                        }
                        self.map_geometry = geom;
                    }
                    Err(e) => {
                        self.map_geometry = SolutionGeometryData::default();
                        self.map_geometry_error = Some(format!("{e}"));
                    }
                }
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
        egui::ScrollArea::vertical()
            .id_salt("import_tab_scroll")
            .show(ui, |ui| {
                ui.heading("Orders, Hubs & Fleet Network");
                ui.label(
                    "Load an existing order network, configure depots/consignments/vehicles & rate cards below, or import JSON.",
                );

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.strong("Set active problem for Solve:");
                    if recent_problems_dropdown(
                        ui,
                        "import_recent_problems",
                        &self.recent_problems,
                        &mut self.problem_id_input,
                    ) {
                        self.refresh_lists();
                    }
                    if !self.problem_id_input.trim().is_empty() {
                        if ui
                            .button("Load into Builder")
                            .on_hover_text(
                                "Fetch this problem from the API into the visual builder for editing.",
                            )
                            .clicked()
                        {
                            self.load_problem_into_builder(self.problem_id_input.clone());
                        }
                        if ui.button("Go to Solve").clicked() {
                            self.tab = Tab::Solve;
                        }
                    }
                });

                ui.add_space(8.0);
                let browse_dir = self.browse_dir();
                let action = render_builder_controls(
                    ui,
                    &mut self.problem_builder,
                    &mut self.map_tiles,
                    &mut self.builder_map_memory,
                    &mut self.builder_map_fit_pending,
                    &browse_dir,
                );
                self.handle_builder_action(action);

                ui.add_space(6.0);
                egui::CollapsingHeader::new("File Import / Raw Problem JSON")
                    .id_salt("raw_problem_json_section")
                    .default_open(false)
                    .show(ui, |ui| {
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
                        ui.label("…or inspect/paste a problem JSON document (`domain::Problem`):");

                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .id_salt("import_scroll")
                            .max_height(260.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.import_json)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(10),
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
                    });

                if self.render_import_status(ui) {
                    self.tab = Tab::Solve;
                }
            });
    }

    fn handle_builder_action(&mut self, action: BuilderAction) {
        match action {
            BuilderAction::None => {}
            BuilderAction::SyncToJson => self.sync_builder_json(),
            BuilderAction::LookupAddress {
                target,
                query,
                add_stop_if_single,
            } => self.spawn_geocode_lookup(target, query, add_stop_if_single),
            BuilderAction::PopulateScenario => match self.problem_builder.generate_scenario() {
                Ok(_) => {
                    self.builder_map_fit_pending = true;
                    self.sync_builder_json();
                }
                Err(err) => {
                    self.problem_builder.editor_error = Some(err.to_string());
                }
            },
            BuilderAction::GenerateAndImport => match self.problem_builder.generate_scenario() {
                Ok(_) => {
                    self.builder_map_fit_pending = true;
                    self.import_validated_draft();
                }
                Err(err) => {
                    self.problem_builder.editor_error = Some(err.to_string());
                }
            },
            BuilderAction::NewCustom => match self.problem_builder.reset_custom_template() {
                Ok(_) => {
                    self.builder_map_fit_pending = true;
                    self.sync_builder_json();
                }
                Err(err) => {
                    self.problem_builder.editor_error = Some(err.to_string());
                }
            },
            BuilderAction::LoadFromJson => {
                if let Err(err) = self.problem_builder.load_from_json(&self.import_json) {
                    self.problem_builder.editor_error = Some(err.to_string());
                } else {
                    self.builder_map_fit_pending = true;
                }
            }
            BuilderAction::ImportDraft => self.import_validated_draft(),
            BuilderAction::ImportAndSolve => {
                self.auto_solve_after_import = true;
                self.open_map_after_solve = true;
                self.import_validated_draft();
            }
            BuilderAction::OpenTruckOptimizer(veh_idx) => {
                if let Some(idx) = veh_idx {
                    self.truck_ui.selected_vehicle_idx = idx;
                }
                self.truck_ui.target_source = LoadTargetSource::DraftVehicle;
                self.tab = Tab::Truck;
            }
            BuilderAction::SaveJson => {
                let json_result = if self.import_json.trim().is_empty() {
                    self.problem_builder.build_validated_json()
                } else {
                    Ok(self.import_json.clone())
                };
                match json_result {
                    Ok(contents) => {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("Save problem JSON")
                            .add_filter("JSON", &["json"])
                            .set_directory(self.browse_dir())
                            .set_file_name("problem.json")
                            .save_file()
                        {
                            match std::fs::write(&path, contents) {
                                Ok(()) => {
                                    self.import_path = path.display().to_string();
                                    self.problem_builder.editor_error = None;
                                }
                                Err(err) => {
                                    self.problem_builder.editor_error =
                                        Some(format!("saving {}: {err}", path.display()));
                                }
                            }
                        }
                    }
                    Err(err) => {
                        self.problem_builder.editor_error = Some(err.to_string());
                    }
                }
            }
        }
    }

    fn spawn_geocode_lookup(
        &mut self,
        target: GeocodeTarget,
        query: String,
        add_stop_if_single: bool,
    ) {
        let trimmed = query.trim().to_string();
        if trimmed.is_empty() {
            self.problem_builder.lookup_status =
                Some("Enter an address or \"lat, lon\" coordinates first.".to_string());
            return;
        }
        self.problem_builder.editor_error = None;
        self.problem_builder.lookup_pending_add_stop = add_stop_if_single;

        // Offline coordinate input resolves instantaneously without background work.
        if let Some((lat, lon)) = crate::geocode::parse_lat_lon_pair(&trimmed) {
            let matched = GeocodeMatch {
                label: format!("{lat:.5}, {lon:.5}"),
                lat,
                lon,
                source: "Coordinates".to_string(),
            };
            self.problem_builder.lookup_in_flight = None;
            self.problem_builder.lookup_pending_add_stop = false;
            match target {
                GeocodeTarget::Depot => self.problem_builder.depot_matches.clear(),
                GeocodeTarget::Stop => self.problem_builder.stop_matches.clear(),
            }
            self.problem_builder.apply_geocode_match(target, &matched);
            if add_stop_if_single
                && target == GeocodeTarget::Stop
                && let Err(err) = self.problem_builder.add_custom_stop_at(lat, lon, None)
            {
                self.problem_builder.editor_error = Some(err.to_string());
            }
            self.builder_map_fit_pending = true;
            self.sync_builder_json();
            return;
        }

        self.problem_builder.lookup_in_flight = Some(target);
        self.problem_builder.lookup_status = Some(format!("Looking up '{trimmed}'…"));
        let tx = self.msg_tx.clone();
        let repaint = Arc::clone(&self.repaint_ctx);
        let client = self.geocode_client.clone();
        let gnaf = (!self.problem_builder.gnaf_db_path.trim().is_empty())
            .then(|| PathBuf::from(self.problem_builder.gnaf_db_path.trim()));
        self.runtime.spawn(async move {
            let result = crate::geocode::search_address(client, gnaf, target, trimmed).await;
            send_msg(&tx, &repaint, Msg::GeocodeCompleted(result));
        });
    }

    fn import_validated_draft(&mut self) {
        match self.problem_builder.build_validated_json() {
            Ok(json) => {
                self.import_json = json.clone();
                self.problem_builder.editor_error = None;
                self.spawn_import(json);
            }
            Err(err) => {
                self.auto_solve_after_import = false;
                self.open_map_after_solve = false;
                self.problem_builder.editor_error = Some(err.to_string());
            }
        }
    }

    fn load_problem_into_builder(&self, id: String) {
        let id = id.trim().to_string();
        if id.is_empty() {
            return;
        }
        let client = self.client.clone();
        let tx = self.msg_tx.clone();
        let repaint = Arc::clone(&self.repaint_ctx);
        self.runtime.spawn(async move {
            let result = async {
                let raw = client.get_problem(&id).await?;
                from_json::<domain::Problem>(&raw).map(Box::new)
            }
            .await;
            send_msg(&tx, &repaint, Msg::ProblemFetchedForBuilder(result));
        });
    }

    fn sync_builder_json(&mut self) {
        match self.problem_builder.build_validated_json() {
            Ok(json) => {
                self.import_json = json;
                self.problem_builder.editor_error = None;
            }
            Err(err) => {
                self.problem_builder.editor_error = Some(err.to_string());
            }
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
    fn browse_dir(&self) -> PathBuf {
        let current = PathBuf::from(&self.import_path);
        if let Some(parent) = current
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty() && parent.exists())
        {
            return parent.to_path_buf();
        }
        if let Some(demo) = discover_repo_path("data/demo")
            && demo.is_dir()
        {
            return demo;
        }
        PathBuf::from(".")
    }

    fn osm_browse_dir(&self) -> PathBuf {
        let current = PathBuf::from(&self.osm_path);
        if let Some(parent) = current
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty() && parent.exists())
        {
            return parent.to_path_buf();
        }
        if let Some(osm) = discover_repo_path("data/osm")
            && osm.is_dir()
        {
            return osm;
        }
        self.browse_dir()
    }

    fn start_import_from_file(&mut self) {
        let path = self.import_path.trim().to_string();
        self.import_status = None;
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                self.import_json = contents.clone();
                let _ = self.problem_builder.load_from_json(&contents);
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
        ui.heading("Route Optimizer & Road Compliance");
        ui.label("Select an imported network and choose solver & HGV road legality options:");

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
        ui.add_enabled_ui(!self.solve_lns, |ui| {
            ui.checkbox(&mut self.solve_ls, "Use local search (when LNS is off)");
        });
        ui.checkbox(
            &mut self.solve_ch,
            "Use Contraction Hierarchies for OSM matrix",
        );

        ui.add_space(8.0);
        ui.label("OSM PBF path (optional; leave blank for straight-line haversine):");
        ui.horizontal(|ui| {
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose an OSM .osm.pbf extract")
                    .add_filter("OSM PBF", &["pbf"])
                    .set_directory(self.osm_browse_dir())
                    .pick_file()
            {
                self.osm_path = path.display().to_string();
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.osm_path)
                    .desired_width(360.0)
                    .hint_text("data/osm/nsw-latest.osm.pbf"),
            );
            if !self.osm_path.is_empty() && ui.button("Clear").clicked() {
                self.osm_path.clear();
            }
        });

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
        let maxweight = match parse_optional_positive(&self.maxweight, "Max weight") {
            Ok(val) => val,
            Err(err) => {
                self.solve_status = Some(Err(err));
                return;
            }
        };
        let maxheight = match parse_optional_positive(&self.maxheight, "Max height") {
            Ok(val) => val,
            Err(err) => {
                self.solve_status = Some(Err(err));
                return;
            }
        };
        let req = SolveRequest {
            problem_id: self.problem_id_input.trim().to_string(),
            lns: self.solve_lns,
            local_search: self.solve_ls,
            ch: self.solve_ch,
            osm_path: non_empty(self.osm_path.trim()),
            maxweight,
            maxheight,
            hgv: self.hgv,
        };
        let tx = self.msg_tx.clone();
        let repaint = Arc::clone(&self.repaint_ctx);
        let client = self.client.clone();
        self.solve_status = None;
        self.solve_in_flight = true;
        self.runtime.spawn(async move {
            let res = client.solve(&req).await;
            send_msg(&tx, &repaint, Msg::SolutionSolved(res));
        });
    }

    fn load_solution(&mut self, id: String) {
        let id = id.trim().to_string();
        self.solution_id_input.clone_from(&id);
        let client = self.client.clone();
        let tx = self.msg_tx.clone();
        let repaint = Arc::clone(&self.repaint_ctx);
        self.solution_status = None;
        self.solution_export_status = None;
        self.selected_route = 0;
        self.runtime.spawn(async move {
            let result = fetch_solution_bundle(client, id).await;
            send_msg(&tx, &repaint, Msg::SolutionFetched(result));
        });
    }

    fn load_kpis(&mut self, id: String) {
        let id = id.trim().to_string();
        self.kpis_id_input.clone_from(&id);
        let tx = self.msg_tx.clone();
        let repaint = Arc::clone(&self.repaint_ctx);
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
            send_msg(&tx, &repaint, Msg::KpisFetched(result));
        });
    }

    fn load_map(&mut self, id: String) {
        let id = id.trim().to_string();
        self.map_id_input.clone_from(&id);
        let tx = self.msg_tx.clone();
        let repaint = Arc::clone(&self.repaint_ctx);
        let client = self.client.clone();
        self.map_status = None;
        self.map_geometry = SolutionGeometryData::default();
        self.map_geometry_loading = false;
        self.map_geometry_error = None;
        self.map_fit_pending = true;
        self.hidden_routes.clear();
        self.runtime.spawn(async move {
            let result = fetch_solution_bundle(client, id).await;
            send_msg(&tx, &repaint, Msg::MapLoaded(result));
        });
    }

    /// Render the solve result. Returns the tab the operator asked to open.
    fn render_solve_status(&mut self, ui: &mut egui::Ui) -> Option<Tab> {
        let resp = match self.solve_status.as_ref()? {
            Ok(resp) => resp.clone(),
            Err(e) => {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
                return None;
            }
        };
        ui.add_space(8.0);
        let mut next_tab = None;
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                egui::Color32::GREEN,
                format!(
                    "Solved {}: {} of {} served, {:.1} km",
                    resp.id, resp.served, resp.total, resp.distance_km
                ),
            );
            copy_button(ui, "Copy solution ID", &resp.id);
            if ui.button("View Solution").clicked() {
                self.load_solution(resp.id.clone());
                next_tab = Some(Tab::Solutions);
            }
            if ui.button("View KPIs").clicked() {
                self.load_kpis(resp.id.clone());
                next_tab = Some(Tab::Kpis);
            }
            if ui.button("View Route Map").clicked() {
                self.load_map(resp.id.clone());
                next_tab = Some(Tab::Map);
            }
        });
        next_tab
    }

    // ── Solutions tab ───────────────────────────────────────────────────

    fn render_solutions_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Dispatch Board, Manifests & BOL");
        ui.label("Manage live route stop sequences, check Chain of Responsibility (CoR) compliance, and export BOL/run sheets:");

        ui.add_space(8.0);
        ui.label("Solution ID:");
        let prev_solution_id = self.solution_id_input.clone();
        if solutions_picker(
            ui,
            "recent_solutions_solutions",
            &self.recent_solutions,
            &self.solutions_error,
            &mut self.solution_id_input,
        ) {
            self.refresh_lists();
        }
        if self.solution_id_input != prev_solution_id && !self.solution_id_input.trim().is_empty() {
            self.load_solution(self.solution_id_input.clone());
        }
        ui.text_edit_singleline(&mut self.solution_id_input);

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Fetch Solution").clicked() {
                self.load_solution(self.solution_id_input.clone());
            }
        });

        ui.add_space(8.0);
        let view_action = match self.solution_status.as_ref() {
            Some(Ok(bundle)) => Self::render_solution_view(
                ui,
                bundle,
                &mut self.selected_route,
                &self.solution_export_status,
            ),
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
                SolutionViewAction::None
            }
            None => SolutionViewAction::None,
        };
        match &view_action {
            SolutionViewAction::None => {}
            SolutionViewAction::OpenTruckLoad(route_idx) => {
                self.truck_ui.selected_route_idx = *route_idx;
                self.truck_ui.target_source = LoadTargetSource::SolvedRoute;
                self.tab = Tab::Truck;
            }
            SolutionViewAction::CopyManifest => {
                if let Some(Ok(bundle)) = self.solution_status.as_ref() {
                    let text = format_route_manifest_text(bundle);
                    ui.ctx().copy_text(text);
                    self.solution_export_status =
                        Some((false, "Copied route manifest to clipboard.".to_string()));
                }
            }
            SolutionViewAction::ExportScheduleCsv => {
                let export_payload = self
                    .solution_status
                    .as_ref()
                    .and_then(|res| res.as_ref().ok())
                    .map(|bundle| (bundle.id.clone(), format_route_schedule_csv(bundle)));
                if let Some((solution_id, csv)) = export_payload
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title("Export route schedule CSV")
                        .add_filter("CSV", &["csv"])
                        .set_directory(self.browse_dir())
                        .set_file_name(format!("routes-{solution_id}.csv"))
                        .save_file()
                {
                    self.solution_export_status = Some(match std::fs::write(&path, csv) {
                        Ok(()) => (false, format!("Saved schedule CSV to {}", path.display())),
                        Err(err) => (true, format!("Saving CSV failed: {err}")),
                    });
                }
            }
            SolutionViewAction::ExportRunSheetHtml => {
                let export_payload = self
                    .solution_status
                    .as_ref()
                    .and_then(|res| res.as_ref().ok())
                    .map(|bundle| (bundle.id.clone(), format_driver_run_sheet_html(bundle)));
                if let Some((solution_id, html)) = export_payload
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title("Export Driver Run Sheet & BOL (HTML)")
                        .add_filter("HTML", &["html"])
                        .set_directory(self.browse_dir())
                        .set_file_name(format!("runsheet-bol-{solution_id}.html"))
                        .save_file()
                {
                    self.solution_export_status = Some(match std::fs::write(&path, html) {
                        Ok(()) => (
                            false,
                            format!("Saved Run Sheet / BOL to {}", path.display()),
                        ),
                        Err(err) => (true, format!("Saving Run Sheet failed: {err}")),
                    });
                }
            }
            SolutionViewAction::MoveStop { .. }
            | SolutionViewAction::UnassignStop { .. }
            | SolutionViewAction::AssignStoreToRoute { .. } => {
                let edited_bundle = self
                    .solution_status
                    .as_ref()
                    .and_then(|res| res.as_ref().ok())
                    .map(|bundle| (**bundle).clone());
                if let Some(mut edited_bundle) = edited_bundle
                    && let Some(msg) =
                        apply_dispatch_edit_to_bundle(&mut edited_bundle, &view_action)
                {
                    self.sync_bundle_after_dispatch_edit(edited_bundle, msg);
                }
            }
        }
    }

    fn render_truck_tab(&mut self, ui: &mut egui::Ui) {
        let action = {
            let bundle = self
                .solution_status
                .as_ref()
                .and_then(|res| res.as_ref().ok())
                .map(|b| &**b)
                .or_else(|| {
                    self.kpis_status
                        .as_ref()
                        .and_then(|res| res.as_ref().ok())
                        .map(|kb| &kb.detail)
                })
                .or_else(|| {
                    self.map_status
                        .as_ref()
                        .and_then(|res| res.as_ref().ok())
                        .map(|b| &**b)
                });
            render_truck_load_tab(ui, &mut self.truck_ui, &self.problem_builder.draft, bundle)
        };
        if let Some(action) = action {
            match action {
                TruckLoadAction::SetDraftVehicleCapacity {
                    vehicle_idx,
                    capacity,
                } => {
                    if let Some(veh) = self.problem_builder.draft.vehicles.get_mut(vehicle_idx) {
                        veh.capacity = capacity;
                        let name = veh.name.clone();
                        self.sync_builder_json();
                        self.truck_ui.status_banner = Some(format!(
                            "Updated draft V{vehicle_idx} ({name}) capacity to A:{} C:{} F:{} ({} total units).",
                            capacity.ambient,
                            capacity.chilled,
                            capacity.frozen,
                            capacity.total()
                        ));
                    }
                }
                TruckLoadAction::SetAllDraftVehicleCapacities { capacity } => {
                    let count = self.problem_builder.draft.vehicles.len();
                    for veh in &mut self.problem_builder.draft.vehicles {
                        veh.capacity = capacity;
                    }
                    self.sync_builder_json();
                    self.truck_ui.status_banner = Some(format!(
                        "Updated all {count} draft vehicle(s) to capacity A:{} C:{} F:{} ({} total units).",
                        capacity.ambient,
                        capacity.chilled,
                        capacity.frozen,
                        capacity.total()
                    ));
                }
                TruckLoadAction::SyncSolverLimits {
                    max_weight_tonnes,
                    max_height_m,
                } => {
                    self.hgv = true;
                    self.maxweight = format!("{max_weight_tonnes:.1}");
                    self.maxheight = format!("{max_height_m:.2}");
                    self.truck_ui.status_banner = Some(format!(
                        "Synced Solve tab road limits: HGV = ON, maxweight = {:.1} t, maxheight = {:.2} m.",
                        max_weight_tonnes, max_height_m
                    ));
                }
            }
        }
    }

    /// Render the structured solution view: summary strip, route table with a
    /// detailed per-route timeline, unassigned store diagnostics, and a collapsible
    /// raw JSON debug section.
    fn render_solution_view(
        ui: &mut egui::Ui,
        bundle: &SolutionBundle,
        selected_route: &mut usize,
        export_status: &Option<(bool, String)>,
    ) -> SolutionViewAction {
        let mut action = SolutionViewAction::None;
        ui.separator();

        // Summary strip of stat cards.
        ui.horizontal_wrapped(|ui| {
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
            if ui.button("Copy Route Manifest").clicked() {
                action = SolutionViewAction::CopyManifest;
            }
            if ui.button("Export Schedule CSV…").clicked() {
                action = SolutionViewAction::ExportScheduleCsv;
            }
            if ui.button("Export Run Sheet / BOL (HTML)…").clicked() {
                action = SolutionViewAction::ExportRunSheetHtml;
            }
        });
        ui.weak(format!("problem: {}", bundle.problem.name));
        match bundle.solution.validate(&bundle.problem) {
            Ok(()) => {
                ui.colored_label(
                    egui::Color32::from_rgb(44, 160, 72),
                    "✓ CoR & Schedule Compliant — 0 violations (capacity, receiving windows, driver shifts & NHVR fatigue verified)",
                );
            }
            Err(violations) => {
                let preview = violations
                    .iter()
                    .take(3)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" | ");
                ui.colored_label(
                    egui::Color32::from_rgb(214, 55, 55),
                    format!(
                        "⚠ {} CoR / Schedule violation(s): {preview}",
                        violations.len()
                    ),
                );
            }
        }
        if let Some((is_err, msg)) = export_status {
            let color = if *is_err {
                egui::Color32::RED
            } else {
                egui::Color32::GREEN
            };
            ui.colored_label(color, msg);
        }

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
        let safe_route_idx = (*selected_route).min(bundle.solution.routes.len().saturating_sub(1));
        if let Some(route) = bundle.solution.routes.get(safe_route_idx) {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!(
                    "Route {} timeline ({})",
                    safe_route_idx + 1,
                    vehicle_name(&bundle.problem, route)
                ));
                if ui.small_button("View Truck Load").clicked() {
                    action = SolutionViewAction::OpenTruckLoad(safe_route_idx);
                }
            });
            egui::ScrollArea::horizontal()
                .id_salt("route_timeline_hscroll")
                .show(ui, |ui| {
                    egui::Grid::new("route_timeline_grid")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("#");
                            ui.strong("Seq");
                            ui.strong("Stop");
                            ui.strong("Address / Coords");
                            ui.strong("Demand (A/C/F)");
                            ui.strong("Arrive");
                            ui.strong("Wait / Svc");
                            ui.strong("Depart");
                            ui.strong("Dispatch");
                            ui.end_row();
                            let mut seq = 0usize;
                            for (index, stop) in route.steps.iter().enumerate() {
                                let seq_opt = if matches!(stop.node, domain::Node::Store(_)) {
                                    seq += 1;
                                    Some(seq)
                                } else {
                                    None
                                };
                                let detail = step_row_details(&bundle.problem, stop, seq_opt);
                                ui.label((index + 1).to_string());
                                ui.label(
                                    seq_opt
                                        .map(|s| format!("#{s}"))
                                        .unwrap_or_else(|| "—".to_string()),
                                );
                                ui.label(&detail.label);
                                ui.weak(&detail.location_text);
                                ui.label(&detail.demand_text);
                                ui.label(stop.arrive.to_string());
                                ui.label(&detail.wait_svc_text);
                                ui.label(stop.depart.to_string());
                                if matches!(stop.node, domain::Node::Store(_)) {
                                    let can_up = index > 0
                                        && matches!(
                                            route.steps.get(index - 1).map(|s| s.node),
                                            Some(domain::Node::Store(_))
                                        );
                                    let can_down = matches!(
                                        route.steps.get(index + 1).map(|s| s.node),
                                        Some(domain::Node::Store(_))
                                    );
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(can_up, egui::Button::new("▲").small())
                                            .on_hover_text("Move stop earlier in route")
                                            .clicked()
                                        {
                                            action = SolutionViewAction::MoveStop {
                                                route_idx: safe_route_idx,
                                                step_idx: index,
                                                delta: -1,
                                            };
                                        }
                                        if ui
                                            .add_enabled(can_down, egui::Button::new("▼").small())
                                            .on_hover_text("Move stop later in route")
                                            .clicked()
                                        {
                                            action = SolutionViewAction::MoveStop {
                                                route_idx: safe_route_idx,
                                                step_idx: index,
                                                delta: 1,
                                            };
                                        }
                                        if ui
                                            .small_button("Unassign")
                                            .on_hover_text("Remove stop from route and recalculate")
                                            .clicked()
                                        {
                                            action = SolutionViewAction::UnassignStop {
                                                route_idx: safe_route_idx,
                                                step_idx: index,
                                            };
                                        }
                                    });
                                } else {
                                    ui.weak("—");
                                }
                                ui.end_row();
                            }
                        });
                });
        }

        // Unassigned stores, if any, with root-cause diagnostics and quick assign.
        if !bundle.solution.unassigned.is_empty() {
            ui.add_space(8.0);
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 0),
                format!("Unassigned ({}):", bundle.solution.unassigned.len()),
            );
            for &store_id in &bundle.solution.unassigned {
                let reason = bundle
                    .problem
                    .store(store_id)
                    .map(|store| diagnose_unassigned_store(&bundle.problem, store))
                    .unwrap_or_default();
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "• {} — {reason}",
                        store_name(&bundle.problem, store_id)
                    ));
                    if !bundle.solution.routes.is_empty()
                        && ui
                            .small_button(format!("+ Assign to Route {}", safe_route_idx + 1))
                            .clicked()
                    {
                        action = SolutionViewAction::AssignStoreToRoute {
                            store_id,
                            route_idx: safe_route_idx,
                        };
                    }
                });
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
        action
    }

    // ── KPIs tab ────────────────────────────────────────────────────────

    fn render_kpis_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Freight, OTIF & Sustainability KPIs");
        ui.label("Explore freight cost per km/stop/unit, OTIF compliance, dock detention wait, diesel/CO₂e & fleet utilization:");

        ui.add_space(6.0);
        ui.label("Solution ID:");
        let prev_kpis_id = self.kpis_id_input.clone();
        if solutions_picker(
            ui,
            "recent_solutions_kpis",
            &self.recent_solutions,
            &self.solutions_error,
            &mut self.kpis_id_input,
        ) {
            self.refresh_lists();
        }
        if self.kpis_id_input != prev_kpis_id && !self.kpis_id_input.trim().is_empty() {
            self.load_kpis(self.kpis_id_input.clone());
        }
        ui.horizontal_wrapped(|ui| {
            ui.text_edit_singleline(&mut self.kpis_id_input);
            if ui.button("Fetch KPIs").clicked() {
                self.load_kpis(self.kpis_id_input.clone());
            }
            if let Some(latest) = self
                .recent_solutions
                .iter()
                .max_by(|a, b| a.solved_at.cmp(&b.solved_at))
                .map(|s| s.id.clone())
                && ui.button("Load Latest").clicked()
            {
                self.load_kpis(latest);
            }
        });

        ui.add_space(6.0);
        let action = match &self.kpis_status {
            Some(Ok(bundle)) => {
                render_kpis_dashboard(ui, bundle, &self.recent_solutions, &mut self.kpis_ui)
            }
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
                None
            }
            None => None,
        };

        if let Some(action) = action {
            match action {
                KpisAction::OpenMap { solution_id } => {
                    self.load_map(solution_id);
                    self.tab = Tab::Map;
                }
                KpisAction::OpenSolution {
                    solution_id,
                    route_index,
                } => {
                    let already_loaded = matches!(
                        &self.solution_status,
                        Some(Ok(existing)) if existing.id == solution_id
                    );
                    if !already_loaded {
                        self.load_solution(solution_id);
                    }
                    if let Some(idx) = route_index {
                        self.selected_route = idx;
                    }
                    self.tab = Tab::Solutions;
                }
            }
        }
    }

    // ── Map tab ──────────────────────────────────────────────────────

    fn render_map_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Fleet Route & Corridor Map");
        ui.label("Inspect sequenced consignments, road polylines, and vehicle corridors on OpenStreetMap.");

        ui.add_space(8.0);
        ui.label("Solution ID:");
        let prev_map_id = self.map_id_input.clone();
        if recent_solutions_dropdown(
            ui,
            "recent_solutions_map",
            &self.recent_solutions,
            &mut self.map_id_input,
        ) {
            self.refresh_lists();
        }
        if self.map_id_input != prev_map_id && !self.map_id_input.trim().is_empty() {
            self.load_map(self.map_id_input.clone());
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
                self.load_map(self.map_id_input.clone());
            }
        });

        ui.add_space(8.0);
        if let Some(res) = &self.map_status {
            match res {
                Ok(bundle) => {
                    let bundle = bundle.clone();
                    if let Some(route_idx) = self.render_map_body(ui, &bundle) {
                        self.truck_ui.selected_route_idx = route_idx;
                        self.truck_ui.target_source = LoadTargetSource::SolvedRoute;
                        self.tab = Tab::Truck;
                    }
                }
                Err(e) => {
                    ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
                }
            }
        } else {
            ui.weak("(no solution loaded - pick one above and click Load Map)");
        }
    }
}

/// Action emitted by the Solutions structured view.
enum SolutionViewAction {
    None,
    OpenTruckLoad(usize),
    CopyManifest,
    ExportScheduleCsv,
    ExportRunSheetHtml,
    MoveStop {
        route_idx: usize,
        step_idx: usize,
        delta: isize,
    },
    UnassignStop {
        route_idx: usize,
        step_idx: usize,
    },
    AssignStoreToRoute {
        store_id: domain::StoreId,
        route_idx: usize,
    },
}

/// Action emitted by the problem configuration panel inside the Import tab.
#[derive(Clone, PartialEq, Eq)]
enum BuilderAction {
    None,
    SyncToJson,
    LookupAddress {
        target: GeocodeTarget,
        query: String,
        add_stop_if_single: bool,
    },
    PopulateScenario,
    GenerateAndImport,
    NewCustom,
    LoadFromJson,
    ImportDraft,
    ImportAndSolve,
    OpenTruckOptimizer(Option<usize>),
    SaveJson,
}

fn render_builder_controls(
    ui: &mut egui::Ui,
    state: &mut ProblemBuilderState,
    map_tiles: &mut Option<walkers::HttpTiles>,
    map_memory: &mut MapMemory,
    map_fit_pending: &mut bool,
    browse_dir: &std::path::Path,
) -> BuilderAction {
    let mut action = BuilderAction::None;
    let mut edited = false;

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Set Up Route (Custom Depot & Places)");
            ui.separator();
            ui.label("Route name:");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut state.draft.name)
                        .desired_width(180.0)
                        .hint_text("Custom Route"),
                )
                .changed()
            {
                state.custom_name = state.draft.name.clone();
                edited = true;
            }
            if ui
                .button(egui::RichText::new("Solve Route Now").strong())
                .on_hover_text(
                    "Import this route, solve it with local search, and open the Route Map.",
                )
                .clicked()
            {
                action = BuilderAction::ImportAndSolve;
            }
            if ui.button("Import & Use Route").clicked() {
                action = BuilderAction::ImportDraft;
            }
            if ui.button("New Blank Route").clicked() {
                action = BuilderAction::NewCustom;
            }
            if ui.button("Save JSON…").clicked() {
                action = BuilderAction::SaveJson;
            }
            if ui.small_button("Load JSON Box -> Editor").clicked() {
                action = BuilderAction::LoadFromJson;
            }
        });

        if let Some(err) = &state.editor_error {
            ui.colored_label(egui::Color32::RED, err);
        }

        ui.add_space(4.0);
        egui::CollapsingHeader::new(format!(
            "Depot & Places Builder — {} ({} depots · {} places · {} vehicles)",
            state.draft.name,
            state.draft.depots.len(),
            state.draft.stores.len(),
            state.draft.vehicles.len()
        ))
        .id_salt("custom_route_builder_header")
        .default_open(true)
        .show(ui, |ui| {
            render_depot_quick_setup(ui, state, &mut action, map_fit_pending, &mut edited);
            ui.separator();
            render_stop_quick_add(ui, state, &mut action, map_fit_pending, &mut edited);
            ui.add_space(4.0);

            ui.horizontal_wrapped(|ui| {
                if state.lookup_in_flight.is_some() {
                    ui.spinner();
                }
                if let Some(status) = &state.lookup_status {
                    ui.weak(status);
                }
            });

            ui.horizontal_wrapped(|ui| {
                ui.label("Local G-NAF DB (optional):");
                ui.add(
                    egui::TextEdit::singleline(&mut state.gnaf_db_path)
                        .desired_width(240.0)
                        .hint_text("data/gnaf.sqlite (falls back to OpenStreetMap)"),
                );
                if ui.small_button("Browse DB…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title("Choose an imported G-NAF SQLite DB")
                        .add_filter("SQLite", &["sqlite", "db"])
                        .set_directory(browse_dir)
                        .pick_file()
                {
                    state.gnaf_db_path = path.display().to_string();
                }
            });

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .checkbox(&mut state.show_map_picker, "Interactive Map Picker")
                    .changed()
                    && state.show_map_picker
                {
                    *map_fit_pending = true;
                }
                if state.show_map_picker {
                    ui.separator();
                    ui.label("Click action:");
                    ui.selectable_value(
                        &mut state.map_pick_mode,
                        MapPickMode::AddStop,
                        "Add Place",
                    );
                    ui.selectable_value(
                        &mut state.map_pick_mode,
                        MapPickMode::SetDepot,
                        "Set Main Depot",
                    );
                    ui.selectable_value(
                        &mut state.map_pick_mode,
                        MapPickMode::PanZoom,
                        "Pan / Zoom",
                    );
                    if ui.small_button("Fit Draft").clicked() {
                        *map_fit_pending = true;
                    }
                }
            });

            if state.show_map_picker {
                render_builder_map_picker(
                    ui,
                    state,
                    map_tiles,
                    map_memory,
                    map_fit_pending,
                    &mut edited,
                );
            }

            ui.separator();
            render_draft_tables(ui, state, &mut action, &mut edited);
        });

        ui.add_space(4.0);
        render_synthetic_generator(ui, state, &mut action);
    });

    if edited && action == BuilderAction::None {
        action = BuilderAction::SyncToJson;
    }
    action
}

fn render_depot_quick_setup(
    ui: &mut egui::Ui,
    state: &mut ProblemBuilderState,
    action: &mut BuilderAction,
    map_fit_pending: &mut bool,
    edited: &mut bool,
) {
    ui.strong("Main Depot (Start / Return Location)");
    let mut coords_or_text_submit = false;
    let mut depot_name_missing = state.draft.depots.is_empty();
    if !depot_name_missing {
        let mut lat = state.draft.depots[0].location.lat;
        let mut lon = state.draft.depots[0].location.lon;
        ui.horizontal_wrapped(|ui| {
            ui.label("Name:");
            *edited |= ui
                .add(
                    egui::TextEdit::singleline(&mut state.draft.depots[0].name)
                        .desired_width(130.0)
                        .hint_text("Main Depot"),
                )
                .changed();
            ui.label("Address / Coords:");
            let addr_resp = ui.add(
                egui::TextEdit::singleline(&mut state.depot_query)
                    .desired_width(250.0)
                    .hint_text("e.g. 100 George St Sydney OR -33.8688, 151.2093"),
            );
            if addr_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                coords_or_text_submit = true;
            }
            if ui.button("Lookup").clicked() || coords_or_text_submit {
                *action = BuilderAction::LookupAddress {
                    target: GeocodeTarget::Depot,
                    query: state.depot_query.clone(),
                    add_stop_if_single: false,
                };
            }
            if edit_lat_lon(ui, &mut lat, &mut lon) {
                state.set_main_depot_location(lat, lon, state.draft.depots[0].address.clone());
                *map_fit_pending = true;
                *edited = true;
            }
            if ui.small_button("Pick on Map").clicked() {
                state.show_map_picker = true;
                state.map_pick_mode = MapPickMode::SetDepot;
                *map_fit_pending = true;
            }
            if ui.small_button("+ Extra Depot").clicked() {
                state.add_depot();
                state.section = EditorSection::Depots;
                *edited = true;
            }
        });
        if let Some(addr) = &state.draft.depots[0].address {
            ui.weak(format!("Depot address: {addr}"));
        }
    } else {
        depot_name_missing = true;
    }
    if depot_name_missing && ui.small_button("+ Create Depot").clicked() {
        let (lat, lon) = city_center(state.city);
        state.set_main_depot_location(lat, lon, None);
        *edited = true;
    }
    if let Some(idx) = render_geocode_match_list(ui, &state.depot_matches, "depot_matches") {
        let chosen = state.depot_matches[idx].clone();
        state.apply_geocode_match(GeocodeTarget::Depot, &chosen);
        *map_fit_pending = true;
        *edited = true;
    }
}

fn render_stop_quick_add(
    ui: &mut egui::Ui,
    state: &mut ProblemBuilderState,
    action: &mut BuilderAction,
    map_fit_pending: &mut bool,
    edited: &mut bool,
) {
    ui.strong("Add Place / Store to Visit");
    let mut submit_lookup = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Place name:");
        *edited |= ui
            .add(
                egui::TextEdit::singleline(&mut state.stop_name_input)
                    .desired_width(140.0)
                    .hint_text("Client / store / site"),
            )
            .changed();
        ui.label("Address / Coords:");
        let addr_resp = ui.add(
            egui::TextEdit::singleline(&mut state.stop_address_input)
                .desired_width(250.0)
                .hint_text("Street address OR lat, lon"),
        );
        if addr_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            submit_lookup = true;
        }
        if ui.button("Lookup").clicked() || submit_lookup {
            *action = BuilderAction::LookupAddress {
                target: GeocodeTarget::Stop,
                query: state.stop_address_input.clone(),
                add_stop_if_single: false,
            };
        }
        edit_lat_lon(ui, &mut state.stop_lat, &mut state.stop_lon);
    });

    ui.horizontal_wrapped(|ui| {
        ui.label("Type:");
        egui::ComboBox::from_id_salt("quick_stop_chain")
            .selected_text(state.stop_chain.to_string())
            .show_ui(ui, |ui| {
                for chain in ALL_CHAINS {
                    ui.selectable_value(&mut state.stop_chain, chain, chain.to_string());
                }
            });
        ui.label("Demand (A/C/F):");
        ui.add(egui::DragValue::new(&mut state.stop_demand_ambient).range(0..=500));
        ui.add(egui::DragValue::new(&mut state.stop_demand_chilled).range(0..=500));
        ui.add(egui::DragValue::new(&mut state.stop_demand_frozen).range(0..=500));
        ui.label("Svc(m):");
        ui.add(egui::DragValue::new(&mut state.stop_service_minutes).range(0..=600));
        ui.label("Window(m):");
        ui.add(egui::DragValue::new(&mut state.stop_window_start_minutes).range(0..=10_000));
        ui.label("–");
        let min_end = state.stop_window_start_minutes + 1;
        if state.stop_window_end_minutes < min_end {
            state.stop_window_end_minutes = min_end;
        }
        ui.add(egui::DragValue::new(&mut state.stop_window_end_minutes).range(min_end..=20_000));
        ui.weak(format!(
            "({}–{})",
            format_hhmm(state.stop_window_start_minutes),
            format_hhmm(state.stop_window_end_minutes)
        ));
        for (preset_label, p_start, p_end) in [
            ("All Day", 0_i64, 1440_i64),
            ("06–18", 360, 1080),
            ("06–12", 360, 720),
            ("12–18", 720, 1080),
        ] {
            if ui.small_button(preset_label).clicked() {
                state.stop_window_start_minutes = p_start;
                state.stop_window_end_minutes = p_end;
            }
        }

        if ui.button("+ Add Place").clicked() {
            match state.try_add_stop_from_inputs() {
                Ok(true) => {
                    *map_fit_pending = true;
                    *edited = true;
                }
                Ok(false) => {
                    *action = BuilderAction::LookupAddress {
                        target: GeocodeTarget::Stop,
                        query: state.stop_address_input.clone(),
                        add_stop_if_single: true,
                    };
                }
                Err(err) => {
                    state.editor_error = Some(err.to_string());
                }
            }
        }
        if ui.small_button("Pick on Map").clicked() {
            state.show_map_picker = true;
            state.map_pick_mode = MapPickMode::AddStop;
            *map_fit_pending = true;
        }
        if ui
            .selectable_label(state.show_bulk_paste, "Bulk Paste…")
            .on_hover_text(
                "Paste multiple stops as 'Name, lat, lon' or 'Name, lat, lon, A, C, F' lines.",
            )
            .clicked()
        {
            state.show_bulk_paste = !state.show_bulk_paste;
        }
        if ui
            .small_button("Clear Places")
            .on_hover_text("Reset places back to the starter stop.")
            .clicked()
        {
            state.clear_stores_to_starter();
            *map_fit_pending = true;
            *edited = true;
        }
    });

    if state.show_bulk_paste {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                "Paste one place per line: Name, lat, lon   or   Name, lat, lon, A, C, F   or   lat, lon",
            );
            ui.add(
                egui::TextEdit::multiline(&mut state.bulk_stops_input)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text("Warehouse North, -33.8120, 151.1040\nCBD Depot Drop, -33.8688, 151.2093, 2, 1, 0"),
            );
            ui.horizontal(|ui| {
                if ui.button("+ Add Lines").clicked() {
                    match state.add_bulk_coord_lines() {
                        Ok(added) => {
                            state.lookup_status =
                                Some(format!("Added {added} place(s) from pasted list."));
                            *map_fit_pending = true;
                            *edited = true;
                        }
                        Err(err) => {
                            state.editor_error = Some(err.to_string());
                        }
                    }
                }
                if ui.small_button("Hide").clicked() {
                    state.show_bulk_paste = false;
                }
            });
        });
    }

    if let Some(idx) = render_geocode_match_list(ui, &state.stop_matches, "stop_matches") {
        let chosen = state.stop_matches[idx].clone();
        state.apply_geocode_match(GeocodeTarget::Stop, &chosen);
        *map_fit_pending = true;
        *edited = true;
    }
}

fn render_geocode_match_list(
    ui: &mut egui::Ui,
    matches: &[GeocodeMatch],
    id_salt: &str,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.push_id(id_salt, |ui| {
            ui.weak("Matches:");
            for (idx, item) in matches.iter().enumerate() {
                let text = format!(
                    "{} ({:.4}, {:.4}) [{}]",
                    truncate_for_dropdown(&item.label, 42),
                    item.lat,
                    item.lon,
                    item.source
                );
                if ui.small_button(text).on_hover_text(&item.label).clicked() {
                    picked = Some(idx);
                }
            }
        });
    });
    picked
}

fn edit_lat_lon(ui: &mut egui::Ui, lat: &mut f64, lon: &mut f64) -> bool {
    let mut changed = false;
    ui.label("Lat:");
    changed |= ui
        .add(
            egui::DragValue::new(lat)
                .speed(0.001)
                .fixed_decimals(5)
                .range(-90.0..=90.0),
        )
        .changed();
    ui.label("Lon:");
    changed |= ui
        .add(
            egui::DragValue::new(lon)
                .speed(0.001)
                .fixed_decimals(5)
                .range(-180.0..=180.0),
        )
        .changed();
    changed
}

struct BuilderMapPlugin<'a> {
    problem: &'a domain::Problem,
    pick_mode: MapPickMode,
    picked_depot: &'a mut Option<(f64, f64)>,
    picked_stop: &'a mut Option<(f64, f64)>,
}

impl Plugin for BuilderMapPlugin<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        response: &egui::Response,
        projector: &Projector,
        _memory: &MapMemory,
    ) {
        let clip = projector.clip_rect();
        let painter = ui.painter().with_clip_rect(clip);
        let font_id = egui::FontId::proportional(11.0);

        if self.pick_mode != MapPickMode::PanZoom && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }

        if response.clicked()
            && let Some(ptr) = response.interact_pointer_pos().or(response.hover_pos())
        {
            let pos = projector.unproject(ptr.to_vec2());
            let (lon, lat) = (pos.x(), pos.y());
            match self.pick_mode {
                MapPickMode::SetDepot => *self.picked_depot = Some((lat, lon)),
                MapPickMode::AddStop => *self.picked_stop = Some((lat, lon)),
                MapPickMode::PanZoom => {}
            }
        }

        if let Some(first_depot) = self.problem.depots.first() {
            let depot_pos = project(
                projector,
                first_depot.location.lon,
                first_depot.location.lat,
            );
            let guide_stroke =
                egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(70, 130, 210, 90));
            for store in &self.problem.stores {
                let stop_pos = project(projector, store.location.lon, store.location.lat);
                painter.line_segment([depot_pos, stop_pos], guide_stroke);
            }
        }

        for (idx, store) in self.problem.stores.iter().enumerate() {
            let pos = project(projector, store.location.lon, store.location.lat);
            if !clip.expand(24.0).contains(pos) {
                continue;
            }
            painter.circle(
                pos,
                5.5,
                egui::Color32::from_rgb(30, 155, 95),
                egui::Stroke::new(1.5, egui::Color32::WHITE),
            );
            let label = format!("#{} {}", idx + 1, store.name);
            draw_picker_badge(
                &painter,
                pos,
                &label,
                &font_id,
                egui::Color32::from_rgb(20, 95, 58),
            );
        }

        for (idx, depot) in self.problem.depots.iter().enumerate() {
            let pos = project(projector, depot.location.lon, depot.location.lat);
            if !clip.expand(24.0).contains(pos) {
                continue;
            }
            painter.circle(
                pos,
                7.5,
                crate::map_nodes::DEPOT_COLOR,
                egui::Stroke::new(2.0, egui::Color32::WHITE),
            );
            let label = format!("D{idx} {}", depot.name);
            draw_picker_badge(
                &painter,
                pos,
                &label,
                &font_id,
                crate::map_nodes::DEPOT_COLOR,
            );
        }
    }
}

fn draw_picker_badge(
    painter: &egui::Painter,
    pos: egui::Pos2,
    text: &str,
    font_id: &egui::FontId,
    border: egui::Color32,
) {
    let galley = painter.layout_no_wrap(text.to_string(), font_id.clone(), egui::Color32::WHITE);
    let pad = egui::vec2(5.0, 2.0);
    let rect = egui::Rect::from_min_size(
        egui::pos2(pos.x + 9.0, pos.y - galley.size().y * 0.5 - pad.y),
        galley.size() + pad * 2.0,
    );
    painter.rect(
        rect,
        egui::CornerRadius::same(4),
        egui::Color32::from_rgba_unmultiplied(18, 22, 30, 215),
        egui::Stroke::new(1.0, border),
        egui::StrokeKind::Outside,
    );
    painter.galley(rect.min + pad, galley, egui::Color32::WHITE);
}

fn fit_builder_map_to_extents(
    memory: &mut MapMemory,
    problem: &domain::Problem,
    width: f64,
    height: f64,
) {
    let points = problem
        .depots
        .iter()
        .map(|d| (d.location.lon, d.location.lat))
        .chain(
            problem
                .stores
                .iter()
                .map(|s| (s.location.lon, s.location.lat)),
        );
    fit_bounds_to_memory(memory, points, width, height, 14.5);
}

fn render_builder_map_picker(
    ui: &mut egui::Ui,
    state: &mut ProblemBuilderState,
    map_tiles: &mut Option<walkers::HttpTiles>,
    map_memory: &mut MapMemory,
    map_fit_pending: &mut bool,
    edited: &mut bool,
) {
    let tiles = map_tiles.get_or_insert_with(|| {
        walkers::HttpTiles::new(walkers::sources::OpenStreetMap, ui.ctx().clone())
    });
    let width = ui.available_width().max(260.0);
    let height = 270.0_f32;
    if *map_fit_pending {
        fit_builder_map_to_extents(
            map_memory,
            &state.draft,
            f64::from(width),
            f64::from(height),
        );
        *map_fit_pending = false;
    }

    let (default_lat, default_lon) = state
        .draft
        .depots
        .first()
        .map(|d| (d.location.lat, d.location.lon))
        .unwrap_or_else(|| city_center(state.city));

    let mut picked_depot: Option<(f64, f64)> = None;
    let mut picked_stop: Option<(f64, f64)> = None;
    ui.allocate_ui(egui::vec2(width, height), |ui| {
        ui.add(
            Map::new(Some(tiles), map_memory, lon_lat(default_lon, default_lat))
                .zoom_with_ctrl(false)
                .double_click_to_zoom(true)
                .double_click_to_zoom_out(true)
                .with_plugin(BuilderMapPlugin {
                    problem: &state.draft,
                    pick_mode: state.map_pick_mode,
                    picked_depot: &mut picked_depot,
                    picked_stop: &mut picked_stop,
                }),
        );
    });

    if let Some((lat, lon)) = picked_depot {
        state.depot_matches.clear();
        state.set_main_depot_location(lat, lon, None);
        state.lookup_status = Some(format!("Main Depot pinned on map at {lat:.5}, {lon:.5}"));
        *edited = true;
    }
    if let Some((lat, lon)) = picked_stop {
        match state.add_custom_stop_at(lat, lon, None) {
            Ok(()) => {
                state.lookup_status = Some(format!("Added place on map at {lat:.5}, {lon:.5}"));
                *edited = true;
            }
            Err(err) => state.editor_error = Some(err.to_string()),
        }
    }
}

fn render_draft_tables(
    ui: &mut egui::Ui,
    state: &mut ProblemBuilderState,
    action: &mut BuilderAction,
    edited: &mut bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(
            &mut state.section,
            EditorSection::Stores,
            format!("Places / Stores ({})", state.draft.stores.len()),
        );
        ui.selectable_value(
            &mut state.section,
            EditorSection::Depots,
            format!("Depots ({})", state.draft.depots.len()),
        );
        ui.selectable_value(
            &mut state.section,
            EditorSection::Vehicles,
            format!("Vehicles ({})", state.draft.vehicles.len()),
        );
        ui.selectable_value(&mut state.section, EditorSection::Costs, "Rate Card & Meta");
        ui.separator();
        if ui.small_button("Import Configured Route").clicked() {
            *action = BuilderAction::ImportDraft;
        }
    });
    ui.separator();

    match state.section {
        EditorSection::Depots => {
            if ui.small_button("+ Add Depot").clicked() {
                state.add_depot();
                *edited = true;
            }
            let mut remove_idx: Option<usize> = None;
            let can_remove = state.draft.depots.len() > 1;
            egui::ScrollArea::both()
                .id_salt("edit_depots_scroll")
                .max_height(180.0)
                .show(ui, |ui| {
                    egui::Grid::new("edit_depots_grid")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("ID");
                            ui.strong("Name");
                            ui.strong("Address");
                            ui.strong("Lat / Lon");
                            ui.strong("Timezone");
                            ui.label("");
                            ui.end_row();
                            for (idx, depot) in state.draft.depots.iter_mut().enumerate() {
                                ui.label(format!("D{idx}"));
                                *edited |= ui
                                    .add(
                                        egui::TextEdit::singleline(&mut depot.name)
                                            .desired_width(130.0),
                                    )
                                    .changed();
                                *edited |= edit_optional_text(
                                    ui,
                                    &mut depot.address,
                                    190.0,
                                    "optional address",
                                );
                                ui.horizontal(|ui| {
                                    *edited |= edit_lat_lon(
                                        ui,
                                        &mut depot.location.lat,
                                        &mut depot.location.lon,
                                    );
                                });
                                *edited |= edit_optional_text(
                                    ui,
                                    &mut depot.timezone,
                                    110.0,
                                    "optional IANA",
                                );
                                if ui
                                    .add_enabled(can_remove, egui::Button::new("Remove"))
                                    .clicked()
                                {
                                    remove_idx = Some(idx);
                                }
                                ui.end_row();
                            }
                        });
                });
            if let Some(idx) = remove_idx {
                state.remove_depot(idx);
                *edited = true;
            }
        }
        EditorSection::Stores => {
            if ui.small_button("+ Add Place").clicked() {
                state.add_store();
                *edited = true;
            }
            let mut remove_idx: Option<usize> = None;
            let can_remove = state.draft.stores.len() > 1;
            egui::ScrollArea::both()
                .id_salt("edit_stores_scroll")
                .max_height(210.0)
                .show(ui, |ui| {
                    egui::Grid::new("edit_stores_grid")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("ID");
                            ui.strong("Name");
                            ui.strong("Address");
                            ui.strong("Type");
                            ui.strong("Lat / Lon");
                            ui.strong("Amb/Chl/Frz");
                            ui.strong("Svc(m)");
                            ui.strong("Window(m)");
                            ui.label("");
                            ui.end_row();
                            for (idx, store) in state.draft.stores.iter_mut().enumerate() {
                                ui.label(format!("S{idx}"));
                                *edited |= ui
                                    .add(
                                        egui::TextEdit::singleline(&mut store.name)
                                            .desired_width(125.0),
                                    )
                                    .changed();
                                *edited |= edit_optional_text(
                                    ui,
                                    &mut store.address,
                                    180.0,
                                    "optional address",
                                );
                                egui::ComboBox::from_id_salt(("store_chain", idx))
                                    .selected_text(store.chain.to_string())
                                    .show_ui(ui, |ui| {
                                        for chain in ALL_CHAINS {
                                            if ui
                                                .selectable_value(
                                                    &mut store.chain,
                                                    chain,
                                                    chain.to_string(),
                                                )
                                                .changed()
                                            {
                                                *edited = true;
                                            }
                                        }
                                    });
                                ui.horizontal(|ui| {
                                    *edited |= edit_lat_lon(
                                        ui,
                                        &mut store.location.lat,
                                        &mut store.location.lon,
                                    );
                                });
                                ui.horizontal(|ui| {
                                    *edited |= ui
                                        .add(
                                            egui::DragValue::new(&mut store.demand.ambient)
                                                .range(0..=500),
                                        )
                                        .changed();
                                    *edited |= ui
                                        .add(
                                            egui::DragValue::new(&mut store.demand.chilled)
                                                .range(0..=500),
                                        )
                                        .changed();
                                    *edited |= ui
                                        .add(
                                            egui::DragValue::new(&mut store.demand.frozen)
                                                .range(0..=500),
                                        )
                                        .changed();
                                });
                                let mut svc = store.service.minutes();
                                if ui
                                    .add(egui::DragValue::new(&mut svc).range(0..=600))
                                    .changed()
                                {
                                    store.service = domain::Minutes::from_minutes(svc);
                                    *edited = true;
                                }
                                if let Some(win) = store.receiving.first_mut() {
                                    ui.horizontal(|ui| {
                                        let mut start = win.start.minutes();
                                        let mut end = win.end.minutes();
                                        let c1 = ui
                                            .add(egui::DragValue::new(&mut start).range(0..=10_000))
                                            .changed();
                                        ui.label("–");
                                        let c2 = ui
                                            .add(
                                                egui::DragValue::new(&mut end)
                                                    .range((start + 1)..=20_000),
                                            )
                                            .changed();
                                        if c1 || c2 {
                                            win.start = domain::Minutes::from_minutes(start);
                                            win.end =
                                                domain::Minutes::from_minutes(end.max(start + 1));
                                            *edited = true;
                                        }
                                    });
                                } else if ui.small_button("+ Window").clicked() {
                                    store.receiving.push(domain::TimeWindow::new(
                                        domain::Minutes::from_minutes(360),
                                        domain::Minutes::from_minutes(1080),
                                    ));
                                    *edited = true;
                                }
                                if ui
                                    .add_enabled(can_remove, egui::Button::new("Remove"))
                                    .clicked()
                                {
                                    remove_idx = Some(idx);
                                }
                                ui.end_row();
                            }
                        });
                });
            if let Some(idx) = remove_idx {
                state.remove_store(idx);
                *edited = true;
            }
        }
        EditorSection::Vehicles => {
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("+ Add Vehicle").clicked() {
                    state.add_vehicle();
                    *edited = true;
                }
                if ui.small_button("Optimize Truck Capacity…").clicked() {
                    *action = BuilderAction::OpenTruckOptimizer(None);
                }
                if ui
                    .small_button("Auto-Fit Shifts to Windows")
                    .on_hover_text(
                        "Expand all driver shifts and vehicle max_duration to cover every stop receiving window.",
                    )
                    .clicked()
                {
                    state.auto_fit_shifts_to_windows();
                    *edited = true;
                }
            });
            for idx in 0..state.draft.vehicles.len() {
                state.ensure_driver_for_vehicle(idx);
            }
            let mut remove_idx: Option<usize> = None;
            let can_remove = state.draft.vehicles.len() > 1;
            let depot_count = state.draft.depots.len();
            egui::ScrollArea::both()
                .id_salt("edit_vehicles_scroll")
                .max_height(180.0)
                .show(ui, |ui| {
                    egui::Grid::new("edit_vehicles_grid")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("ID");
                            ui.strong("Name");
                            ui.strong("Depot");
                            ui.strong("Cap A/C/F");
                            ui.strong("MaxDur(m)");
                            ui.strong("km/h");
                            ui.strong("Driver Shift (m)");
                            ui.label("");
                            ui.end_row();
                            for (idx, veh) in state.draft.vehicles.iter_mut().enumerate() {
                                ui.label(format!("V{idx}"));
                                *edited |= ui
                                    .add(
                                        egui::TextEdit::singleline(&mut veh.name)
                                            .desired_width(120.0),
                                    )
                                    .changed();
                                let mut dep = (veh.depot.0 as usize).min(depot_count - 1);
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut dep)
                                            .range(0..=(depot_count - 1))
                                            .prefix("D"),
                                    )
                                    .changed()
                                {
                                    veh.depot = domain::DepotId(dep as u32);
                                    *edited = true;
                                }
                                ui.horizontal(|ui| {
                                    *edited |= ui
                                        .add(
                                            egui::DragValue::new(&mut veh.capacity.ambient)
                                                .range(0..=1000),
                                        )
                                        .changed();
                                    *edited |= ui
                                        .add(
                                            egui::DragValue::new(&mut veh.capacity.chilled)
                                                .range(0..=1000),
                                        )
                                        .changed();
                                    *edited |= ui
                                        .add(
                                            egui::DragValue::new(&mut veh.capacity.frozen)
                                                .range(0..=1000),
                                        )
                                        .changed();
                                });
                                let mut max_dur = veh.max_duration.minutes();
                                if ui
                                    .add(egui::DragValue::new(&mut max_dur).range(30..=10_000))
                                    .changed()
                                {
                                    veh.max_duration = domain::Minutes::from_minutes(max_dur);
                                    *edited = true;
                                }
                                *edited |= ui
                                    .add(
                                        egui::DragValue::new(&mut veh.avg_speed_kmh)
                                            .speed(1.0)
                                            .fixed_decimals(1)
                                            .range(5.0..=120.0),
                                    )
                                    .changed();
                                if let Some(shift) = state
                                    .draft
                                    .drivers
                                    .get_mut(idx)
                                    .and_then(|d| d.shifts.first_mut())
                                {
                                    ui.horizontal(|ui| {
                                        let mut s_min = shift.start.minutes();
                                        let mut e_min = shift.end.minutes();
                                        let c1 = ui
                                            .add(egui::DragValue::new(&mut s_min).range(0..=10_000))
                                            .changed();
                                        ui.label("–");
                                        let c2 = ui
                                            .add(
                                                egui::DragValue::new(&mut e_min)
                                                    .range((s_min + 15)..=20_000),
                                            )
                                            .changed();
                                        ui.weak(format!(
                                            "({}–{})",
                                            format_hhmm(s_min),
                                            format_hhmm(e_min.max(s_min + 15))
                                        ));
                                        if c1 || c2 {
                                            shift.start = domain::Minutes::from_minutes(s_min);
                                            shift.end = domain::Minutes::from_minutes(
                                                e_min.max(s_min + 15),
                                            );
                                            *edited = true;
                                        }
                                    });
                                } else {
                                    ui.weak("—");
                                }
                                ui.horizontal(|ui| {
                                    if ui.small_button("Fill / Size").clicked() {
                                        *action = BuilderAction::OpenTruckOptimizer(Some(idx));
                                    }
                                    if ui
                                        .add_enabled(can_remove, egui::Button::new("Remove"))
                                        .clicked()
                                    {
                                        remove_idx = Some(idx);
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
            if let Some(idx) = remove_idx {
                state.remove_vehicle(idx);
                *edited = true;
            }
        }
        EditorSection::Costs => {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Carrier Rate Card Presets:");
                for (label, fixed, per_km, per_hr) in [
                    (
                        "Metro Van ($65 + $0.65/km + $42/h)",
                        6_500_i64,
                        65_i64,
                        4_200_i64,
                    ),
                    (
                        "12-Pallet Rigid ($150 + $0.90/km + $45/h)",
                        15_000,
                        90,
                        4_500,
                    ),
                    (
                        "22-Pallet Semi ($280 + $1.45/km + $68/h)",
                        28_000,
                        145,
                        6_800,
                    ),
                    (
                        "Linehaul B-Double ($450 + $2.10/km + $85/h)",
                        45_000,
                        210,
                        8_500,
                    ),
                ] {
                    if ui.small_button(label).clicked() {
                        state.draft.cost.fixed_per_route_cents = fixed;
                        state.draft.cost.per_km_cents = per_km;
                        state.draft.cost.per_hour_cents = per_hr;
                        *edited = true;
                    }
                }
            });
            ui.add_space(4.0);
            egui::Grid::new("edit_costs_grid").show(ui, |ui| {
                ui.label("Problem name:");
                *edited |= ui
                    .add(egui::TextEdit::singleline(&mut state.draft.name).desired_width(240.0))
                    .changed();
                ui.end_row();

                ui.label("Fixed / route (cents):");
                ui.horizontal(|ui| {
                    *edited |= ui
                        .add(
                            egui::DragValue::new(&mut state.draft.cost.fixed_per_route_cents)
                                .speed(50)
                                .range(0..=1_000_000),
                        )
                        .changed();
                    ui.weak(format!(
                        "(${:.2})",
                        state.draft.cost.fixed_per_route_cents as f64 / 100.0
                    ));
                });
                ui.end_row();

                ui.label("Per km (cents):");
                ui.horizontal(|ui| {
                    *edited |= ui
                        .add(
                            egui::DragValue::new(&mut state.draft.cost.per_km_cents)
                                .speed(5)
                                .range(0..=100_000),
                        )
                        .changed();
                    ui.weak(format!(
                        "(${:.2}/km)",
                        state.draft.cost.per_km_cents as f64 / 100.0
                    ));
                });
                ui.end_row();

                ui.label("Per hour (cents):");
                ui.horizontal(|ui| {
                    *edited |= ui
                        .add(
                            egui::DragValue::new(&mut state.draft.cost.per_hour_cents)
                                .speed(50)
                                .range(0..=1_000_000),
                        )
                        .changed();
                    ui.weak(format!(
                        "(${:.2}/hr)",
                        state.draft.cost.per_hour_cents as f64 / 100.0
                    ));
                });
                ui.end_row();
            });
            let bench_cents = state.draft.cost.fixed_per_route_cents
                + 100 * state.draft.cost.per_km_cents
                + 4 * state.draft.cost.per_hour_cents;
            ui.add_space(4.0);
            ui.weak(format!(
                "Benchmark 100 km / 4 hr run: ${:.2}",
                bench_cents as f64 / 100.0
            ));
        }
    }
}

fn edit_optional_text(
    ui: &mut egui::Ui,
    value: &mut Option<String>,
    width: f32,
    hint: &str,
) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(width)
                .hint_text(hint),
        )
        .changed();
    if changed {
        let trimmed = text.trim();
        *value = (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    changed
}

fn render_synthetic_generator(
    ui: &mut egui::Ui,
    state: &mut ProblemBuilderState,
    action: &mut BuilderAction,
) {
    egui::CollapsingHeader::new("Synthetic Scenario Generator (Sim presets & benchmarks)")
        .id_salt("synthetic_sim_generator_header")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Type:");
                egui::ComboBox::from_id_salt("builder_chain")
                    .selected_text(state.chain.to_string())
                    .show_ui(ui, |ui| {
                        for chain in ALL_CHAINS {
                            ui.selectable_value(&mut state.chain, chain, chain.to_string());
                        }
                    });

                ui.label("City:");
                egui::ComboBox::from_id_salt("builder_city")
                    .selected_text(city_label(state.city))
                    .show_ui(ui, |ui| {
                        for city in sim::City::ALL {
                            ui.selectable_value(&mut state.city, city, city_label(city));
                        }
                    });

                ui.label("Places:");
                ui.add(egui::DragValue::new(&mut state.stores).range(1..=4_900));
                ui.label("Depots:");
                ui.add(egui::DragValue::new(&mut state.depots).range(1..=50));
                ui.label("Vehicles:");
                ui.add(egui::DragValue::new(&mut state.vehicles).range(1..=500));
                ui.label("Seed:");
                ui.add(egui::DragValue::new(&mut state.seed).range(0..=u64::MAX));
            });

            ui.horizontal_wrapped(|ui| {
                ui.label("Name (opt):");
                ui.add(
                    egui::TextEdit::singleline(&mut state.custom_name)
                        .desired_width(160.0)
                        .hint_text("auto-generated"),
                );
                ui.checkbox(&mut state.christmas_peak, "Christmas peak");
                ui.checkbox(&mut state.use_city_timezones, "City timezones");
                ui.label("Horizon start (opt):");
                ui.add(
                    egui::TextEdit::singleline(&mut state.horizon_start)
                        .desired_width(175.0)
                        .hint_text("2026-11-05T00:00:00Z"),
                );
            });

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Generate & Import").clicked() {
                    *action = BuilderAction::GenerateAndImport;
                }
                if ui.button("Populate Editor & JSON").clicked() {
                    *action = BuilderAction::PopulateScenario;
                }
            });
        });
}

/// The per-route colour chips plus the Fit-extents button, above the map.
fn route_chips(
    ui: &mut egui::Ui,
    bundle: &SolutionBundle,
    map_fit_pending: &mut bool,
    hidden_routes: &mut HashSet<usize>,
) {
    ui.horizontal_wrapped(|ui| {
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

/// Navigation/action emitted by the Selected Node inspector panel.
enum MapInspectorAction {
    None,
    Clear,
    Center(f64, f64),
    SelectNode(domain::Node, f64, f64),
    OpenTruckRoute(usize),
}

impl ConsoleApp {
    /// Render the loaded map body: route chips, node controls, geometry status,
    /// optional selected-node inspector panel, the map itself, and attribution.
    /// Returns `Some(route_idx)` when the operator clicks `Truck Load` for a route.
    fn render_map_body(&mut self, ui: &mut egui::Ui, bundle: &SolutionBundle) -> Option<usize> {
        let mut jump_to_truck_route: Option<usize> = None;
        let map_nodes = collect_map_nodes(
            &bundle.problem,
            &bundle.solution,
            &self.hidden_routes,
            route_color,
        );

        route_chips(
            ui,
            bundle,
            &mut self.map_fit_pending,
            &mut self.hidden_routes,
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Zoom −").clicked() {
                let _ = self.map_memory.zoom_out();
            }
            if ui.button("Zoom +").clicked() {
                let _ = self.map_memory.zoom_in();
            }
            ui.weak(format!("zoom {:.1}", self.map_memory.zoom()));
            ui.separator();
            ui.checkbox(&mut self.show_map_labels, "Show node info");
            ui.label("Inspect:");
            let selected_text = self
                .selected_map_node
                .and_then(|sel| map_nodes.iter().find(|n| n.node == sel))
                .map(|info| truncate_for_dropdown(&info.title, 30))
                .unwrap_or_else(|| format!("pick node ({})…", map_nodes.len()));
            egui::ComboBox::from_id_salt("map_node_picker")
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for info in &map_nodes {
                        let chosen = self.selected_map_node == Some(info.node);
                        if ui.selectable_label(chosen, &info.short_badge).clicked() {
                            self.selected_map_node = Some(info.node);
                        }
                    }
                });
            ui.separator();
            if self.map_geometry_loading {
                ui.spinner();
                ui.label("Loading OSM road geometry…");
            } else if let Some(err) = &self.map_geometry_error {
                ui.colored_label(egui::Color32::RED, format!("Road geometry error: {err}"));
            } else if self.map_geometry.routes.iter().any(|r| !r.is_empty()) {
                let routed = self
                    .map_geometry
                    .routes
                    .iter()
                    .filter(|r| !r.is_empty())
                    .count();
                let fallback_legs: usize = self
                    .map_geometry
                    .route_legs
                    .iter()
                    .flat_map(|legs| legs.iter())
                    .filter(|leg| leg.fallback)
                    .count();
                if fallback_legs == 0 {
                    ui.colored_label(
                        egui::Color32::GREEN,
                        format!("OSM road alignment ({routed} routed)"),
                    );
                } else {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!(
                            "OSM road alignment ({routed} routes, {fallback_legs} unrouted leg(s) dashed)"
                        ),
                    );
                }
            } else {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Straight-line preview (solved without OSM PBF — re-solve with OSM PBF for road alignment)",
                );
            }
        });

        // Tiles are created lazily on first paint; downloading starts when
        // the tab is actually shown.
        let tiles = self.map_tiles.get_or_insert_with(|| {
            walkers::HttpTiles::new(walkers::sources::OpenStreetMap, ui.ctx().clone())
        });

        let selected_info = self
            .selected_map_node
            .and_then(|sel| map_nodes.iter().find(|n| n.node == sel))
            .cloned();

        let footer_height = 24.0_f32;
        let avail_width = ui.available_width().max(200.0);
        let show_side_inspector = selected_info.is_some() && avail_width >= 760.0;
        let bottom_inspector_height = if !show_side_inspector && selected_info.is_some() {
            175.0_f32
        } else {
            0.0
        };
        let map_height =
            (ui.available_height() - footer_height - bottom_inspector_height).max(200.0);
        let inspector_width = if show_side_inspector { 290.0_f32 } else { 0.0 };
        let map_width = (avail_width - inspector_width).max(200.0);

        if self.map_fit_pending {
            fit_map_to_extents(
                &mut self.map_memory,
                bundle,
                &self.map_geometry,
                f64::from(map_width),
                f64::from(map_height),
            );
            self.map_fit_pending = false;
        }

        ui.separator();
        let mut inspector_action = MapInspectorAction::None;
        ui.horizontal(|ui| {
            ui.allocate_ui(egui::vec2(map_width, map_height), |ui| {
                ui.add(
                    Map::new(Some(tiles), &mut self.map_memory, lon_lat(151.21, -33.87))
                        // Wheel zooms without needing Ctrl; double-click zooms in,
                        // right-double-click zooms out. Drag still pans.
                        .zoom_with_ctrl(false)
                        .double_click_to_zoom(true)
                        .double_click_to_zoom_out(true)
                        .with_plugin(RouteOverlay {
                            bundle,
                            hidden: &self.hidden_routes,
                            geometry: &self.map_geometry,
                            nodes: &map_nodes,
                            show_labels: self.show_map_labels,
                            selected_node: &mut self.selected_map_node,
                        }),
                );
            });
            if show_side_inspector && let Some(info) = &selected_info {
                ui.allocate_ui(egui::vec2(inspector_width - 8.0, map_height), |ui| {
                    inspector_action =
                        render_selected_node_frame(ui, info, bundle, "map_selected_node_scroll");
                });
            }
        });
        if !show_side_inspector && let Some(info) = &selected_info {
            ui.allocate_ui(
                egui::vec2(avail_width, bottom_inspector_height - 6.0),
                |ui| {
                    inspector_action =
                        render_selected_node_frame(ui, info, bundle, "map_selected_node_drawer");
                },
            );
        }
        match inspector_action {
            MapInspectorAction::None => {}
            MapInspectorAction::Clear => self.selected_map_node = None,
            MapInspectorAction::Center(lon, lat) => {
                self.map_memory.center_at(lon_lat(lon, lat));
            }
            MapInspectorAction::SelectNode(node, lon, lat) => {
                self.selected_map_node = Some(node);
                self.map_memory.center_at(lon_lat(lon, lat));
            }
            MapInspectorAction::OpenTruckRoute(route_idx) => {
                jump_to_truck_route = Some(route_idx);
            }
        }

        ui.horizontal(|ui| {
            ui.weak("© OpenStreetMap contributors — tile.openstreetmap.org");
            ui.weak(format!(
                "· showing {} of {} routes · {} nodes",
                bundle.solution.routes.len() - self.hidden_routes.len(),
                bundle.solution.routes.len(),
                map_nodes.len(),
            ));
            ui.weak(format!(
                "· zoom {:.1} · hover/click any node for details",
                self.map_memory.zoom()
            ));
        });
        jump_to_truck_route
    }
}

impl ConsoleApp {
    // ── Events tab ──────────────────────────────────────────────────────

    fn render_events_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Control Tower Event Stream");
        ui.label("Live SSE dispatch & optimizer execution feed from the TMS backend.");

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

pub(crate) fn route_color(index: usize) -> egui::Color32 {
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
/// Normalized Web Mercator Y coordinate in `[0.0, 1.0]` (matching walkers).
fn mercator_y(lat_deg: f64) -> f64 {
    let clamped = lat_deg.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    0.5 - (std::f64::consts::FRAC_PI_4 + clamped * 0.5).tan().ln() / (2.0 * std::f64::consts::PI)
}

/// Compute the Web Mercator centre `(lon, lat)` and zoom level fitting `(min_lon..max_lon, min_lat..max_lat)`
/// into a `(width, height)` viewport in pixels.
fn compute_fit_camera(
    min_lon: f64,
    max_lon: f64,
    min_lat: f64,
    max_lat: f64,
    width: f64,
    height: f64,
) -> (f64, f64, f64) {
    let center_lon = (min_lon + max_lon) / 2.0;
    let center_lat = (min_lat + max_lat) / 2.0;
    let x_span = ((max_lon - min_lon).abs() / 360.0).max(1e-5);
    let y_span = (mercator_y(min_lat) - mercator_y(max_lat)).abs().max(1e-5);
    let zoom_x = (width / (256.0 * x_span)).log2();
    let zoom_y = (height / (256.0 * y_span)).log2();
    let zoom = (zoom_x.min(zoom_y) - 0.35).clamp(2.0, 16.0);
    (center_lon, center_lat, zoom)
}

fn fit_bounds_to_memory(
    memory: &mut MapMemory,
    points: impl IntoIterator<Item = (f64, f64)>,
    width: f64,
    height: f64,
    max_zoom: f64,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let mut min_lon = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut min_lat = f64::INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    for (lon, lat) in points {
        if lon.is_finite() && lat.is_finite() {
            min_lon = min_lon.min(lon);
            max_lon = max_lon.max(lon);
            min_lat = min_lat.min(lat);
            max_lat = max_lat.max(lat);
        }
    }
    if !min_lon.is_finite() {
        return;
    }
    let (center_lon, center_lat, zoom) =
        compute_fit_camera(min_lon, max_lon, min_lat, max_lat, width, height);
    memory.center_at(lon_lat(center_lon, center_lat));
    let _ = memory.set_zoom(zoom.min(max_zoom));
}

/// Center and zoom the map to encompass every depot, stop, unassigned store,
/// and routed polyline point. Called at render time when the available size is
/// known.
fn fit_map_to_extents(
    memory: &mut MapMemory,
    bundle: &SolutionBundle,
    geometry: &SolutionGeometryData,
    width: f64,
    height: f64,
) {
    let route_points = bundle.solution.routes.iter().flat_map(|route| {
        route
            .steps
            .iter()
            .filter_map(|stop| node_lon_lat(&bundle.problem, stop.node))
    });
    let geom_points = geometry
        .routes
        .iter()
        .flat_map(|poly| poly.iter().map(|&[lat, lon]| (lon, lat)));
    let unassigned_points = bundle.solution.unassigned.iter().filter_map(|store_id| {
        bundle
            .problem
            .stores
            .get(store_id.0 as usize)
            .map(|s| (s.location.lon, s.location.lat))
    });
    fit_bounds_to_memory(
        memory,
        route_points.chain(geom_points).chain(unassigned_points),
        width,
        height,
        16.0,
    );
}

/// Draws depot markers, stop dots and per-route polylines on the map.
///
/// When `geometry` has per-leg polylines for a route, routed legs are drawn
/// along real OSM roads while disconnected fallback legs are dashed. Otherwise
/// it falls back to straight lines between stops.
struct RouteOverlay<'a> {
    bundle: &'a SolutionBundle,
    hidden: &'a HashSet<usize>,
    geometry: &'a SolutionGeometryData,
    nodes: &'a [MapNodeInfo],
    show_labels: bool,
    selected_node: &'a mut Option<domain::Node>,
}

impl Plugin for RouteOverlay<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        response: &egui::Response,
        projector: &Projector,
        memory: &MapMemory,
    ) {
        let clip = projector.clip_rect();
        let painter = ui.painter().with_clip_rect(clip);

        for (index, route) in self.bundle.solution.routes.iter().enumerate() {
            if self.hidden.contains(&index) {
                continue;
            }
            let color = route_color(index);
            let stroke = egui::Stroke::new(2.5_f32, color);

            if let Some(legs) = self
                .geometry
                .route_legs
                .get(index)
                .filter(|l| !l.is_empty())
            {
                for leg in legs {
                    let pts: Vec<egui::Pos2> = leg
                        .points
                        .iter()
                        .map(|&[lat, lon]| project(projector, lon, lat))
                        .collect();
                    if leg.fallback {
                        draw_dashed_polyline(&painter, &pts, color);
                    } else {
                        draw_polyline(&painter, &pts, stroke);
                        draw_polyline_arrows(&painter, &pts, color);
                    }
                }
            } else {
                let line: Vec<egui::Pos2> =
                    match self.geometry.routes.get(index).filter(|p| !p.is_empty()) {
                        Some(polyline) => polyline
                            .iter()
                            .map(|&[lat, lon]| project(projector, lon, lat))
                            .collect(),
                        None => route
                            .steps
                            .iter()
                            .filter_map(|stop| {
                                node_lon_lat(&self.bundle.problem, stop.node)
                                    .map(|(lon, lat)| project(projector, lon, lat))
                            })
                            .collect(),
                    };
                draw_polyline(&painter, &line, stroke);
                draw_polyline_arrows(&painter, &line, color);
            }
        }

        let projected_nodes: Vec<(usize, egui::Pos2)> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, info)| {
                let pos = project(projector, info.lon, info.lat);
                clip.expand(16.0).contains(pos).then_some((idx, pos))
            })
            .collect();

        let hover_hits = if response.hovered() {
            response
                .hover_pos()
                .map(|ptr| nodes_near_pointer(&projected_nodes, ptr, 13.0))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if !hover_hits.is_empty() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        if response.clicked()
            && let Some(ptr) = response.interact_pointer_pos().or(response.hover_pos())
        {
            let click_hits = nodes_near_pointer(&projected_nodes, ptr, 14.0);
            if click_hits.is_empty() {
                *self.selected_node = None;
            } else if let Some(current) = *self.selected_node
                && let Some(pos) = click_hits
                    .iter()
                    .position(|&idx| self.nodes[idx].node == current)
            {
                let next = click_hits[(pos + 1) % click_hits.len()];
                *self.selected_node = Some(self.nodes[next].node);
            } else {
                *self.selected_node = Some(self.nodes[click_hits[0]].node);
            }
        }

        // Draw markers in Z-order: unassigned -> served stores -> depots.
        for pass in 0..3 {
            for &(idx, pos) in &projected_nodes {
                let info = &self.nodes[idx];
                let layer = match info.kind {
                    MapNodeKind::UnassignedStore { .. } => 0,
                    MapNodeKind::ServedStore { .. } => 1,
                    MapNodeKind::Depot { .. } => 2,
                };
                if layer != pass {
                    continue;
                }
                let is_selected = *self.selected_node == Some(info.node);
                let is_hovered = hover_hits.contains(&idx);
                draw_node_marker(&painter, pos, info, is_selected, is_hovered);
            }
        }

        // Draw concise on-map information badges with screen-space collision avoidance.
        let show_detail_line = memory.zoom() >= 12.8;
        self.draw_node_labels(
            &painter,
            clip,
            &projected_nodes,
            &hover_hits,
            show_detail_line,
        );

        if response.hovered() && !hover_hits.is_empty() {
            egui::Tooltip::for_widget(response)
                .at_pointer()
                .width(340.0)
                .show(|ui| {
                    for (i, &idx) in hover_hits.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
                        render_node_card(ui, &self.nodes[idx]);
                    }
                });
        }
    }
}

/// Project a lon/lat to a screen position.
fn project(projector: &Projector, lon: f64, lat: f64) -> egui::Pos2 {
    let projected = projector.project(lon_lat(lon, lat));
    egui::pos2(projected.x, projected.y)
}

/// Draw a connected polyline through `points` (no-op for fewer than two).
fn draw_polyline(painter: &egui::Painter, points: &[egui::Pos2], stroke: egui::Stroke) {
    if points.len() >= 2 {
        painter.add(egui::Shape::line(points.to_vec(), stroke));
    }
}

/// Draw directional arrowheads along `points` when segments are visually long enough.
fn draw_polyline_arrows(painter: &egui::Painter, points: &[egui::Pos2], color: egui::Color32) {
    if points.len() < 2 {
        return;
    }
    let total_len: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
    if total_len < 55.0 {
        return;
    }
    let mut since_arrow = 0.0_f32;
    let spacing = 85.0_f32;
    let mut drew_any = false;
    let tip_size = 5.5_f32;
    for seg in points.windows(2) {
        let delta = seg[1] - seg[0];
        let seg_len = delta.length();
        if seg_len < 1.0 {
            continue;
        }
        since_arrow += seg_len;
        let threshold = if drew_any {
            spacing
        } else {
            (total_len * 0.5).min(spacing)
        };
        if since_arrow >= threshold {
            let dir = delta / seg_len;
            let mid = seg[0] + delta * 0.5;
            let tip = mid + dir * tip_size;
            let perp = egui::vec2(-dir.y, dir.x) * (tip_size * 0.6);
            let wing1 = mid - dir * (tip_size * 0.5) + perp;
            let wing2 = mid - dir * (tip_size * 0.5) - perp;
            painter.add(egui::Shape::convex_polygon(
                vec![tip, wing1, wing2],
                color,
                egui::Stroke::new(0.8, egui::Color32::WHITE),
            ));
            since_arrow = 0.0;
            drew_any = true;
        }
    }
}

/// Draw a subtle dashed segment for unrouted fallback legs.
fn draw_dashed_polyline(painter: &egui::Painter, points: &[egui::Pos2], color: egui::Color32) {
    if points.len() >= 2 {
        let stroke = egui::Stroke::new(2.0_f32, color.gamma_multiply(0.65));
        painter.add(egui::Shape::dashed_line(points, stroke, 8.0, 6.0));
    }
}

/// Draw a depot ("D"), served store (numbered), or unassigned store ("!") marker.
fn draw_node_marker(
    painter: &egui::Painter,
    pos: egui::Pos2,
    info: &MapNodeInfo,
    selected: bool,
    hovered: bool,
) {
    let base_radius = match info.kind {
        MapNodeKind::Depot { .. } => 9.0_f32,
        MapNodeKind::ServedStore { .. } => 8.0_f32,
        MapNodeKind::UnassignedStore { .. } => 8.0_f32,
    };
    let radius = if selected || hovered {
        base_radius + 1.5
    } else {
        base_radius
    };

    if selected || hovered {
        let halo_color = if selected {
            egui::Color32::from_rgb(255, 215, 60)
        } else {
            egui::Color32::WHITE
        };
        painter.circle_stroke(pos, radius + 3.5, egui::Stroke::new(2.0, halo_color));
    }

    match info.kind {
        MapNodeKind::Depot { .. } => {
            painter.circle(
                pos,
                radius,
                info.accent,
                egui::Stroke::new(2.0, egui::Color32::WHITE),
            );
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                "D",
                egui::FontId::proportional(10.5),
                egui::Color32::WHITE,
            );
        }
        MapNodeKind::ServedStore { .. } => {
            painter.circle(
                pos,
                radius,
                info.accent,
                egui::Stroke::new(1.2, egui::Color32::WHITE),
            );
            if let Some(seq) = info.visits.first().and_then(|v| v.stop_sequence) {
                let font_size = if seq >= 100 {
                    8.0
                } else if seq >= 10 {
                    9.5
                } else {
                    10.5
                };
                painter.text(
                    pos,
                    egui::Align2::CENTER_CENTER,
                    seq.to_string(),
                    egui::FontId::proportional(font_size),
                    egui::Color32::WHITE,
                );
            }
        }
        MapNodeKind::UnassignedStore { .. } => {
            let pts = vec![
                pos + egui::vec2(0.0, -radius),
                pos + egui::vec2(radius, 0.0),
                pos + egui::vec2(0.0, radius),
                pos + egui::vec2(-radius, 0.0),
            ];
            painter.add(egui::Shape::convex_polygon(
                pts,
                info.accent,
                egui::Stroke::new(1.5, egui::Color32::WHITE),
            ));
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                "!",
                egui::FontId::proportional(10.5),
                egui::Color32::WHITE,
            );
        }
    }
}

impl RouteOverlay<'_> {
    /// Draws on-map node information badges prioritised so selected, hovered, and
    /// depot nodes claim screen space cleanly before individual stores.
    fn draw_node_labels(
        &self,
        painter: &egui::Painter,
        clip: egui::Rect,
        projected_nodes: &[(usize, egui::Pos2)],
        hover_hits: &[usize],
        show_detail_line: bool,
    ) {
        let mut order: Vec<(u8, usize, egui::Pos2)> = projected_nodes
            .iter()
            .filter_map(|&(idx, pos)| {
                let info = &self.nodes[idx];
                let is_selected = *self.selected_node == Some(info.node);
                let is_hovered = hover_hits.contains(&idx);
                if !self.show_labels && !is_selected && !is_hovered {
                    return None;
                }
                let priority = if is_selected {
                    0
                } else if is_hovered {
                    1
                } else if matches!(info.kind, MapNodeKind::Depot { .. }) {
                    2
                } else if matches!(info.kind, MapNodeKind::UnassignedStore { .. }) {
                    3
                } else {
                    4
                };
                Some((priority, idx, pos))
            })
            .collect();
        order.sort_by_key(|(prio, idx, _)| (*prio, *idx));

        let font_id = egui::FontId::proportional(11.0);
        let mut occupied: Vec<egui::Rect> = Vec::with_capacity(order.len());
        let bounds = clip.shrink(2.0);

        for (prio, idx, pos) in order {
            let info = &self.nodes[idx];
            let include_detail = show_detail_line || prio <= 1;
            let text = if include_detail {
                format!("{}\n{}", info.short_badge, info.detail_badge)
            } else {
                info.short_badge.clone()
            };
            let galley = painter.layout_no_wrap(text, font_id.clone(), egui::Color32::WHITE);
            let pill_size = galley.size() + egui::vec2(10.0, 4.0);
            let rect = if let Some(placed) =
                pick_non_overlapping_rect(pos, pill_size, &mut occupied, bounds)
            {
                placed
            } else if prio <= 1 {
                let fallback = egui::Rect::from_min_size(
                    egui::pos2(pos.x + 10.0, pos.y - pill_size.y * 0.5),
                    pill_size,
                );
                occupied.push(fallback);
                fallback
            } else {
                continue;
            };

            painter.rect_filled(
                rect,
                egui::CornerRadius::same(4),
                egui::Color32::from_rgba_unmultiplied(18, 22, 28, 215),
            );
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(4),
                egui::Stroke::new(1.0, info.accent),
                egui::StrokeKind::Inside,
            );
            painter.galley(
                rect.min + egui::vec2(5.0, 2.0),
                galley,
                egui::Color32::WHITE,
            );
        }
    }
}

/// Renders structured node metadata inside either a hover tooltip or the docked
/// Selected Node inspector card, including Prev/Next route step navigation.
fn render_selected_node_frame(
    ui: &mut egui::Ui,
    info: &MapNodeInfo,
    bundle: &SolutionBundle,
    scroll_id: &str,
) -> MapInspectorAction {
    let mut action = MapInspectorAction::None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Selected Node");
            if let Some(visit) = info.visits.first()
                && let Some(route) = bundle.solution.routes.get(visit.route_index)
            {
                if ui
                    .add_enabled(visit.step_index > 0, egui::Button::new("◀ Prev").small())
                    .clicked()
                    && let Some(prev_stop) = route.steps.get(visit.step_index - 1)
                    && let Some((lon, lat)) = node_lon_lat(&bundle.problem, prev_stop.node)
                {
                    action = MapInspectorAction::SelectNode(prev_stop.node, lon, lat);
                }
                if ui
                    .add_enabled(
                        visit.step_index + 1 < route.steps.len(),
                        egui::Button::new("Next ▶").small(),
                    )
                    .clicked()
                    && let Some(next_stop) = route.steps.get(visit.step_index + 1)
                    && let Some((lon, lat)) = node_lon_lat(&bundle.problem, next_stop.node)
                {
                    action = MapInspectorAction::SelectNode(next_stop.node, lon, lat);
                }
                if ui.small_button("Truck Load").clicked() {
                    action = MapInspectorAction::OpenTruckRoute(visit.route_index);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").clicked() {
                    action = MapInspectorAction::Clear;
                }
                if ui.small_button("Center").clicked() {
                    action = MapInspectorAction::Center(info.lon, info.lat);
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt(scroll_id)
            .show(ui, |ui| {
                render_node_card(ui, info);
            });
    });
    action
}

fn render_store_specs(
    ui: &mut egui::Ui,
    chain: domain::Chain,
    demand: &domain::Load,
    service: domain::Minutes,
    windows: &[domain::TimeWindow],
    from_chain_defaults: bool,
) {
    if chain != domain::Chain::General {
        ui.label(format!("Banner: {chain}"));
    }
    ui.label(format!(
        "Demand: {} units ({}A / {}C / {}F)",
        demand.total(),
        demand.ambient,
        demand.chilled,
        demand.frozen
    ));
    ui.label(format!("Service duration: {service}"));
    ui.label(format!(
        "Receiving: {}",
        format_windows(windows, from_chain_defaults)
    ));
}

fn render_node_card(ui: &mut egui::Ui, info: &MapNodeInfo) {
    ui.colored_label(info.accent, egui::RichText::new(&info.title).strong());
    ui.weak(format!("Coordinates: {:.5}, {:.5}", info.lat, info.lon));
    if let Some(addr) = &info.address {
        ui.label(format!("Address: {addr}"));
    }
    if let Some(tz) = &info.timezone {
        ui.weak(format!("Timezone: {tz}"));
    }
    ui.add_space(4.0);

    match &info.kind {
        MapNodeKind::Depot { routes_using } => {
            if routes_using.is_empty() {
                ui.label("Status: Depot idle (no visible routes)");
            } else {
                let list = routes_using
                    .iter()
                    .map(|r| format!("R{}", r + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.label(format!("Active routes: {list}"));
                for visit in info.visits.iter().take(8) {
                    let role = if visit.step_index == 0 {
                        "Depart"
                    } else if visit.step_index + 1 == visit.total_steps {
                        "Return"
                    } else {
                        "Hand-over"
                    };
                    ui.small(format!(
                        "• R{} ({}, {}): {} {}–{}",
                        visit.route_index + 1,
                        visit.vehicle_name,
                        visit.driver_names,
                        role,
                        visit.arrive,
                        visit.depart
                    ));
                }
                if info.visits.len() > 8 {
                    ui.small(format!("…and {} more visit(s)", info.visits.len() - 8));
                }
            }
        }
        MapNodeKind::ServedStore {
            chain,
            demand,
            service,
            windows,
            from_chain_defaults,
        } => {
            if let Some(visit) = info.visits.first() {
                let seq_label = visit
                    .stop_sequence
                    .map(|s| format!("Drop #{s}"))
                    .unwrap_or_else(|| format!("Step #{}", visit.step_index + 1));
                ui.label(format!(
                    "Assignment: Route {} · {seq_label} (step {}/{})",
                    visit.route_index + 1,
                    visit.step_index + 1,
                    visit.total_steps
                ));
                ui.label(format!(
                    "Vehicle / Drivers: {} · {}",
                    visit.vehicle_name, visit.driver_names
                ));
                ui.label(format!(
                    "Schedule: arrive {} · depart {}",
                    visit.arrive, visit.depart
                ));
                if visit.wait_minutes > 0 {
                    ui.label(format!("Wait before service: +{}m", visit.wait_minutes));
                }
                ui.label(format!(
                    "Leg: {:.1} km (cum {:.1} km)",
                    visit.prev_leg_km, visit.cumulative_km
                ));
                ui.label(format!(
                    "Cum. Load: {}u ({}A/{}C/{}F) of {}u cap",
                    visit.cumulative_load.total(),
                    visit.cumulative_load.ambient,
                    visit.cumulative_load.chilled,
                    visit.cumulative_load.frozen,
                    visit.vehicle_capacity.total()
                ));
            }
            render_store_specs(ui, *chain, demand, *service, windows, *from_chain_defaults);
        }
        MapNodeKind::UnassignedStore {
            chain,
            demand,
            service,
            windows,
            from_chain_defaults,
            reason,
        } => {
            ui.colored_label(
                crate::map_nodes::UNASSIGNED_COLOR,
                egui::RichText::new("Status: UNASSIGNED").strong(),
            );
            ui.colored_label(
                crate::map_nodes::UNASSIGNED_COLOR,
                format!("Why unassigned: {reason}"),
            );
            render_store_specs(ui, *chain, demand, *service, windows, *from_chain_defaults);
        }
    }
}

/// A stat card for summary strips (solution view and the KPIs tab).
pub(crate) fn stat_card(ui: &mut egui::Ui, label: &str, value: &str) {
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

pub(crate) fn store_stop_count(route: &domain::RoutePlan) -> usize {
    route
        .steps
        .iter()
        .filter(|stop| matches!(stop.node, domain::Node::Store(_)))
        .count()
}

pub(crate) fn vehicle_name<'a>(problem: &'a domain::Problem, route: &domain::RoutePlan) -> &'a str {
    problem
        .vehicles
        .get(route.vehicle.0 as usize)
        .map(|vehicle| vehicle.name.as_str())
        .unwrap_or("?")
}

pub(crate) fn driver_names(problem: &domain::Problem, route: &domain::RoutePlan) -> String {
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

/// Formats horizon minutes as `HH:MM`.
fn format_hhmm(minutes: i64) -> String {
    let clamped = minutes.max(0);
    format!("{:02}:{:02}", clamped / 60, clamped % 60)
}

/// Structured stop row data shared across the Solutions timeline grid, clipboard
/// manifest, and schedule CSV export.
struct StepRowDetails {
    node_type: &'static str,
    name: String,
    label: String,
    address_raw: String,
    location_text: String,
    lat: f64,
    lon: f64,
    wait_minutes: i64,
    service_minutes: i64,
    wait_svc_text: String,
    demand: domain::Load,
    demand_text: String,
    seq: Option<usize>,
}

fn step_row_details(
    problem: &domain::Problem,
    stop: &domain::PlannedStop,
    seq: Option<usize>,
) -> StepRowDetails {
    let label = node_label(problem, stop.node);
    match stop.node {
        domain::Node::Depot(id) => {
            let depot = problem.depots.get(id.0 as usize);
            let name = depot
                .map(|d| d.name.clone())
                .unwrap_or_else(|| format!("Depot #{}", id.0));
            let (lat, lon) = depot
                .map(|d| (d.location.lat, d.location.lon))
                .unwrap_or((0.0, 0.0));
            let address_raw = depot.and_then(|d| d.address.clone()).unwrap_or_default();
            let location_text = if address_raw.is_empty() {
                format!("{lat:.5}, {lon:.5}")
            } else {
                address_raw.clone()
            };
            StepRowDetails {
                node_type: "Depot",
                name,
                label,
                address_raw,
                location_text,
                lat,
                lon,
                wait_minutes: 0,
                service_minutes: 0,
                wait_svc_text: "—".to_string(),
                demand: domain::Load::ZERO,
                demand_text: "—".to_string(),
                seq: None,
            }
        }
        domain::Node::Store(id) => {
            let store = problem.store(id);
            let name = store
                .map(|s| s.name.clone())
                .unwrap_or_else(|| format!("Store #{}", id.0));
            let (lat, lon) = store
                .map(|s| (s.location.lat, s.location.lon))
                .unwrap_or((0.0, 0.0));
            let address_raw = store.and_then(|s| s.address.clone()).unwrap_or_default();
            let location_text = if address_raw.is_empty() {
                format!("{lat:.5}, {lon:.5}")
            } else {
                address_raw.clone()
            };
            let service_minutes = store.map(|s| s.service.minutes()).unwrap_or(0);
            let wait_minutes =
                (stop.depart.minutes() - service_minutes - stop.arrive.minutes()).max(0);
            let svc_label = store
                .map(|s| s.service.to_string())
                .unwrap_or_else(|| "0m".to_string());
            let wait_svc_text = if wait_minutes > 0 {
                format!("+{wait_minutes}m wait · {svc_label} svc")
            } else {
                svc_label
            };
            let demand = store.map(|s| s.demand).unwrap_or(domain::Load::ZERO);
            let demand_text = format!("{}/{}/{}", demand.ambient, demand.chilled, demand.frozen);
            StepRowDetails {
                node_type: "Store",
                name,
                label,
                address_raw,
                location_text,
                lat,
                lon,
                wait_minutes,
                service_minutes,
                wait_svc_text,
                demand,
                demand_text,
                seq,
            }
        }
    }
}

/// Formats a human-readable route manifest for clipboard/driver dispatch sharing.
fn format_route_manifest_text(bundle: &SolutionBundle) -> String {
    let served = served_store_count(&bundle.solution);
    let total = total_store_count(bundle);
    let mut out = format!(
        "{} — {served}/{total} stops served · {} route(s) · {:.1} km · ${:.2}\n",
        bundle.problem.name,
        bundle.solution.routes.len(),
        bundle.solution.total_distance_km,
        bundle.solution.total_cost_cents as f64 / 100.0
    );

    for (r_idx, route) in bundle.solution.routes.iter().enumerate() {
        out.push_str(&format!(
            "\nRoute {} ({}, {}) — {} stops · {:.1} km · {} · ${:.2}\n",
            r_idx + 1,
            vehicle_name(&bundle.problem, route),
            driver_names(&bundle.problem, route),
            store_stop_count(route),
            route.distance_km,
            route.duration,
            route.cost_cents as f64 / 100.0
        ));
        let mut seq = 0usize;
        for (step_idx, stop) in route.steps.iter().enumerate() {
            let seq_opt = if matches!(stop.node, domain::Node::Store(_)) {
                seq += 1;
                Some(seq)
            } else {
                None
            };
            let detail = step_row_details(&bundle.problem, stop, seq_opt);
            let seq_tag = detail.seq.map(|s| format!("#{s} ")).unwrap_or_default();
            out.push_str(&format!(
                "  {}. {seq_tag}[{}] {} · {}–{} ({}) · {} · Demand {}\n",
                step_idx + 1,
                detail.node_type,
                detail.label,
                stop.arrive,
                stop.depart,
                detail.wait_svc_text,
                detail.location_text,
                detail.demand_text
            ));
        }
    }

    if !bundle.solution.unassigned.is_empty() {
        out.push_str(&format!(
            "\nUnassigned ({}):\n",
            bundle.solution.unassigned.len()
        ));
        for store_id in &bundle.solution.unassigned {
            let reason = bundle
                .problem
                .store(*store_id)
                .map(|s| diagnose_unassigned_store(&bundle.problem, s))
                .unwrap_or_default();
            out.push_str(&format!(
                "  - {} — {reason}\n",
                store_name(&bundle.problem, *store_id)
            ));
        }
    }
    out
}

/// Serializes every route step into an RFC-4180 CSV dispatch schedule.
fn format_route_schedule_csv(bundle: &SolutionBundle) -> String {
    let mut out = String::from(
        "route,vehicle,drivers,step,stop_sequence,node_type,name,address,lat,lon,arrive,depart,wait_min,service_min,ambient,chilled,frozen\n",
    );
    for (r_idx, route) in bundle.solution.routes.iter().enumerate() {
        let route_label = format!("Route {}", r_idx + 1);
        let veh = vehicle_name(&bundle.problem, route);
        let drivers = driver_names(&bundle.problem, route);
        let mut seq = 0usize;
        for (step_idx, stop) in route.steps.iter().enumerate() {
            let seq_opt = if matches!(stop.node, domain::Node::Store(_)) {
                seq += 1;
                Some(seq)
            } else {
                None
            };
            let d = step_row_details(&bundle.problem, stop, seq_opt);
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{:.6},{:.6},{},{},{},{},{},{},{}\n",
                csv_escape(&route_label),
                csv_escape(veh),
                csv_escape(&drivers),
                step_idx + 1,
                d.seq.map(|s| s.to_string()).unwrap_or_default(),
                d.node_type,
                csv_escape(&d.name),
                csv_escape(&d.address_raw),
                d.lat,
                d.lon,
                stop.arrive,
                stop.depart,
                d.wait_minutes,
                d.service_minutes,
                d.demand.ambient,
                d.demand.chilled,
                d.demand.frozen
            ));
        }
    }
    out
}

/// Syncs a manually edited solution into all downstream tabs (`Solutions`, `KPIs`, `Map`).
impl ConsoleApp {
    fn sync_bundle_after_dispatch_edit(&mut self, updated: SolutionBundle, status_msg: String) {
        let updated_id = updated.id.clone();
        self.solution_status = Some(Ok(Box::new(updated.clone())));
        if let Some(Ok(kb)) = &mut self.kpis_status
            && kb.detail.id == updated_id
        {
            kb.detail = updated.clone();
            kb.kpis.served = served_store_count(&updated.solution);
            kb.kpis.serve_rate = if kb.kpis.total > 0 {
                kb.kpis.served as f64 / kb.kpis.total as f64
            } else {
                0.0
            };
            kb.kpis.distance_km = updated.solution.total_distance_km;
            kb.kpis.cost_cents = updated.solution.total_cost_cents;
        }
        if let Some(Ok(mb)) = &mut self.map_status
            && mb.id == updated_id
        {
            **mb = updated;
            self.map_geometry = SolutionGeometryData::default();
            self.map_geometry_error = None;
        }
        self.solution_export_status = Some((false, status_msg));
    }
}

/// Applies one interactive dispatch board action (`MoveStop`, `UnassignStop`,
/// or `AssignStoreToRoute`), recalculates route timing/costs, and updates raw JSON.
fn apply_dispatch_edit_to_bundle(
    bundle: &mut SolutionBundle,
    action: &SolutionViewAction,
) -> Option<String> {
    let msg = match *action {
        SolutionViewAction::MoveStop {
            route_idx,
            step_idx,
            delta,
        } => {
            let route = bundle.solution.routes.get_mut(route_idx)?;
            let target_isize = step_idx as isize + delta;
            if target_isize < 0 {
                return None;
            }
            let target_idx = target_isize as usize;
            if step_idx >= route.steps.len() || target_idx >= route.steps.len() {
                return None;
            }
            if !matches!(route.steps[step_idx].node, domain::Node::Store(_))
                || !matches!(route.steps[target_idx].node, domain::Node::Store(_))
            {
                return None;
            }
            route.steps.swap(step_idx, target_idx);
            recalculate_route_in_bundle(&bundle.problem, &mut bundle.solution, route_idx);
            format!(
                "Re-sequenced Route {} (recalculated distance, ETAs & CoR validation).",
                route_idx + 1
            )
        }
        SolutionViewAction::UnassignStop {
            route_idx,
            step_idx,
        } => {
            let route = bundle.solution.routes.get_mut(route_idx)?;
            let stop = route.steps.get(step_idx)?;
            let domain::Node::Store(store_id) = stop.node else {
                return None;
            };
            route.steps.remove(step_idx);
            if !bundle.solution.unassigned.contains(&store_id) {
                bundle.solution.unassigned.push(store_id);
            }
            recalculate_route_in_bundle(&bundle.problem, &mut bundle.solution, route_idx);
            format!(
                "Removed {} from Route {} and moved to Unassigned.",
                store_name(&bundle.problem, store_id),
                route_idx + 1
            )
        }
        SolutionViewAction::AssignStoreToRoute {
            store_id,
            route_idx,
        } => {
            let route = bundle.solution.routes.get_mut(route_idx)?;
            let insert_at = if route
                .steps
                .last()
                .is_some_and(|s| matches!(s.node, domain::Node::Depot(_)))
            {
                route.steps.len().saturating_sub(1)
            } else {
                route.steps.len()
            };
            route.steps.insert(
                insert_at,
                domain::PlannedStop {
                    node: domain::Node::Store(store_id),
                    arrive: domain::Minutes::ZERO,
                    depart: domain::Minutes::ZERO,
                },
            );
            bundle.solution.unassigned.retain(|id| *id != store_id);
            recalculate_route_in_bundle(&bundle.problem, &mut bundle.solution, route_idx);
            format!(
                "Assigned {} to Route {} (check CoR banner for window/capacity status).",
                store_name(&bundle.problem, store_id),
                route_idx + 1
            )
        }
        _ => return None,
    };
    bundle.raw = serde_json::to_value(&bundle.solution).unwrap_or_default();
    Some(msg)
}

/// Recomputes step ETAs, wait times, haversine leg distances, duration, and route
/// cost for `solution.routes[route_idx]`, then rolls totals back onto `solution`.
fn recalculate_route_in_bundle(
    problem: &domain::Problem,
    solution: &mut domain::Solution,
    route_idx: usize,
) {
    if let Some(route) = solution.routes.get_mut(route_idx) {
        let speed_kmh = problem
            .vehicles
            .get(route.vehicle.0 as usize)
            .map(|v| v.avg_speed_kmh)
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(50.0);
        let depart_start = route
            .drivers
            .first()
            .and_then(|d| problem.drivers.get(d.0 as usize))
            .and_then(|d| d.shifts.first())
            .map(|s| s.start)
            .or_else(|| route.steps.first().map(|s| s.depart))
            .unwrap_or(domain::Minutes::from_minutes(360));

        let mut total_km = 0.0_f64;
        let mut prev_loc: Option<domain::LatLon> = None;
        let mut prev_depart = depart_start;

        for (idx, step) in route.steps.iter_mut().enumerate() {
            let loc = match step.node {
                domain::Node::Depot(id) => problem.depots.get(id.0 as usize).map(|d| d.location),
                domain::Node::Store(id) => problem.store(id).map(|s| s.location),
            };
            if idx == 0 {
                step.arrive = depart_start;
                step.depart = depart_start;
                prev_depart = depart_start;
                prev_loc = loc;
                continue;
            }
            let leg_km = match (prev_loc, loc) {
                (Some(a), Some(b)) => domain::haversine_km(a, b),
                _ => 0.0,
            };
            total_km += leg_km;
            let travel_mins = (leg_km / speed_kmh * 60.0).round() as i64;
            let arrive = domain::Minutes::from_minutes(prev_depart.minutes() + travel_mins);
            let depart = match step.node {
                domain::Node::Depot(_) => arrive,
                domain::Node::Store(sid) => {
                    if let Some(store) = problem.store(sid) {
                        let wait_target = problem
                            .chain_rules
                            .windows_for_store(store)
                            .and_then(|wins| {
                                wins.iter()
                                    .filter(|w| w.end >= arrive)
                                    .map(|w| w.start)
                                    .min()
                            })
                            .unwrap_or(arrive);
                        let service_start = arrive.max(wait_target);
                        domain::Minutes::from_minutes(
                            service_start.minutes() + store.service.minutes().max(0),
                        )
                    } else {
                        arrive
                    }
                }
            };
            step.arrive = arrive;
            step.depart = depart;
            prev_depart = depart;
            if loc.is_some() {
                prev_loc = loc;
            }
        }

        let duration_min = (prev_depart.minutes() - depart_start.minutes()).max(0);
        route.distance_km = total_km;
        route.duration = domain::Minutes::from_minutes(duration_min);
        let dist_millikm = (total_km * 1000.0).round() as i64;
        let dist_cents = (dist_millikm * problem.cost.per_km_cents + 500) / 1000;
        let time_cents = (duration_min * problem.cost.per_hour_cents + 30) / 60;
        route.cost_cents = if route
            .steps
            .iter()
            .any(|s| matches!(s.node, domain::Node::Store(_)))
        {
            problem.cost.fixed_per_route_cents + dist_cents + time_cents
        } else {
            0
        };
    }

    solution.total_distance_km = solution.routes.iter().map(|r| r.distance_km).sum();
    solution.total_duration =
        domain::Minutes::from_minutes(solution.routes.iter().map(|r| r.duration.minutes()).sum());
    solution.total_cost_cents = solution.routes.iter().map(|r| r.cost_cents).sum();
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Renders a printable HTML TMS Driver Run Sheet / Bill of Lading (BOL) with
/// Chain of Responsibility (CoR) checklist and per-stop ePOD signature blocks.
fn format_driver_run_sheet_html(bundle: &SolutionBundle) -> String {
    let served = served_store_count(&bundle.solution);
    let total = total_store_count(bundle);
    let active_routes = bundle.solution.routes.len();
    let total_km = bundle.solution.total_distance_km;
    let total_cost = bundle.solution.total_cost_cents as f64 / 100.0;
    let est_fuel_l: f64 = bundle
        .solution
        .routes
        .iter()
        .map(|r| r.distance_km.max(0.0) * 0.33)
        .sum();
    let est_co2_kg = est_fuel_l * 2.68;

    let mut out = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>TMS Run Sheet &amp; BOL — {}</title>\n\
         <style>\
         body{{font-family:system-ui,-apple-system,sans-serif;margin:24px;color:#111;line-height:1.4;}}\
         h1,h2{{margin:0 0 8px 0;}}\
         .meta{{display:flex;flex-wrap:wrap;gap:16px;padding:12px;border:1px solid #cbd5e1;border-radius:6px;background:#f8fafc;margin-bottom:16px;}}\
         .cor{{border:2px solid #1e3a8a;border-radius:6px;padding:12px 16px;margin-bottom:20px;background:#eff6ff;}}\
         .cor ul{{list-style:none;padding:0;margin:8px 0 0 0;display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:6px;}}\
         .route{{margin-bottom:24px;page-break-inside:avoid;}}\
         table{{width:100%;border-collapse:collapse;margin-top:8px;font-size:13px;}}\
         th,td{{border:1px solid #cbd5e1;padding:6px 8px;text-align:left;vertical-align:top;}}\
         th{{background:#f1f5f9;}}\
         .epod{{min-width:150px;color:#64748b;}}\
         @media print{{body{{margin:10mm;}} a{{color:#111;text-decoration:none;}}}}\
         </style>\n</head>\n<body>\n\
         <h1>TMS Driver Run Sheet &amp; Bill of Lading (BOL)</h1>\n\
         <div class=\"meta\">\
         <div><strong>Network:</strong> {}</div>\
         <div><strong>Manifest ID:</strong> <code>{}</code></div>\
         <div><strong>Consignments:</strong> {served}/{total}</div>\
         <div><strong>Active Fleet:</strong> {active_routes}</div>\
         <div><strong>Distance:</strong> {total_km:.1} km</div>\
         <div><strong>Freight Cost:</strong> ${total_cost:.2}</div>\
         <div><strong>Est. ESG:</strong> {est_fuel_l:.1} L diesel ({est_co2_kg:.1} kg CO&#8322;e)</div>\
         </div>\n\
         <div class=\"cor\"><strong>Chain of Responsibility (CoR) Pre-Departure Verification</strong>\
         <ul>\
         <li>&#9744; GVM &amp; Axle Mass Limits Checked</li>\
         <li>&#9744; Route Height/Weight Clearance Verified</li>\
         <li>&#9744; Cargo Restraint &amp; Bulkhead Locked</li>\
         <li>&#9744; Chilled/Frozen Temperature Setpoints Verified</li>\
         <li>&#9744; NHVR Driver Fitness &amp; Work Diary Checked</li>\
         </ul></div>\n",
        html_escape(&bundle.problem.name),
        html_escape(&bundle.problem.name),
        html_escape(&bundle.id),
    );

    for (r_idx, route) in bundle.solution.routes.iter().enumerate() {
        let veh = vehicle_name(&bundle.problem, route);
        let drv = driver_names(&bundle.problem, route);
        let route_load = route.steps.iter().fold(domain::Load::ZERO, |acc, stop| {
            if let domain::Node::Store(sid) = stop.node
                && let Some(store) = bundle.problem.store(sid)
            {
                acc.saturating_add(store.demand)
            } else {
                acc
            }
        });
        out.push_str(&format!(
            "<div class=\"route\">\n<h2>Route {} &mdash; {} (Drivers: {})</h2>\n\
             <p><strong>Distance:</strong> {:.1} km &middot; <strong>Duration:</strong> {} &middot; \
             <strong>Cost:</strong> ${:.2} &middot; <strong>Total Cargo:</strong> {}A / {}C / {}F ({} units)</p>\n\
             <table>\n<thead><tr>\
             <th>Step</th><th>Consignment ID</th><th>Location / Customer</th>\
             <th>Address / Map Link</th><th>Arrive</th><th>Wait / Svc</th><th>Depart</th>\
             <th>Units (A/C/F)</th><th class=\"epod\">Receiver Name / Signature / Time (ePOD)</th>\
             </tr></thead>\n<tbody>\n",
            r_idx + 1,
            html_escape(veh),
            html_escape(&drv),
            route.distance_km,
            route.duration,
            route.cost_cents as f64 / 100.0,
            route_load.ambient,
            route_load.chilled,
            route_load.frozen,
            route_load.total(),
        ));

        let mut seq = 0usize;
        for (step_idx, stop) in route.steps.iter().enumerate() {
            let seq_opt = if matches!(stop.node, domain::Node::Store(_)) {
                seq += 1;
                Some(seq)
            } else {
                None
            };
            let d = step_row_details(&bundle.problem, stop, seq_opt);
            let consignment_id = match d.seq {
                Some(s) => format!("CON-R{:02}-{s:03}", r_idx + 1),
                None => "HUB".to_string(),
            };
            let osm_href = format!(
                "https://www.openstreetmap.org/?mlat={:.6}&amp;mlon={:.6}#map=16/{:.6}/{:.6}",
                d.lat, d.lon, d.lat, d.lon
            );
            out.push_str(&format!(
                "<tr><td>{}</td><td><code>{}</code></td><td>{}</td>\
                 <td><a href=\"{osm_href}\">{}</a></td>\
                 <td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"epod\"></td></tr>\n",
                step_idx + 1,
                consignment_id,
                html_escape(&d.label),
                html_escape(&d.location_text),
                stop.arrive,
                html_escape(&d.wait_svc_text),
                stop.depart,
                html_escape(&d.demand_text),
            ));
        }
        out.push_str("</tbody>\n</table>\n</div>\n");
    }

    out.push_str("</body>\n</html>\n");
    out
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
pub(crate) fn copy_button(ui: &mut egui::Ui, label: &str, text: &str) {
    if ui
        .button(label)
        .on_hover_text("Copy to the clipboard")
        .clicked()
    {
        ui.ctx().copy_text(text.to_owned());
    }
}

/// Search current working directory, executable directory, and parent folders
/// for `relative` so launched binaries inside `target/release/` still discover
/// repo data assets like `data/osm/nsw-latest.osm.pbf`.
fn discover_repo_path(relative: &str) -> Option<PathBuf> {
    let rel = PathBuf::from(relative);
    if rel.exists() {
        return Some(rel);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        roots.push(dir.to_path_buf());
    }
    for root in roots {
        let mut cursor = Some(root.as_path());
        for _ in 0..6 {
            if let Some(dir) = cursor {
                let candidate = dir.join(relative);
                if candidate.exists() {
                    return Some(candidate);
                }
                cursor = dir.parent();
            } else {
                break;
            }
        }
    }
    None
}

/// Forward a background message to the UI channel and request a repaint if an
/// `egui::Context` has already been captured.
fn send_msg(tx: &mpsc::Sender<Msg>, repaint: &OnceLock<egui::Context>, msg: Msg) {
    if tx.send(msg).is_ok()
        && let Some(ctx) = repaint.get()
    {
        ctx.request_repaint();
    }
}

/// `Some(text)` when a text field holds something.
fn non_empty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// Parse an optional positive finite numeric solver option; returns an inline
/// error if the user entered invalid text instead of silently ignoring it.
fn parse_optional_positive(text: &str, label: &str) -> Result<Option<f64>, anyhow::Error> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let val: f64 = trimmed
        .parse()
        .map_err(|_| anyhow::anyhow!("{label} must be a valid number"))?;
    if !val.is_finite() || val <= 0.0 {
        anyhow::bail!("{label} must be positive and finite");
    }
    Ok(Some(val))
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
async fn sse_loop(base_url: String, tx: mpsc::Sender<Msg>, repaint: Arc<OnceLock<egui::Context>>) {
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
                        send_msg(&tx, &repaint, Msg::SolveEvent(event));
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
                address: None,
                timezone: None,
            }],
            stores: vec![Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "Store 0".into(),
                location: domain::LatLon::new(-33.92, 151.23),
                address: None,
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
        app.kpis_ui.selected_route = Some(2);
        app.kpis_ui.action_status = Some((false, "stale".into()));
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

        assert_eq!(app.kpis_ui.selected_route, None);
        assert_eq!(app.kpis_ui.action_status, None);
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

    #[test]
    fn compute_fit_camera_uses_web_mercator_zoom() {
        let (lon, lat, zoom) = compute_fit_camera(151.10, 151.30, -33.95, -33.80, 1000.0, 700.0);
        assert!((lon - 151.20).abs() < 1e-9);
        assert!((lat - -33.875).abs() < 1e-9);
        // Sydney metro span (~18 km x ~17 km) fits around zoom 11 in a 1000x700 viewport.
        assert!(
            (10.0..=13.0).contains(&zoom),
            "expected metro Web Mercator zoom in 10..=13, got {zoom}"
        );

        // At higher latitude, the same degree span covers a larger Mercator y span -> lower zoom.
        let (_, _, polar_zoom) = compute_fit_camera(151.10, 151.30, 59.85, 60.00, 1000.0, 700.0);
        assert!(polar_zoom < zoom);
    }

    #[test]
    fn parse_optional_positive_validates_inputs() {
        assert_eq!(parse_optional_positive("   ", "Height").unwrap(), None);
        assert_eq!(parse_optional_positive("4.3", "Height").unwrap(), Some(4.3));
        assert!(parse_optional_positive("0", "Height").is_err());
        assert!(parse_optional_positive("-1.5", "Weight").is_err());
        assert!(parse_optional_positive("NaN", "Weight").is_err());
        assert!(parse_optional_positive("abc", "Weight").is_err());
    }

    #[tokio::test]
    async fn builder_actions_populate_and_sync_json_box() {
        let mut app = ConsoleApp::new(Handle::current(), "http://127.0.0.1:8080".to_string());
        app.problem_builder.custom_name = "Test Scenario".into();
        app.problem_builder.stores = 5;
        app.problem_builder.depots = 1;
        app.problem_builder.vehicles = 2;

        app.handle_builder_action(BuilderAction::PopulateScenario);
        assert!(app.problem_builder.editor_error.is_none());
        let parsed: domain::Problem = serde_json::from_str(&app.import_json).unwrap();
        assert_eq!(parsed.name, "Test Scenario");
        assert_eq!(parsed.stores.len(), 5);

        app.problem_builder.custom_name.clear();
        app.handle_builder_action(BuilderAction::NewCustom);
        let reset: domain::Problem = serde_json::from_str(&app.import_json).unwrap();
        assert_eq!(reset.depots.len(), 1);
        assert_eq!(reset.stores.len(), 1);
        assert_eq!(reset.vehicles.len(), 1);
    }

    #[tokio::test]
    async fn lookup_coords_and_geocode_response_update_custom_depot_and_stops() {
        let mut app = ConsoleApp::new(Handle::current(), "http://127.0.0.1:8080".to_string());

        // Offline coordinate input updates Main Depot directly without spawning network work.
        app.handle_builder_action(BuilderAction::LookupAddress {
            target: GeocodeTarget::Depot,
            query: "-37.8136, 144.9631".into(),
            add_stop_if_single: false,
        });
        assert!((app.problem_builder.draft.depots[0].location.lat - -37.8136).abs() < 1e-6);

        // Simulated geocoded stop response sets coordinates & preserves human-readable address.
        app.problem_builder.stop_name_input = "Hardware Warehouse".into();
        app.problem_builder.lookup_pending_add_stop = true;
        app.handle_msg(Msg::GeocodeCompleted(Ok(GeocodeResponse {
            target: GeocodeTarget::Stop,
            query: "200 Bourke St Melbourne".into(),
            matches: vec![GeocodeMatch {
                label: "200 Bourke Street, Melbourne VIC 3000".into(),
                lat: -37.8129,
                lon: 144.9675,
                source: "OpenStreetMap".into(),
            }],
        })));
        let parsed: domain::Problem = serde_json::from_str(&app.import_json).unwrap();
        // First real stop replaces the untouched "Stop 1" starter placeholder.
        assert_eq!(parsed.stores.len(), 1);
        assert_eq!(parsed.stores[0].name, "Hardware Warehouse");
        assert_eq!(parsed.stores[0].chain, domain::Chain::General);
        assert_eq!(
            parsed.stores[0].address.as_deref(),
            Some("200 Bourke Street, Melbourne VIC 3000")
        );
    }

    #[tokio::test]
    async fn solve_route_now_sets_auto_pipeline_and_exports_schedule() {
        let mut app = ConsoleApp::new(Handle::current(), "http://127.0.0.1:8080".to_string());
        assert!(app.solve_ls);

        app.handle_builder_action(BuilderAction::ImportAndSolve);
        assert!(app.auto_solve_after_import);
        assert!(app.open_map_after_solve);

        app.handle_msg(Msg::ProblemImported(Ok("prob-auto".into())));
        assert!(!app.auto_solve_after_import);
        assert_eq!(app.problem_id_input, "prob-auto");
        assert!(app.solve_in_flight);

        let mut bundle = test_solution_bundle();
        bundle.solution.routes[0].steps = vec![
            domain::PlannedStop {
                node: domain::Node::Depot(domain::DepotId(0)),
                arrive: domain::Minutes::from_minutes(360),
                depart: domain::Minutes::from_minutes(360),
            },
            domain::PlannedStop {
                node: domain::Node::Store(domain::StoreId(0)),
                arrive: domain::Minutes::from_minutes(375),
                depart: domain::Minutes::from_minutes(390),
            },
            domain::PlannedStop {
                node: domain::Node::Depot(domain::DepotId(0)),
                arrive: domain::Minutes::from_minutes(405),
                depart: domain::Minutes::from_minutes(405),
            },
        ];

        let csv = format_route_schedule_csv(&bundle);
        assert!(csv.starts_with("route,vehicle,drivers,step,stop_sequence,"));
        assert!(csv.contains(",1,Store,Store 0,"));

        let manifest = format_route_manifest_text(&bundle);
        assert!(manifest.contains("Route 1 (Rigid, Sam)"));
        assert!(manifest.contains("#1 [Store] Store 0 (#0)"));
    }

    #[test]
    fn dispatch_board_edits_resequence_assign_and_render_bol_html() {
        use domain::{LatLon, Load, Minutes, PlannedStop, Store, StoreId, TimeWindow};

        let mut bundle = test_solution_bundle();
        // Add a second route store + an unassigned store so we can swap, unassign, and re-assign.
        bundle.problem.stores.push(Store {
            id: StoreId(1),
            chain: domain::Chain::General,
            name: "Store 1".into(),
            location: LatLon::new(-33.95, 151.25),
            address: Some("10 Market St".into()),
            timezone: None,
            demand: Load::new(2, 1, 0),
            service: Minutes::from_minutes(10),
            receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
        });
        bundle.problem.stores.push(Store {
            id: StoreId(2),
            chain: domain::Chain::General,
            name: "Store 2".into(),
            location: LatLon::new(-33.89, 151.19),
            address: None,
            timezone: None,
            demand: Load::new(1, 0, 0),
            service: Minutes::from_minutes(10),
            receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
        });
        bundle.solution.routes[0].steps = vec![
            PlannedStop {
                node: domain::Node::Depot(domain::DepotId(0)),
                arrive: Minutes::ZERO,
                depart: Minutes::ZERO,
            },
            PlannedStop {
                node: domain::Node::Store(StoreId(0)),
                arrive: Minutes::ZERO,
                depart: Minutes::ZERO,
            },
            PlannedStop {
                node: domain::Node::Store(StoreId(1)),
                arrive: Minutes::ZERO,
                depart: Minutes::ZERO,
            },
            PlannedStop {
                node: domain::Node::Depot(domain::DepotId(0)),
                arrive: Minutes::ZERO,
                depart: Minutes::ZERO,
            },
        ];
        bundle.solution.unassigned = vec![StoreId(2)];

        // Move Store 0 down past Store 1.
        let msg = apply_dispatch_edit_to_bundle(
            &mut bundle,
            &SolutionViewAction::MoveStop {
                route_idx: 0,
                step_idx: 1,
                delta: 1,
            },
        )
        .unwrap();
        assert!(msg.contains("Re-sequenced Route 1"));
        assert_eq!(
            bundle.solution.routes[0].steps[1].node,
            domain::Node::Store(StoreId(1))
        );
        assert_eq!(
            bundle.solution.routes[0].steps[2].node,
            domain::Node::Store(StoreId(0))
        );
        assert!(bundle.solution.total_distance_km > 5.0);

        // Assign unassigned Store 2 into Route 1 before return depot.
        apply_dispatch_edit_to_bundle(
            &mut bundle,
            &SolutionViewAction::AssignStoreToRoute {
                store_id: StoreId(2),
                route_idx: 0,
            },
        )
        .unwrap();
        assert!(bundle.solution.unassigned.is_empty());
        assert_eq!(bundle.solution.routes[0].steps.len(), 5);
        assert_eq!(
            bundle.solution.routes[0].steps[3].node,
            domain::Node::Store(StoreId(2))
        );

        let html = format_driver_run_sheet_html(&bundle);
        assert!(html.contains("TMS Driver Run Sheet &amp; Bill of Lading (BOL)"));
        assert!(html.contains("Chain of Responsibility (CoR) Pre-Departure Verification"));
        assert!(html.contains("CON-R01-001"));
        assert!(html.contains("CON-R01-003"));
        assert!(html.contains("Receiver Name / Signature / Time (ePOD)"));
    }

    #[test]
    fn discover_repo_path_finds_workspace_asset() {
        assert!(discover_repo_path("Cargo.toml").is_some());
        assert!(discover_repo_path("data/demo/problem.json").is_some());
        assert!(discover_repo_path("nonexistent/path/missing.file").is_none());
    }
}
