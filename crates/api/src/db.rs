//! SQLite persistence for problems and solutions (M4 product shell).
//!
//! Stores imported problems and solved plans so they can be retrieved,
//! re-solved, and exposed through the axum HTTP API.

use anyhow::{Context, Result};
use domain::{Problem, Solution};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use std::path::Path;

/// Open (or create) the SQLite database at `path` and run migrations.
pub async fn open_db(path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(options).await?;
    run_migrations(&pool).await?;
    Ok(pool)
}

/// Create the `problems` and `solutions` tables if they don't exist.
async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS problems (
            id         TEXT PRIMARY KEY,
            name       TEXT NOT NULL,
            json       TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS solutions (
            id           TEXT PRIMARY KEY,
            problem_id   TEXT NOT NULL REFERENCES problems(id),
            json         TEXT NOT NULL,
            served       INTEGER NOT NULL,
            total        INTEGER NOT NULL,
            distance_km  REAL NOT NULL,
            cost_cents   INTEGER NOT NULL,
            solved_at    TEXT NOT NULL DEFAULT (datetime('now')),
            used_lns     INTEGER NOT NULL DEFAULT 0
        )
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Import a problem into the database, returning its assigned id.
pub async fn import_problem(pool: &SqlitePool, problem: &Problem) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let json = serde_json::to_string(problem).context("serializing problem")?;
    sqlx::query("INSERT INTO problems (id, name, json) VALUES (?1, ?2, ?3)")
        .bind(&id)
        .bind(&problem.name)
        .bind(&json)
        .execute(pool)
        .await
        .context("inserting problem")?;
    Ok(id)
}

/// Load a problem from the database by id.
pub async fn load_problem(pool: &SqlitePool, id: &str) -> Result<Option<Problem>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT json FROM problems WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row
        .map(|(json,)| serde_json::from_str(&json).context("deserializing problem"))
        .transpose()?)
}

/// Store a solved solution in the database, returning its assigned id.
pub async fn store_solution(
    pool: &SqlitePool,
    problem_id: &str,
    solution: &Solution,
    used_lns: bool,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let json = serde_json::to_string(solution).context("serializing solution")?;
    let served = solution
        .routes
        .iter()
        .flat_map(|r| &r.steps)
        .filter(|s| matches!(s.node, domain::Node::Store(_)))
        .count() as i64;
    let total = problem_store_count(pool, problem_id).await?;
    sqlx::query(
        "INSERT INTO solutions (id, problem_id, json, served, total, distance_km, cost_cents, used_lns) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&id)
    .bind(problem_id)
    .bind(&json)
    .bind(served)
    .bind(total)
    .bind(solution.total_distance_km)
    .bind(solution.total_cost_cents)
    .bind(used_lns as i64)
    .execute(pool)
    .await
    .context("inserting solution")?;
    Ok(id)
}

/// Load a solution from the database by id, returning (problem_id, solution).
pub async fn load_solution_with_problem(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<(String, Solution)>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT problem_id, json FROM solutions WHERE id = ?1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row
        .map(
            |(problem_id, json): (String, String)| -> Result<(String, Solution), anyhow::Error> {
                let solution = serde_json::from_str(&json).context("deserializing solution")?;
                Ok((problem_id, solution))
            },
        )
        .transpose()?)
}

/// Return the number of stores in the referenced problem.
async fn problem_store_count(pool: &SqlitePool, problem_id: &str) -> Result<i64> {
    let row: Option<(String,)> = sqlx::query_as("SELECT json FROM problems WHERE id = ?1")
        .bind(problem_id)
        .fetch_optional(pool)
        .await?;
    match row {
        Some((json,)) => {
            let problem: Problem = serde_json::from_str(&json).context("deserializing problem")?;
            Ok(problem.stores.len() as i64)
        }
        None => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        Chain, CostModel, Depot, DepotId, Driver, DriverId, FatigueRules, LatLon, Load, Minutes,
        Node, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
    };
    use tempfile;

    fn test_problem() -> Problem {
        Problem {
            name: "test".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Depot".into(),
                location: LatLon::new(-33.87, 151.21),
            }],
            stores: vec![Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "Coles Test".into(),
                location: LatLon::new(-33.92, 151.23),
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
        }
    }

    #[tokio::test]
    async fn import_and_load_problem() {
        let db_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let pool = open_db(&db_path).await.unwrap();
        let problem = test_problem();
        let id = import_problem(&pool, &problem).await.unwrap();
        let loaded = load_problem(&pool, &id).await.unwrap().unwrap();
        assert_eq!(loaded.name, problem.name);
        assert_eq!(loaded.stores.len(), problem.stores.len());
        drop(pool);
    }

    #[tokio::test]
    async fn store_and_load_solution() {
        let db_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let pool = open_db(&db_path).await.unwrap();
        let problem = test_problem();
        let problem_id = import_problem(&pool, &problem).await.unwrap();

        let solution = solver::solve(&problem);
        let solution_id = store_solution(&pool, &problem_id, &solution, false)
            .await
            .unwrap();

        let (_problem_id, loaded) = load_solution_with_problem(&pool, &solution_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.routes.len(), solution.routes.len());
        drop(pool);
    }
}
