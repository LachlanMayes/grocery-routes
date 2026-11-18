//! HTTP request handlers for the M4 product shell API.
//!
//! Endpoints:
//! - POST /problems - import a problem JSON
//! - GET /problems - list recent imported problems (summaries)
//! - GET /problems/{id} - retrieve a problem
//! - POST /solve - solve a problem (greedy or LNS, optional OSM routing)
//! - GET /solutions - list recent solutions (summaries, optional ?problem_id filter)
//! - GET /solutions/{id} - retrieve a solution
//! - GET /solutions/{id}/geometry - road polylines per route
//! - GET /solutions/{id}/kpis - KPI summary for a solution
//! - GET /events - SSE stream of live solve events

use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response, sse::Event, sse::Sse},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::result::Result;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, OnceCell, broadcast};
use tokio_stream::{Stream, StreamExt, wrappers::BroadcastStream};
use tower_http::limit::RequestBodyLimitLayer;

use crate::db::{
    StoredRoutingProfile, import_problem, list_problems, list_solutions, load_problem,
    load_solution_with_problem, solution_routing_params, store_solution_with_profile,
};
use domain::{LatLon, Problem, Solution};
use routing::{CachedMatrix, MatrixStrategy, TruckProfile};

/// Error returned from HTTP handlers, preserving the HTTP status code while
/// carrying a human-readable diagnostic message in the response body.
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn internal(err: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

type OsmCacheEntry = Arc<OnceCell<Arc<Mutex<CachedMatrix>>>>;

/// An event broadcast to SSE clients when a solve completes.
#[derive(Clone, Debug, Serialize)]
pub struct SolveEvent {
    pub solution_id: String,
    pub problem_id: String,
    pub served: usize,
    pub total: usize,
    pub distance_km: f64,
    pub cost_cents: i64,
}

/// Shared application state: the SQLite connection pool, an SSE broadcast
/// channel for live solve events, and a cache of loaded OSM graphs keyed by
/// canonical PBF path.
#[derive(Debug)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub events: broadcast::Sender<SolveEvent>,
    osm_caches: AsyncMutex<HashMap<PathBuf, OsmCacheEntry>>,
}

/// Import request body: a serialized [`domain::Problem`].
#[derive(Deserialize, Debug)]
pub struct ImportProblem {
    pub problem: Problem,
}

/// Import response.
#[derive(Serialize, Debug)]
pub struct ImportResponse {
    pub id: String,
}

/// Solve request body.
#[derive(Deserialize, Debug)]
pub struct SolveRequest {
    /// Problem id (must exist in the database).
    pub problem_id: String,
    /// Use LNS/ALNS metaheuristic instead of greedy.
    #[serde(default)]
    pub lns: bool,
    /// Use local search when `lns` is false.
    #[serde(default)]
    pub local_search: bool,
    /// Use Contraction Hierarchies instead of Dijkstra for OSM matrix queries.
    #[serde(default)]
    pub ch: bool,
    /// Use OSM PBF for road-network distances/durations (optional).
    pub osm_path: Option<String>,
    /// Maximum vehicle weight in tonnes.
    pub maxweight: Option<f64>,
    /// Maximum vehicle height in metres.
    pub maxheight: Option<f64>,
    /// Classify vehicle as HGV.
    #[serde(default)]
    pub hgv: bool,
}

/// Solve response.
#[derive(Serialize, Debug)]
pub struct SolveResponse {
    pub id: String,
    pub served: usize,
    pub total: usize,
    pub distance_km: f64,
    pub cost_cents: i64,
}

/// Solution response.
#[derive(Serialize, Debug)]
pub struct SolutionResponse {
    pub id: String,
    pub problem_id: String,
    pub solution: Solution,
}

/// KPI summary.
#[derive(Serialize, Debug)]
pub struct Kpis {
    pub served: usize,
    pub total: usize,
    pub serve_rate: f64,
    pub distance_km: f64,
    pub cost_cents: i64,
    pub routes: usize,
}

/// One leg between consecutive stops along a route.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RouteLegGeometry {
    pub points: Vec<[f64; 2]>,
    #[serde(default)]
    pub fallback: bool,
}

