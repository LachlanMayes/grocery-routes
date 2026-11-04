//! Main egui application state and UI.
//!
//! Provides a tabbed operator console:
//! - **Import**: paste a problem JSON and POST it to the API
//! - **Solve**: select a problem, choose solver options, kick off a solve
//! - **Solutions**: list and inspect solved plans
//! - **KPIs**: view KPI summary for a solution
//! - **Live Events**: SSE stream of solve completions

use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::api_client::{ApiClient, Kpis, SolveRequest, SolveResponse};
use eframe::egui;

/// Messages passed from background tasks to the UI thread.
enum Msg {
    ProblemImported(Result<String, anyhow::Error>),
    SolutionSolved(Result<SolveResponse, anyhow::Error>),
    SolutionFetched(Result<serde_json::Value, anyhow::Error>),
    KpisFetched(Result<Kpis, anyhow::Error>),
    SolveEvent(crate::api_client::SolveEvent),
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
    client: ApiClient,

    // Navigation
    tab: Tab,

    // Import tab state
    import_json: String,
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
    solution_status: Option<Result<serde_json::Value, anyhow::Error>>,

    // KPIs tab state
    kpis_id_input: String,
    kpis_status: Option<Result<Kpis, anyhow::Error>>,

    // Events tab state
    events: Vec<SolveEventDisplay>,
    max_events: usize,

    // Background message channel
    msg_rx: mpsc::Receiver<Msg>,
    msg_tx: mpsc::Sender<Msg>,

    // Last SSE poll time
    last_event_poll: Instant,
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

impl Default for ConsoleApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            client: ApiClient::default_local(),
            tab: Tab::Import,
            import_json: String::new(),
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
            kpis_id_input: String::new(),
            kpis_status: None,
            events: Vec::new(),
            max_events: 50,
            msg_rx: rx,
            msg_tx: tx,
            last_event_poll: Instant::now(),
        }
    }
}

