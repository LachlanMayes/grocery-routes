//! Main egui application state and UI.
//!
//! Provides a tabbed operator console:
//! - **Import**: paste a problem JSON and POST it to the API
//! - **Solve**: select a problem, choose solver options, kick off a solve
//! - **Solutions**: list and inspect solved plans
//! - **KPIs**: view KPI summary for a solution
//! - **Live Events**: SSE stream of solve completions

use std::result::Result;
use std::sync::mpsc;
use std::time::Duration;

use crate::api_client::{
    ApiClient, Kpis, ProblemSummary, SolutionSummary, SolveRequest, SolveResponse, from_json,
};
use eframe::egui;
use tokio::runtime::Handle;
use tokio_stream::StreamExt;

/// Messages passed from background tasks to the UI thread.
enum Msg {
    ProblemImported(Result<String, anyhow::Error>),
    SolutionSolved(Result<SolveResponse, anyhow::Error>),
    SolutionFetched(Result<Box<SolutionBundle>, anyhow::Error>),
    KpisFetched(Result<Kpis, anyhow::Error>),
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

/// Tab selection for the console.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Import,
    Solve,
    Solutions,
    Kpis,
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

    // Solutions tab state
    solution_id_input: String,
    solution_status: Option<Result<Box<SolutionBundle>, anyhow::Error>>,
    selected_route: usize,

    // KPIs tab state
    kpis_id_input: String,
    kpis_status: Option<Result<Kpis, anyhow::Error>>,

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
            solution_id_input: String::new(),
            solution_status: None,
            selected_route: 0,
            kpis_id_input: String::new(),
            kpis_status: None,
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
                if let Ok(resp) = &res {
                    self.solution_id_input.clone_from(&resp.id);
                    self.kpis_id_input.clone_from(&resp.id);
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
        if ui.button("Solve").clicked() {
            self.start_solve();
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
                let tx = self.msg_tx.clone();
                let client = self.client.clone();
                self.solution_status = None;
                self.selected_route = 0;
                self.runtime.spawn(async move {
                    let result = async {
                        let resp = client.get_solution(&id).await?;
                        let problem_raw = client.get_problem(&resp.problem_id).await?;
                        let problem: domain::Problem = from_json(&problem_raw)?;
                        let solution: domain::Solution = from_json(&resp.solution)?;
                        Ok::<_, anyhow::Error>(Box::new(SolutionBundle {
                            id,
                            problem_id: resp.problem_id,
                            problem,
                            solution,
                            raw: resp.solution,
                        }))
                    }
                    .await;
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
                    match client.get_kpis(&id).await {
                        Ok(kpis) => {
                            tx.send(Msg::KpisFetched(Ok(kpis))).ok();
                        }
                        Err(e) => {
                            tx.send(Msg::KpisFetched(Err(e))).ok();
                        }
                    }
                });
            }
        });

        ui.add_space(8.0);
        match &self.kpis_status {
            Some(Ok(kpis)) => {
                ui.label(format!("Served: {} / {}", kpis.served, kpis.total));
                ui.label(format!("Serve rate: {:.1}%", kpis.serve_rate * 100.0));
                ui.label(format!("Distance: {:.1} km", kpis.distance_km));
                ui.label(format!("Routes: {}", kpis.routes));
                ui.label(format!("Cost: {:.0} cents", kpis.cost_cents));
            }
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
            }
            None => {}
        }
    }

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

        app.handle_msg(Msg::KpisFetched(Ok(kpis)));

        match app.kpis_status {
            Some(Ok(kpis)) => assert_eq!(kpis.routes, 1),
            other => panic!("unexpected KPI status: {other:?}"),
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