/// Road geometry for a solution: one flattened polyline per route (`routes`),
/// plus per-leg polylines (`route_legs`) so clients can render disconnected
/// straight-line fallbacks differently from routed road segments.
#[derive(Serialize, Debug)]
pub struct SolutionGeometry {
    pub routes: Vec<Vec<[f64; 2]>>,
    #[serde(default)]
    pub route_legs: Vec<Vec<RouteLegGeometry>>,
}

/// Summary of one imported problem, for GET /problems.
#[derive(Serialize, Debug)]
pub struct ProblemSummary {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub stores: usize,
    pub vehicles: usize,
    pub depots: usize,
}

/// Summary of one solved solution, for GET /solutions.
#[derive(Serialize, Debug)]
pub struct SolutionSummary {
    pub id: String,
    pub problem_id: String,
    pub served: i64,
    pub total: i64,
    pub distance_km: f64,
    pub cost_cents: i64,
    pub solved_at: String,
    pub used_lns: bool,
}

/// Query parameters for the list endpoints. `limit` defaults to 20 and is
/// clamped to 1..=100 by the db layer; `problem_id` is only meaningful for
/// GET /solutions.
#[derive(Deserialize, Debug, Default)]
pub struct ListParams {
    pub limit: Option<u32>,
    pub problem_id: Option<String>,
}

/// Count the store stops served across a solution's routes.
fn served_store_count(solution: &domain::Solution) -> usize {
    solution
        .routes
        .iter()
        .flat_map(|r| &r.steps)
        .filter(|s| matches!(s.node, domain::Node::Store(_)))
        .count()
}

/// Build the axum router with all API routes.
pub fn router(pool: sqlx::SqlitePool) -> Router {
    let (events, _rx) = broadcast::channel(16);
    let state = Arc::new(AppState {
        pool,
        events,
        osm_caches: AsyncMutex::new(HashMap::new()),
    });
    Router::new()
        .route(
            "/problems",
            post(import_problem_handler).get(list_problems_handler),
        )
        .route("/problems/{id}", get(get_problem_handler))
        .route("/solve", post(solve_handler))
        .route("/solutions", get(list_solutions_handler))
        .route("/solutions/{id}", get(get_solution_handler))
        .route(
            "/solutions/{id}/geometry",
            get(get_solution_geometry_handler),
        )
        .route("/solutions/{id}/kpis", get(get_kpis_handler))
        .route("/events", get(sse_events_handler))
        .layer(RequestBodyLimitLayer::new(16 * 1024 * 1024))
        .with_state(state)
}

/// Ensures ids are exactly the contiguous 0..len indices the domain model
/// assumes, so duplicate or out-of-range ids cannot panic the solver.
fn check_dense_ids(kind: &str, len: usize, ids: impl Iterator<Item = u32>) -> Result<(), ApiError> {
    let mut seen = vec![false; len];
    for id in ids {
        let index = id as usize;
        if index >= len || seen[index] {
            return Err(ApiError::bad_request(format!(
                "invalid {kind} id {id}: expected dense 0..{len}"
            )));
        }
        seen[index] = true;
    }
    Ok(())
}

/// Hard ceiling on total nodes (depots + stores) matching the CLI solver limit.
const MAX_SOLVE_NODES: usize = 5_000;

/// Checks that a geographic coordinate is finite and within WGS-84 bounds.
fn check_valid_location(kind: &str, id: u32, loc: LatLon) -> Result<(), ApiError> {
    if !loc.lat.is_finite()
        || !loc.lon.is_finite()
        || !(-90.0..=90.0).contains(&loc.lat)
        || !(-180.0..=180.0).contains(&loc.lon)
    {
        return Err(ApiError::bad_request(format!(
            "{kind} {id} has invalid location ({}, {})",
            loc.lat, loc.lon
        )));
    }
    Ok(())
}

/// Checks an optional positive finite truck profile limit (`maxweight`, `maxheight`).
fn check_positive_limit(name: &str, value: Option<f64>) -> Result<(), ApiError> {
    if let Some(limit) = value
        && (!limit.is_finite() || limit <= 0.0)
    {
        return Err(ApiError::bad_request(format!(
            "{name} must be finite and > 0 (got {limit})"
        )));
    }
    Ok(())
}