impl eframe::App for ConsoleApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain any messages from background tasks
        while let Ok(msg) = self.msg_rx.try_recv() {
            self.handle_msg(msg);
        }

        // Poll SSE for live events (every 2 seconds)
        if self.last_event_poll.elapsed() > Duration::from_secs(2) {
            self.last_event_poll = Instant::now();
            let tx = self.msg_tx.clone();
            let client = self.client.clone();
            tokio::task::spawn(async move {
                if let Ok(resp) = client.events_stream().await {
                    if let Ok(text) = resp.text().await {
                        for line in text.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if let Ok(event) =
                                    serde_json::from_str::<crate::api_client::SolveEvent>(data)
                                {
                                    let _ = tx.send(Msg::SolveEvent(event));
                                }
                            }
                        }
                    }
                }
            });
        }

        egui::Window::new("sidebar").show(ctx, |ui| {
            self.render_sidebar(ui);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_content(ui);
        });

        ctx.request_repaint_after(Duration::from_millis(100));
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
            self.client = ApiClient::default_local();
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
            Msg::ProblemImported(res) => self.import_status = Some(res),
            Msg::SolutionSolved(res) => self.solve_status = Some(res),
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
        ui.label("Paste a problem JSON document (serialized [`domain::Problem`]):");

        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .id_salt("import_scroll")
            .max_height(300.0)
            .show(ui, |ui| {
                ui.text_edit_multiline(&mut self.import_json);
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Import Problem").clicked() {
                let json = self.import_json.clone();
                let tx = self.msg_tx.clone();
                let client = self.client.clone();
                self.import_status = None;
                tokio::task::spawn(async move {
                    let val: serde_json::Value = match serde_json::from_str(&json) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = tx.send(Msg::ProblemImported(Err(anyhow::anyhow!(
                                "Invalid JSON: {e}"
                            ))));
                            return;
                        }
                    };
                    match client.import_problem(&val).await {
                        Ok(resp) => {
                            let _ = tx.send(Msg::ProblemImported(Ok(resp.id)));
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::ProblemImported(Err(e)));
                        }
                    }
                });
            }
            if ui.button("Clear").clicked() {
                self.import_json.clear();
                self.import_status = None;
            }
        });

        if let Some(ref status) = self.import_status {
            ui.add_space(4.0);
            match status {
                Ok(id) => {
                    ui.colored_label(egui::Color32::GREEN, format!("Imported: {id}"));
                }
                Err(e) => {
                    ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
                }
            }
        }
    }

    // ── Solve tab ───────────────────────────────────────────────────────

    fn render_solve_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Solve Problem");
        ui.label("Enter a problem ID and choose solver options:");

        ui.add_space(8.0);
        ui.label("Problem ID:");
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
            let pid = self.problem_id_input.clone();
            let lns = self.solve_lns;
            let osm = if self.osm_path.is_empty() {
                None
            } else {
                Some(self.osm_path.clone())
            };
            let mw = if self.maxweight.is_empty() {
                None
            } else {
                self.maxweight.parse::<f64>().ok()
            };
            let mh = if self.maxheight.is_empty() {
                None
            } else {
                self.maxheight.parse::<f64>().ok()
            };
            let hgv = self.hgv;
            let tx = self.msg_tx.clone();
            let client = self.client.clone();
            self.solve_status = None;
            tokio::task::spawn(async move {
                let req = SolveRequest {
                    problem_id: pid,
                    lns,
                    osm_path: osm,
                    maxweight: mw,
                    maxheight: mh,
                    hgv,
                };
                match client.solve(&req).await {
                    Ok(resp) => {
                        let _ = tx.send(Msg::SolutionSolved(Ok(resp)));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::SolutionSolved(Err(e)));
                    }
                }
            });
        }

        ui.add_space(8.0);
        if let Some(ref status) = self.solve_status {
            match status {
                Ok(resp) => {
                    ui.colored_label(
                        egui::Color32::GREEN,
                        format!(
                            "Solved: {} routes, {} of {} served, {:.1} km",
                            resp.id, resp.served, resp.total, resp.distance_km
                        ),
                    );
                }
                Err(e) => {
                    ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
                }
            }
        }
    }

    // ── Solutions tab ───────────────────────────────────────────────────

    fn render_solutions_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("View Solution");
        ui.label("Enter a solution ID to retrieve it:");

        ui.add_space(8.0);
        ui.label("Solution ID:");
        ui.text_edit_singleline(&mut self.solution_id_input);

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Fetch Solution").clicked() {
                let id = self.solution_id_input.clone();
                let tx = self.msg_tx.clone();
                let client = self.client.clone();
                self.solution_status = None;
                tokio::task::spawn(async move {
                    match client.get_solution(&id).await {
                        Ok(resp) => {
                            let _ = tx.send(Msg::SolutionFetched(Ok(resp.solution)));
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::SolutionFetched(Err(e)));
                        }
                    }
                });
            }
        });

        ui.add_space(8.0);
        match &self.solution_status {
            Some(Ok(sol)) => {
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(serde_json::to_string_pretty(sol).unwrap_or_default());
                });
            }
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::RED, format!("Error: {e}"));
            }
            None => {}
        }
    }

    // ── KPIs tab ────────────────────────────────────────────────────────

    fn render_kpis_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Solution KPIs");
        ui.label("Enter a solution ID to view KPIs:");

        ui.add_space(8.0);
        ui.label("Solution ID:");
        ui.text_edit_singleline(&mut self.kpis_id_input);

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Fetch KPIs").clicked() {
                let id = self.kpis_id_input.clone();
                let tx = self.msg_tx.clone();
                let client = self.client.clone();
                self.kpis_status = None;
                tokio::task::spawn(async move {
                    match client.get_kpis(&id).await {
                        Ok(kpis) => {
                            let _ = tx.send(Msg::KpisFetched(Ok(kpis)));
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::KpisFetched(Err(e)));
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
