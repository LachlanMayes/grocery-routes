//! HTTP client for the grocery-routes API.
//!
//! Wraps reqwest to talk to the axum backend (M4 API).
//! All methods return [`anyhow::Result`] so the UI can display errors.

// Mirrors the API surface: some endpoints and payload fields are not yet
// consumed by the console UI, so dead-code warnings are expected here.
#![allow(dead_code)]

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;

/// Base URL for the API. Defaults to localhost:8080.
const DEFAULT_BASE_URL: &str = "http://localhost:8080";

/// Import request body: a serialized domain problem.
#[derive(Clone, Debug, Serialize)]
pub struct ImportProblem {
    pub problem: serde_json::Value,
}

/// Import response.
#[derive(Clone, Debug, Deserialize)]
pub struct ImportResponse {
    pub id: String,
}

/// Solve request body.
#[derive(Clone, Debug, Serialize)]
pub struct SolveRequest {
    pub problem_id: String,
    #[serde(default)]
    pub lns: bool,
    pub osm_path: Option<String>,
    pub maxweight: Option<f64>,
    pub maxheight: Option<f64>,
    #[serde(default)]
    pub hgv: bool,
}

/// Solve response.
#[derive(Clone, Debug, Deserialize)]
pub struct SolveResponse {
    pub id: String,
    pub served: usize,
    pub total: usize,
    pub distance_km: f64,
    pub cost_cents: i64,
}

/// Solution response.
#[derive(Clone, Debug, Deserialize)]
pub struct SolutionResponse {
    pub id: String,
    pub problem_id: String,
    pub solution: serde_json::Value,
}

/// KPI summary.
#[derive(Clone, Debug, Deserialize)]
pub struct Kpis {
    pub served: usize,
    pub total: usize,
    pub serve_rate: f64,
    pub distance_km: f64,
    pub cost_cents: i64,
    pub routes: usize,
}

/// SSE solve event broadcast by the server.
#[derive(Clone, Debug, Deserialize)]
pub struct SolveEvent {
    pub solution_id: String,
    pub problem_id: String,
    pub served: usize,
    pub total: usize,
    pub distance_km: f64,
    pub cost_cents: i64,
}

/// HTTP client for the grocery-routes API.
#[derive(Clone)]
pub struct ApiClient {
    inner: Client,
    base_url: String,
}

impl ApiClient {
    /// Create a new client pointing at `base_url`.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            inner: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            base_url: base_url.into(),
        }
    }

    /// Default client pointing at localhost:8080.
    pub fn default_local() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }

    /// Returns the base URL of the API.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// POST /problems — import a problem JSON.
    pub async fn import_problem(&self, problem: &serde_json::Value) -> Result<ImportResponse> {
        let body = ImportProblem {
            problem: problem.clone(),
        };
        let resp = self
            .inner
            .post(format!("{}/problems", self.base_url))
            .json(&body)
            .send()
            .await
            .context("POST /problems")?;
        if !resp.status().is_success() {
            anyhow::bail!("POST /problems failed: {}", resp.status());
        }
        let json: ImportResponse = resp.json().await.context("parsing import response")?;
        Ok(json)
    }

    /// GET /problems/{id} — retrieve a problem.
    pub async fn get_problem(&self, id: &str) -> Result<serde_json::Value> {
        let resp = self
            .inner
            .get(format!("{}/problems/{}", self.base_url, id))
            .send()
            .await
            .context("GET /problems/{id}")?;
        if !resp.status().is_success() {
            anyhow::bail!("GET /problems/{id} failed: {}", resp.status());
        }
        let json: serde_json::Value = resp.json().await.context("parsing problem response")?;
        Ok(json)
    }

    /// POST /solve — solve a problem.
    pub async fn solve(&self, req: &SolveRequest) -> Result<SolveResponse> {
        let resp = self
            .inner
            .post(format!("{}/solve", self.base_url))
            .json(req)
            .send()
            .await
            .context("POST /solve")?;
        if !resp.status().is_success() {
            anyhow::bail!("POST /solve failed: {}", resp.status());
        }
        let json: SolveResponse = resp.json().await.context("parsing solve response")?;
        Ok(json)
    }

    /// GET /solutions/{id} — retrieve a solution.
    pub async fn get_solution(&self, id: &str) -> Result<SolutionResponse> {
        let resp = self
            .inner
            .get(format!("{}/solutions/{}", self.base_url, id))
            .send()
            .await
            .context("GET /solutions/{id}")?;
        if !resp.status().is_success() {
            anyhow::bail!("GET /solutions/{id} failed: {}", resp.status());
        }
        let json: SolutionResponse = resp.json().await.context("parsing solution response")?;
        Ok(json)
    }

    /// GET /solutions/{id}/kpis — KPI summary.
    pub async fn get_kpis(&self, id: &str) -> Result<Kpis> {
        let resp = self
            .inner
            .get(format!("{}/solutions/{}/kpis", self.base_url, id))
            .send()
            .await
            .context("GET /solutions/{id}/kpis")?;
        if !resp.status().is_success() {
            anyhow::bail!("GET /solutions/{id}/kpis failed: {}", resp.status());
        }
        let json: Kpis = resp.json().await.context("parsing kpis response")?;
        Ok(json)
    }

    /// Open an SSE event stream. Returns a stream of [`SolveEvent`]s.
    ///
    /// The caller should drop the returned stream when done.
    pub async fn events_stream(&self) -> Result<reqwest::Response> {
        let resp = self
            .inner
            .get(format!("{}/events", self.base_url))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .context("GET /events (SSE)")?;
        if !resp.status().is_success() {
            anyhow::bail!("GET /events failed: {}", resp.status());
        }
        Ok(resp)
    }
}