/// Validates the structure the solver relies on. Returns BAD_REQUEST when it
/// cannot produce a valid plan.
fn validate_problem(problem: &Problem) -> Result<(), ApiError> {
    let node_count = problem.depots.len() + problem.stores.len();
    if node_count > MAX_SOLVE_NODES {
        return Err(ApiError::bad_request(format!(
            "problem has {node_count} nodes (depots + stores), exceeding maximum {MAX_SOLVE_NODES}"
        )));
    }

    check_dense_ids(
        "depot",
        problem.depots.len(),
        problem.depots.iter().map(|depot| depot.id.0),
    )?;
    check_dense_ids(
        "store",
        problem.stores.len(),
        problem.stores.iter().map(|store| store.id.0),
    )?;
    check_dense_ids(
        "vehicle",
        problem.vehicles.len(),
        problem.vehicles.iter().map(|vehicle| vehicle.id.0),
    )?;
    check_dense_ids(
        "driver",
        problem.drivers.len(),
        problem.drivers.iter().map(|driver| driver.id.0),
    )?;

    for depot in &problem.depots {
        check_valid_location("depot", depot.id.0, depot.location)?;
    }
    for store in &problem.stores {
        check_valid_location("store", store.id.0, store.location)?;
    }

    if let Some(vehicle) = problem
        .vehicles
        .iter()
        .find(|vehicle| !vehicle.avg_speed_kmh.is_finite() || vehicle.avg_speed_kmh <= 0.0)
    {
        return Err(ApiError::bad_request(format!(
            "vehicle {} has non-positive or non-finite avg_speed_kmh ({})",
            vehicle.id.0, vehicle.avg_speed_kmh
        )));
    }

    if problem
        .vehicles
        .iter()
        .any(|vehicle| vehicle.depot.0 as usize >= problem.depots.len())
    {
        return Err(ApiError::bad_request("vehicle references unknown depot id"));
    }
    if problem.stores.iter().any(|store| {
        store
            .receiving
            .iter()
            .any(|window| window.start > window.end)
    }) {
        return Err(ApiError::bad_request(
            "store has a receiving window with start > end",
        ));
    }
    if problem
        .drivers
        .iter()
        .any(|driver| driver.shifts.iter().any(|shift| shift.start >= shift.end))
    {
        return Err(ApiError::bad_request(
            "driver has a shift with start >= end",
        ));
    }

    // Problem-level configuration (invalid IANA timezone names) cannot be
    // repaired by the solver; reject at the boundary.
    let bad_tzs = problem.validate_timezones();
    if !bad_tzs.is_empty() {
        return Err(ApiError::bad_request(format!(
            "invalid timezone(s): {}",
            bad_tzs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    Ok(())
}

/// Resolves `osm_path` to a canonical file. When GROCERY_OSM_ROOT is set, the
/// resolved file must remain under that directory.
fn validate_osm_path(osm_path: &str) -> Result<PathBuf, ApiError> {
    let path = PathBuf::from(osm_path);
    let path = path
        .canonicalize()
        .map_err(|e| ApiError::bad_request(format!("cannot open OSM path {osm_path}: {e}")))?;
    if !path.is_file() {
        return Err(ApiError::bad_request(format!(
            "OSM path is not a file: {osm_path}"
        )));
    }

    let is_pbf = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase().ends_with(".osm.pbf"))
        .unwrap_or(false);
    if !is_pbf {
        return Err(ApiError::bad_request(format!(
            "OSM path must end in .osm.pbf: {osm_path}"
        )));
    }

    if let Some(root) = std::env::var_os("GROCERY_OSM_ROOT") {
        let root = PathBuf::from(root)
            .canonicalize()
            .map_err(|e| ApiError::bad_request(format!("invalid GROCERY_OSM_ROOT: {e}")))?;
        if !path.starts_with(root) {
            return Err(ApiError::bad_request(
                "OSM path is outside GROCERY_OSM_ROOT",
            ));
        }
    }

    Ok(path)
}

/// Returns the cached OSM graph for `osm_path`, loading it in the blocking
/// thread pool on first use without holding the cache-map mutex while parsing.
async fn osm_cache_for(
    state: &Arc<AppState>,
    osm_path: &str,
) -> Result<Arc<Mutex<CachedMatrix>>, ApiError> {
    let path = validate_osm_path(osm_path)?;
    let cell = {
        let mut caches = state.osm_caches.lock().await;
        caches
            .entry(path.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone()
    };

    let cache = cell
        .get_or_try_init(|| async move {
            tokio::task::spawn_blocking(move || -> anyhow::Result<CachedMatrix> {
                let mut cache = CachedMatrix::new();
                cache.load_pbf(&path)?;
                Ok(cache)
            })
            .await
            .map_err(ApiError::internal)?
            .map(|cache| Arc::new(Mutex::new(cache)))
            .map_err(|err| ApiError::bad_request(err.to_string()))
        })
        .await?;

    Ok(Arc::clone(cache))
}

/// GET /events — SSE stream of live solve events.
async fn sse_events_handler(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Box<dyn std::error::Error + Send + Sync>>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).map(|result| match result {
        Ok(event) => {
            let json = serde_json::to_string(&event).unwrap_or_default();
            Ok(Event::default().data(json))
        }
        Err(e) => Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
    });
    Sse::new(stream)
}

/// POST /problems — import a problem JSON.
async fn import_problem_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ImportProblem>,
) -> Result<Json<ImportResponse>, ApiError> {
    validate_problem(&payload.problem)?;
    let id = import_problem(&state.pool, &payload.problem)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ImportResponse { id }))
}

