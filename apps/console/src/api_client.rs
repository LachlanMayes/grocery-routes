//! HTTP client for the grocery-routes API.
//!
//! Wraps reqwest to talk to the axum backend (M4 API).
//! All methods return [`anyhow::Result`] so the UI can display errors.

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