/// Helper to deserialize a JSON value into any `DeserializeOwned` type.
pub fn from_json<T: DeserializeOwned>(v: &serde_json::Value) -> Result<T> {
    serde_json::from_value(v.clone()).context("deserializing JSON value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::Path,
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post},
    };
    use serde_json::json;
    use tokio::task::JoinHandle;

    async fn spawn_stub() -> (String, JoinHandle<()>) {
        async fn import_problem(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
            let fail = body
                .get("problem")
                .and_then(|problem| problem.get("fail"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if fail {
                return (StatusCode::INTERNAL_SERVER_ERROR, "bad import").into_response();
            }
            Json(json!({"id": "problem-123"})).into_response()
        }

        async fn get_problem(Path(id): Path<String>) -> impl IntoResponse {
            if id == "bad" {
                return (StatusCode::INTERNAL_SERVER_ERROR, "bad problem").into_response();
            }
            Json(json!({"id": id, "name": "stub problem"})).into_response()
        }

        async fn solve(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
            if body["problem_id"] == "fail" {
                return (StatusCode::INTERNAL_SERVER_ERROR, "bad solve").into_response();
            }
            Json(json!({
                "id": "solution-123",
                "served": 2,
                "total": 2,
                "distance_km": 12.5,
                "cost_cents": 1000,
            }))
            .into_response()
        }

        async fn get_solution(Path(id): Path<String>) -> impl IntoResponse {
            if id == "bad" {
                return (StatusCode::INTERNAL_SERVER_ERROR, "bad solution").into_response();
            }
            Json(json!({
                "id": id,
                "problem_id": "problem-123",
                "solution": { "hello": "world" },
            }))
            .into_response()
        }

        async fn get_kpis(Path(id): Path<String>) -> impl IntoResponse {
            if id == "bad" {
                return (StatusCode::INTERNAL_SERVER_ERROR, "bad kpis").into_response();
            }
            Json(json!({
                "served": 2,
                "total": 2,
                "serve_rate": 1.0,
                "distance_km": 12.5,
                "cost_cents": 1000,
                "routes": 1,
            }))
            .into_response()
        }

        async fn events() -> &'static str {
            "data: {\"solution_id\":\"s1\",\"problem_id\":\"p1\",\"served\":2,\"total\":2,\"distance_km\":12.5,\"cost_cents\":1000}\n\n"
        }

        let router = Router::new()
            .route("/problems", post(import_problem))
            .route("/problems/{id}", get(get_problem))
            .route("/solve", post(solve))
            .route("/solutions/{id}", get(get_solution))
            .route("/solutions/{id}/kpis", get(get_kpis))
            .route("/events", get(events));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service())
                .await
                .expect("stub server should run");
        });

        (format!("http://{address}"), handle)
    }

    #[tokio::test]
    async fn client_imports_problem() {
        let (base_url, server) = spawn_stub().await;
        let client = ApiClient::new(base_url);

        let response = client
            .import_problem(&json!({ "problem": {} }))
            .await
            .unwrap();
        assert_eq!(response.id, "problem-123");

        server.abort();
    }

    #[tokio::test]
    async fn client_import_errors_on_server_failure() {
        let (base_url, server) = spawn_stub().await;
        let client = ApiClient::new(base_url);

        let error = client
            .import_problem(&json!({ "fail": true }))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("500"));

        server.abort();
    }

    #[tokio::test]
    async fn client_gets_problem() {
        let (base_url, server) = spawn_stub().await;
        let client = ApiClient::new(base_url);

        let problem = client.get_problem("stub").await.unwrap();
        assert_eq!(problem["name"], "stub problem");

        let error = client.get_problem("bad").await.unwrap_err();
        assert!(error.to_string().contains("500"));

        server.abort();
    }

    #[tokio::test]
    async fn client_solves_problem() {
        let (base_url, server) = spawn_stub().await;
        let client = ApiClient::new(base_url);
        let request = SolveRequest {
            problem_id: "problem-123".into(),
            lns: false,
            osm_path: None,
            maxweight: None,
            maxheight: None,
            hgv: false,
        };

        let response = client.solve(&request).await.unwrap();
        assert_eq!(response.id, "solution-123");

        let failing_request = SolveRequest {
            problem_id: "fail".into(),
            ..request.clone()
        };
        let error = client.solve(&failing_request).await.unwrap_err();
        assert!(error.to_string().contains("500"));

        server.abort();
    }

    #[tokio::test]
    async fn client_gets_solution() {
        let (base_url, server) = spawn_stub().await;
        let client = ApiClient::new(base_url);

        let response = client.get_solution("solution-123").await.unwrap();
        assert_eq!(response.problem_id, "problem-123");

        let error = client.get_solution("bad").await.unwrap_err();
        assert!(error.to_string().contains("500"));

        server.abort();
    }

    #[tokio::test]
    async fn client_gets_kpis() {
        let (base_url, server) = spawn_stub().await;
        let client = ApiClient::new(base_url);

        let response = client.get_kpis("solution-123").await.unwrap();
        assert_eq!(response.served, 2);
        assert_eq!(response.routes, 1);

        let error = client.get_kpis("bad").await.unwrap_err();
        assert!(error.to_string().contains("500"));

        server.abort();
    }

    #[tokio::test]
    async fn client_opens_events_stream() {
        let (base_url, server) = spawn_stub().await;
        let client = ApiClient::new(base_url);

        let response = client.events_stream().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        server.abort();
    }

    #[test]
    fn test_from_json() {
        let value: serde_json::Value = from_json(&json!({ "hello": "world" })).unwrap();
        assert_eq!(value["hello"], "world");
    }
}