/// GET /problems - list recent imported problems, newest first.
async fn list_problems_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<ProblemSummary>>, ApiError> {
    let limit = params.limit.unwrap_or(20);
    let rows = list_problems(&state.pool, limit)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|row| ProblemSummary {
                id: row.id,
                name: row.name,
                created_at: row.created_at,
                stores: row.stores,
                vehicles: row.vehicles,
                depots: row.depots,
            })
            .collect(),
    ))
}

/// GET /solutions - list recent solutions, newest first, optionally
/// filtered to one problem with `?problem_id=`.
async fn list_solutions_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<SolutionSummary>>, ApiError> {
    let limit = params.limit.unwrap_or(20);
    let rows = list_solutions(&state.pool, params.problem_id.as_deref(), limit)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|row| SolutionSummary {
                id: row.id,
                problem_id: row.problem_id,
                served: row.served,
                total: row.total,
                distance_km: row.distance_km,
                cost_cents: row.cost_cents,
                solved_at: row.solved_at,
                used_lns: row.used_lns,
            })
            .collect(),
    ))
}

/// GET /problems/{id} - retrieve a problem.
async fn get_problem_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Problem>, ApiError> {
    let problem = load_problem(&state.pool, &id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("problem {id} not found")))?;
    Ok(Json(problem))
}

