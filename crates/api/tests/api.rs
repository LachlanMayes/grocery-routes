//! HTTP API integration tests (M4).
//!
//! These tests boot the real axum router on a random local port and drive it
//! with reqwest. They cover the full import -> solve -> read flow, validation
//! of malformed imports, 404 handling, and the SSE solve event.

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use domain::{
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
};
use reqwest::{StatusCode, header};
use serde_json::json;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tower::ServiceExt;

fn test_problem() -> Problem {
    Problem {
        name: "api integration fixture".into(),
        depots: vec![Depot {
            id: DepotId(0),
            name: "Depot".into(),
            location: LatLon::new(-33.87, 151.21),
            timezone: None,
        }],
        stores: vec![
            Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "Coles A".into(),
                location: LatLon::new(-33.90, 151.20),
                demand: Load::new(1, 0, 0),
                service: Minutes::from_minutes(10),
                receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
                timezone: None,
            },
            Store {
                id: StoreId(1),
                chain: Chain::Woolworths,
                name: "Woolworths B".into(),
                location: LatLon::new(-33.85, 151.25),
                demand: Load::new(2, 1, 0),
                service: Minutes::from_minutes(10),
                receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
                timezone: None,
            },
        ],
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
            per_km_cents: 100,
            per_hour_cents: 0,
            late_per_min_cents: 0,
        },
        chain_rules: ChainRules::default(),
        curfews: CurfewRules::default(),
        horizon_start: None,
    }
}

async fn spawn_app() -> (String, JoinHandle<()>, tempfile::TempPath) {
    let db_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
    let pool = api::db::open_db(&db_path).await.unwrap();
    let router = api::router(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .expect("test server should run");
    });

    (format!("http://{address}"), handle, db_path)
}

async fn import_problem(
    client: &reqwest::Client,
    base_url: &str,
    problem: &Problem,
) -> reqwest::Response {
    client
        .post(format!("{base_url}/problems"))
        .json(&json!({ "problem": problem }))
        .send()
        .await
        .expect("import request should complete")
}

#[tokio::test]
async fn import_solve_read_flow() {
    let (base_url, handle, _db_path) = spawn_app().await;
    let client = reqwest::Client::new();
    let problem = test_problem();

    let import = import_problem(&client, &base_url, &problem).await;
    assert_eq!(import.status(), StatusCode::OK);
    let body: serde_json::Value = import.json().await.unwrap();
    let problem_id = body["id"].as_str().unwrap().to_owned();

    let solve = client
        .post(format!("{base_url}/solve"))
        .json(&json!({
            "problem_id": problem_id,
            "lns": false,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(solve.status(), StatusCode::OK);
    let solve_body: serde_json::Value = solve.json().await.unwrap();
    let solution_id = solve_body["id"].as_str().unwrap().to_owned();
    assert_eq!(solve_body["served"], 2);

    let solution = client
        .get(format!("{base_url}/solutions/{solution_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(solution.status(), StatusCode::OK);
    let solution_body: serde_json::Value = solution.json().await.unwrap();
    assert_eq!(solution_body["problem_id"], problem_id);

    let kpis = client
        .get(format!("{base_url}/solutions/{solution_id}/kpis"))
        .send()
        .await
        .unwrap();
    assert_eq!(kpis.status(), StatusCode::OK);
    let kpis_body: serde_json::Value = kpis.json().await.unwrap();
    assert_eq!(kpis_body["served"], 2);
    assert_eq!(kpis_body["total"], 2);
    assert!(kpis_body["serve_rate"].as_f64().unwrap() > 0.99);

    handle.abort();
}

#[tokio::test]
async fn invalid_import_returns_bad_request() {
    let (base_url, handle, _db_path) = spawn_app().await;
    let client = reqwest::Client::new();
    let mut problem = test_problem();
    problem.stores[0].id = StoreId(1); // duplicate with store 1: non-contiguous ids

    let response = import_problem(&client, &base_url, &problem).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    handle.abort();
}

#[tokio::test]
async fn unknown_ids_return_not_found() {
    let (base_url, handle, _db_path) = spawn_app().await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{base_url}/problems/does-not-exist"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = client
        .post(format!("{base_url}/solve"))
        .json(&json!({ "problem_id": "does-not-exist" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = client
        .get(format!("{base_url}/solutions/does-not-exist"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = client
        .get(format!("{base_url}/solutions/does-not-exist/kpis"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    handle.abort();
}

#[tokio::test]
async fn post_solve_with_missing_problem_returns_not_found() {
    let (base_url, handle, _db_path) = spawn_app().await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{base_url}/solve"))
        .json(&json!({ "problem_id": "missing" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    handle.abort();
}

#[tokio::test]
async fn events_stream_broadcasts_solve_event() {
    let (base_url, handle, _db_path) = spawn_app().await;
    let client = reqwest::Client::new();
    let problem = test_problem();

    // Subscribe before importing and solving so the broadcast is received.
    let sse = client
        .get(format!("{base_url}/events"))
        .header(header::ACCEPT, "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(sse.status(), StatusCode::OK);
    let mut stream = sse.bytes_stream();
    let mut buffer = String::new();

    let import = import_problem(&client, &base_url, &problem).await;
    assert_eq!(import.status(), StatusCode::OK);
    let problem_id = import.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let solve = client
        .post(format!("{base_url}/solve"))
        .json(&json!({ "problem_id": problem_id, "lns": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(solve.status(), StatusCode::OK);

    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let chunk = stream
                .next()
                .await
                .expect("SSE stream closed early")
                .unwrap();
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            if let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim_end_matches('\r').to_owned();
                if line.starts_with("data: ") {
                    return line;
                }
                buffer.drain(..=pos);
            }
        }
    })
    .await
    .expect("timed out waiting for SSE solve event");

    assert!(event.contains(&problem_id));

    handle.abort();
}

#[tokio::test]
async fn oversized_request_body_returns_payload_too_large() {
    let db_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
    let pool = api::db::open_db(&db_path).await.unwrap();
    let router = api::router(pool);
    let body = vec![b' '; 17 * 1024 * 1024];
    let request = Request::builder()
        .method("POST")
        .uri("/problems")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
