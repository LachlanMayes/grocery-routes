//! HTTP request handlers for the M4 product shell API.
//!
//! Endpoints:
//! - POST /problems — import a problem JSON
//! - GET /problems/{id} — retrieve a problem
//! - POST /solve — solve a problem (greedy or LNS, optional OSM routing)
//! - GET /solutions/{id} — retrieve a solution
//! - GET /solutions/{id}/kpis — KPI summary for a solution
//! - GET /events — SSE stream of live solve events

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Json, sse::Event, sse::Sse},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt, wrappers::BroadcastStream};

use crate::db::{import_problem, load_problem, load_solution_with_problem, store_solution};
use domain::{Problem, Solution};
use routing::{CachedMatrix, TruckProfile};

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

/// Shared application state: the SQLite connection pool plus an SSE broadcast
/// channel for live solve events.
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub events: broadcast::Sender<SolveEvent>,
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

/// Build the axum router with all API routes.
pub fn router(pool: sqlx::SqlitePool) -> Router {
    let (events, _rx) = broadcast::channel(16);
    let state = Arc::new(AppState { pool, events });
    Router::new()
        .route("/problems", post(import_problem_handler))
        .route("/problems/:id", get(get_problem_handler))
        .route("/solve", post(solve_handler))
        .route("/solutions/:id", get(get_solution_handler))
        .route("/solutions/:id/kpis", get(get_kpis_handler))
        .route("/events", get(sse_events_handler))
        .with_state(state)
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
) -> Result<Json<ImportResponse>, StatusCode> {
    let id = import_problem(&state.pool, &payload.problem)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(ImportResponse { id }))
}

/// GET /problems/{id} — retrieve a problem.
async fn get_problem_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Problem>, StatusCode> {
    let problem = load_problem(&state.pool, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(problem))
}

/// POST /solve — solve a problem.
async fn solve_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SolveRequest>,
) -> Result<Json<SolveResponse>, StatusCode> {
    let problem = load_problem(&state.pool, &payload.problem_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let solution = if let Some(osm_path) = payload.osm_path {
        // M3: road-network distances/durations from OSM PBF.
        let mut cached = CachedMatrix::new();
        cached
            .load_pbf(std::path::Path::new(&osm_path))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let profile = TruckProfile {
            maxweight_tonnes: payload.maxweight,
            maxheight_m: payload.maxheight,
            hgv: payload.hgv,
        };
        if payload.lns {
            solver::solve_with_lns_routing(
                &problem,
                solver::LNSConfig::default(),
                &mut cached,
                profile,
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        } else {
            solver::solve_greedy_routing(&problem, &mut cached, profile)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        }
    } else if payload.lns {
        solver::solve_with_lns_haversine(&problem, solver::LNSConfig::default())
    } else {
        solver::solve(&problem)
    };

    let served = solution
        .routes
        .iter()
        .flat_map(|r| &r.steps)
        .filter(|s| matches!(s.node, domain::Node::Store(_)))
        .count();
    let total = problem.stores.len();

    let solution_id = store_solution(&state.pool, &payload.problem_id, &solution, payload.lns)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Broadcast solve event to SSE subscribers.
    let event = SolveEvent {
        solution_id: solution_id.clone(),
        problem_id: payload.problem_id.clone(),
        served,
        total,
        distance_km: solution.total_distance_km,
        cost_cents: solution.total_cost_cents,
    };
    let _ = state.events.send(event);

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
) -> Result<Json<SolutionResponse>, StatusCode> {
    let (problem_id, solution) = load_solution_with_problem(&state.pool, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(SolutionResponse {
        id,
        problem_id,
        solution,
    }))
}

/// GET /solutions/{id}/kpis — KPI summary for a solution.
async fn get_kpis_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Kpis>, StatusCode> {
    let (_problem_id, solution) = load_solution_with_problem(&state.pool, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let served = solution
        .routes
        .iter()
        .flat_map(|r| &r.steps)
        .filter(|s| matches!(s.node, domain::Node::Store(_)))
        .count();
    let routes = solution.routes.len();
    let total = solution
        .routes
        .iter()
        .flat_map(|r| &r.steps)
        .filter(|s| matches!(s.node, domain::Node::Store(_)))
        .count()
        + solution.unassigned.len();
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