/// POST /solve — solve a problem.
async fn solve_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SolveRequest>,
) -> Result<Json<SolveResponse>, ApiError> {
    let problem = load_problem(&state.pool, &payload.problem_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("problem {} not found", payload.problem_id)))?;

    validate_problem(&problem)?;
    check_positive_limit("maxweight", payload.maxweight)?;
    check_positive_limit("maxheight", payload.maxheight)?;
    let run_lns = payload.lns;
    let run_ls = payload.local_search;
    let strategy = if payload.ch {
        MatrixStrategy::ContractionHierarchy
    } else {
        MatrixStrategy::Dijkstra
    };
    let total = problem.stores.len();
    let stored_profile = StoredRoutingProfile {
        maxweight_tonnes: payload.maxweight,
        maxheight_m: payload.maxheight,
        hgv: payload.hgv,
    };

    let solution = if let Some(osm_path) = payload.osm_path.as_deref() {
        // M3: road-network distances/durations from OSM PBF. Load the graph
        // once per path and run the blocking solver work off the async worker.
        let cache = osm_cache_for(&state, osm_path).await?;
        let profile = TruckProfile {
            maxweight_tonnes: payload.maxweight,
            maxheight_m: payload.maxheight,
            hgv: payload.hgv,
        };
        tokio::task::spawn_blocking(move || -> anyhow::Result<Solution> {
            let mut cache = cache
                .lock()
                .map_err(|_| anyhow::anyhow!("cached OSM graph lock poisoned"))?;
            if run_lns {
                solver::solve_with_lns_routing_with(
                    &problem,
                    solver::LNSConfig::default(),
                    &mut cache,
                    profile,
                    strategy,
                )
            } else if run_ls {
                let entries = cache
                    .matrix_for_with(&problem, profile, strategy)?
                    .ok_or_else(|| anyhow::anyhow!("no OSM graph loaded"))?;
                let node_count = problem.depots.len() + problem.stores.len();
                let matrix = solver::DistanceMatrix::from_routing(
                    problem.depots.len(),
                    node_count,
                    &entries,
                );
                Ok(solver::solve_with_local_search(
                    &problem,
                    solver::LSConfig::default(),
                    matrix,
                ))
            } else {
                solver::solve_greedy_routing_with(&problem, &mut cache, profile, strategy)
            }
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?
    } else {
        tokio::task::spawn_blocking(move || {
            if run_lns {
                solver::solve_with_lns_haversine(&problem, solver::LNSConfig::default())
            } else if run_ls {
                solver::solve_with_local_search_haversine(&problem, solver::LSConfig::default())
            } else {
                solver::solve(&problem)
            }
        })
        .await
        .map_err(ApiError::internal)?
    };

    let served = served_store_count(&solution);

    let solution_id = store_solution_with_profile(
        &state.pool,
        &payload.problem_id,
        &solution,
        run_lns,
        payload.osm_path.as_deref(),
        stored_profile,
    )
    .await
    .map_err(ApiError::internal)?;

    // Broadcast solve event to SSE subscribers.
    let event = SolveEvent {
        solution_id: solution_id.clone(),
        problem_id: payload.problem_id.clone(),
        served,
        total,
        distance_km: solution.total_distance_km,
        cost_cents: solution.total_cost_cents,
    };
    state.events.send(event).ok();

    Ok(Json(SolveResponse {
        id: solution_id,
        served,
        total,
        distance_km: solution.total_distance_km,
        cost_cents: solution.total_cost_cents,
    }))
}

/// GET /solutions/{id} — retrieve a solution.
async fn get_solution_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SolutionResponse>, ApiError> {
    let (problem_id, solution) = load_solution_with_problem(&state.pool, &id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("solution {id} not found")))?;
    Ok(Json(SolutionResponse {
        id,
        problem_id,
        solution,
    }))
}

/// GET /solutions/{id}/geometry - road polylines for the solution's routes.
///
/// Requires the solution to have been solved against an OSM extract (its
/// `osm_path` was recorded). For each route, every consecutive pair of stops is
/// routed over the road network using the same [`TruckProfile`] stored with the
/// solve and tethered cleanly to the exact stop coordinates. Returns an empty
/// `routes` list when there is no recorded extract.
async fn get_solution_geometry_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SolutionGeometry>, ApiError> {
    let (problem_id, solution) = load_solution_with_problem(&state.pool, &id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("solution {id} not found")))?;

    let Some((osm_path, stored_profile)) = solution_routing_params(&state.pool, &id)
        .await
        .map_err(ApiError::internal)?
    else {
        return Ok(Json(SolutionGeometry {
            routes: Vec::new(),
            route_legs: Vec::new(),
        }));
    };

    let cache = osm_cache_for(&state, &osm_path).await?;
    let problem = load_problem(&state.pool, &problem_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("problem {problem_id} not found")))?;

    let profile = TruckProfile {
        maxweight_tonnes: stored_profile.maxweight_tonnes,
        maxheight_m: stored_profile.maxheight_m,
        hgv: stored_profile.hgv,
    };

    let geometry = tokio::task::spawn_blocking(move || -> anyhow::Result<SolutionGeometry> {
        let cache = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("cached OSM graph lock poisoned"))?;
        let graph = cache
            .graph()
            .ok_or_else(|| anyhow::anyhow!("no OSM graph loaded"))?;
        let node_ids = cache.problem_node_ids(&problem)?;
        Ok(compute_geometry(
            graph, &problem, &node_ids, &solution, profile,
        ))
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;

    Ok(Json(geometry))
}

fn node_lat_lon(problem: &Problem, node: domain::Node) -> Option<[f64; 2]> {
    match node {
        domain::Node::Depot(id) => problem.depot(id).map(|d| [d.location.lat, d.location.lon]),
        domain::Node::Store(id) => problem.store(id).map(|s| [s.location.lat, s.location.lon]),
    }
}

fn push_unique_point(out: &mut Vec<[f64; 2]>, pt: [f64; 2]) {
    let same = out
        .last()
        .is_some_and(|prev| (prev[0] - pt[0]).abs() < 1e-7 && (prev[1] - pt[1]).abs() < 1e-7);
    if !same {
        out.push(pt);
    }
}

/// Build road-following polylines for each route using `profile`.
///
/// Each leg tethers the origin and destination stop coordinates onto its routed
/// OSM road nodes and deduplicates shared seam vertices. When no routed path
/// exists between a pair of stops, that leg is recorded with `fallback = true`
/// so renderers can dash it distinctly instead of stitching unrelated segments.
fn compute_geometry(
    graph: &routing::Graph,
    problem: &domain::Problem,
    node_ids: &[routing::NodeId],
    solution: &domain::Solution,
    profile: TruckProfile,
) -> SolutionGeometry {
    let depot_count = problem.depots.len();
    let stop_index = |node: domain::Node| -> usize {
        match node {
            domain::Node::Depot(id) => id.0 as usize,
            domain::Node::Store(id) => depot_count + id.0 as usize,
        }
    };

    let mut routes = Vec::with_capacity(solution.routes.len());
    let mut route_legs = Vec::with_capacity(solution.routes.len());

    for route in &solution.routes {
        let mut flat_points: Vec<[f64; 2]> = Vec::new();
        let mut legs: Vec<RouteLegGeometry> =
            Vec::with_capacity(route.steps.len().saturating_sub(1));

        for pair in route.steps.windows(2) {
            let from_stop = node_lat_lon(problem, pair[0].node);
            let to_stop = node_lat_lon(problem, pair[1].node);
            let from_node = node_ids.get(stop_index(pair[0].node)).copied();
            let to_node = node_ids.get(stop_index(pair[1].node)).copied();

            let mut leg_pts: Vec<[f64; 2]> = Vec::new();
            let mut fallback = true;

            if let (Some(from_id), Some(to_id)) = (from_node, to_node)
                && let Some(coords) = routing::Router::path_geometry(graph, from_id, to_id, profile)
            {
                fallback = false;
                if let Some(p) = from_stop {
                    push_unique_point(&mut leg_pts, p);
                }
                for c in &coords {
                    push_unique_point(&mut leg_pts, [c.lat, c.lon]);
                }
                if let Some(p) = to_stop {
                    push_unique_point(&mut leg_pts, p);
                }
            } else {
                if let Some(p) = from_stop {
                    push_unique_point(&mut leg_pts, p);
                }
                if let Some(p) = to_stop {
                    push_unique_point(&mut leg_pts, p);
                }
            }

            for &pt in &leg_pts {
                push_unique_point(&mut flat_points, pt);
            }
            legs.push(RouteLegGeometry {
                points: leg_pts,
                fallback,
            });
        }

        routes.push(flat_points);
        route_legs.push(legs);
    }

    SolutionGeometry { routes, route_legs }
}

/// GET /solutions/{id}/kpis - KPI summary for a solution.
async fn get_kpis_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Kpis>, ApiError> {
    let (_problem_id, solution) = load_solution_with_problem(&state.pool, &id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("solution {id} not found")))?;

    let served = served_store_count(&solution);
    let routes = solution.routes.len();
    let total = served + solution.unassigned.len();
    let serve_rate = if total > 0 {
        served as f64 / total as f64
    } else {
        0.0
    };

    Ok(Json(Kpis {
        served,
        total,
        serve_rate,
        distance_km: solution.total_distance_km,
        cost_cents: solution.total_cost_cents,
        routes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
        LatLon, Load, Minutes, Node, PlannedStop, RoutePlan, Shift, Store, StoreId, TimeWindow,
        Vehicle, VehicleId,
    };
    use routing::{Graph, NodeId};

    #[test]
    fn compute_geometry_tethers_stops_dedupes_seams_and_honors_truck_profile() {
        let mut graph = Graph::new();
        // Snapped road nodes offset slightly from exact depot/store coordinates.
        let (n0, n1, n2) = (NodeId(0), NodeId(1), NodeId(2));
        assert!(graph.add_node(n0, LatLon::new(-33.8705, 151.2105)));
        assert!(graph.add_node(n1, LatLon::new(-33.8805, 151.2205)));
        assert!(graph.add_node(n2, LatLon::new(-33.8750, 151.2300)));
        // Direct edge between n0 and n1 carries a low bridge (3.5 m).
        graph.add_edge(
            n0,
            n1,
            1.0,
            2,
            None,
            None,
            Some(3.5),
            Some("yes".into()),
            None,
            true,
        );
        graph.add_edge(
            n1,
            n0,
            1.0,
            2,
            None,
            None,
            None,
            Some("yes".into()),
            None,
            true,
        );
        // Tall-safe detour n0 -> n2 -> n1.
        for (a, b) in [(n0, n2), (n2, n1)] {
            graph.add_edge(
                a,
                b,
                2.0,
                4,
                None,
                None,
                Some(5.0),
                Some("yes".into()),
                None,
                true,
            );
        }

        let problem = Problem {
            name: "geom".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "DC".into(),
                location: LatLon::new(-33.8700, 151.2100),
                address: None,
                timezone: None,
            }],
            stores: vec![Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "S0".into(),
                location: LatLon::new(-33.8800, 151.2200),
                address: None,
                timezone: None,
                demand: Load::new(1, 0, 0),
                service: Minutes::from_minutes(5),
                receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
            }],
            vehicles: vec![Vehicle {
                id: VehicleId(0),
                name: "V0".into(),
                depot: DepotId(0),
                capacity: Load::new(5, 0, 0),
                max_duration: Minutes::from_minutes(600),
                avg_speed_kmh: 60.0,
            }],
            drivers: vec![Driver {
                id: DriverId(0),
                name: "D0".into(),
                shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(600))],
                fatigue_rules: FatigueRules::default(),
            }],
            cost: CostModel {
                fixed_per_route_cents: 0,
                per_km_cents: 100,
                per_hour_cents: 0,
                late_per_min_cents: 0,
            },
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
                        arrive: Minutes::ZERO,
                        depart: Minutes::ZERO,
                    },
                    PlannedStop {
                        node: Node::Store(StoreId(0)),
                        arrive: Minutes::from_minutes(8),
                        depart: Minutes::from_minutes(13),
                    },
                    PlannedStop {
                        node: Node::Depot(DepotId(0)),
                        arrive: Minutes::from_minutes(15),
                        depart: Minutes::from_minutes(15),
                    },
                ],
                distance_km: 5.0,
                duration: Minutes::from_minutes(15),
                cost_cents: 500,
            }],
            unassigned: vec![],
            total_distance_km: 5.0,
            total_duration: Minutes::from_minutes(15),
            total_cost_cents: 500,
        };
        let node_ids: Vec<NodeId> = vec![n0, n1];
        let profile = TruckProfile {
            maxweight_tonnes: None,
            maxheight_m: Some(4.2),
            hgv: true,
        };

        let geom = compute_geometry(&graph, &problem, &node_ids, &solution, profile);
        assert_eq!(geom.routes.len(), 1);
        assert_eq!(geom.route_legs.len(), 1);
        assert_eq!(geom.route_legs[0].len(), 2);
        assert!(!geom.route_legs[0][0].fallback);
        assert!(!geom.route_legs[0][1].fallback);
        // Must take the tall-safe detour through n2 (-33.8750, 151.2300).
        assert!(geom.routes[0].contains(&[-33.8750, 151.2300]));
        // Stop tethers: starts and ends at the exact depot coordinate, and seam
        // between legs is deduplicated (no adjacent duplicates).
        assert_eq!(geom.routes[0].first(), Some(&[-33.8700, 151.2100]));
        assert_eq!(geom.routes[0].last(), Some(&[-33.8700, 151.2100]));
        assert!(
            geom.routes[0]
                .windows(2)
                .all(|w| (w[0][0] - w[1][0]).abs() > 1e-7 || (w[0][1] - w[1][1]).abs() > 1e-7)
        );
    }
}
