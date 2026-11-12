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
            used_lns     INTEGER NOT NULL DEFAULT 0,
            osm_path     TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Add osm_path to databases created before it existed (SQLite has no
    // "ADD COLUMN IF NOT EXISTS", so a duplicate-column error is ignored).
    let _ = sqlx::query("ALTER TABLE solutions ADD COLUMN osm_path TEXT")
        .execute(pool)
        .await;

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

/// Summary of a stored problem, for the list endpoint.
///
/// Counts are parsed from the stored JSON on read: the problems table has no
/// count columns. Only used for recent-import dropdowns, so the cost of
/// deserializing the handful of JSON blobs it reads is fine.
#[derive(Clone, Debug)]
pub struct ProblemSummaryRow {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub stores: usize,
    pub vehicles: usize,
    pub depots: usize,
}

/// Summary of a stored solution, for the list endpoint.
#[derive(Clone, Debug)]
pub struct SolutionSummaryRow {
    pub id: String,
    pub problem_id: String,
    pub served: i64,
    pub total: i64,
    pub distance_km: f64,
    pub cost_cents: i64,
    pub solved_at: String,
    pub used_lns: bool,
}

/// List the most recently imported problems, newest first.
///
/// `limit` is clamped to 1..=100.
pub async fn list_problems(pool: &SqlitePool, limit: u32) -> Result<Vec<ProblemSummaryRow>> {
    let limit = limit.clamp(1, 100) as i64;
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, name, created_at, json FROM problems ORDER BY created_at DESC, rowid DESC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, name, created_at, json) in rows {
        let problem: Problem = serde_json::from_str(&json).context("deserializing problem")?;
        out.push(ProblemSummaryRow {
            id,
            name,
            created_at,
            stores: problem.stores.len(),
            vehicles: problem.vehicles.len(),
            depots: problem.depots.len(),
        });
    }
    Ok(out)
}

/// List the most recently solved solutions, newest first, optionally
/// filtered to one problem. `limit` is clamped to 1..=100.
pub async fn list_solutions(
    pool: &SqlitePool,
    problem_id: Option<&str>,
    limit: u32,
) -> Result<Vec<SolutionSummaryRow>> {
    let limit = limit.clamp(1, 100) as i64;
    type SolutionTuple = (String, String, i64, i64, f64, i64, String, i64);
    let rows: Vec<SolutionTuple> = match problem_id {
        Some(pid) => sqlx::query_as(
            "SELECT id, problem_id, served, total, distance_km, cost_cents, solved_at, used_lns \
                 FROM solutions WHERE problem_id = ?2 ORDER BY solved_at DESC, rowid DESC LIMIT ?1",
        )
        .bind(limit)
        .bind(pid)
        .fetch_all(pool)
        .await?,
        None => sqlx::query_as(
            "SELECT id, problem_id, served, total, distance_km, cost_cents, solved_at, used_lns \
                 FROM solutions ORDER BY solved_at DESC, rowid DESC LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?,
    };
    Ok(rows
        .into_iter()
        .map(
            |(id, problem_id, served, total, distance_km, cost_cents, solved_at, used_lns)| {
                SolutionSummaryRow {
                    id,
                    problem_id,
                    served,
                    total,
                    distance_km,
                    cost_cents,
                    solved_at,
                    used_lns: used_lns != 0,
                }
            },
        )
        .collect())
}

/// Load a problem from the database by id.
pub async fn load_problem(pool: &SqlitePool, id: &str) -> Result<Option<Problem>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT json FROM problems WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|(json,)| serde_json::from_str(&json).context("deserializing problem"))
        .transpose()
}

/// Store a solved solution in the database, returning its assigned id.
pub async fn store_solution(
    pool: &SqlitePool,
    problem_id: &str,
    solution: &Solution,
    used_lns: bool,
    osm_path: Option<&str>,
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
        "INSERT INTO solutions (id, problem_id, json, served, total, distance_km, cost_cents, used_lns, osm_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&id)
    .bind(problem_id)
    .bind(&json)
    .bind(served)
    .bind(total)
    .bind(solution.total_distance_km)
    .bind(solution.total_cost_cents)
    .bind(used_lns as i64)
    .bind(osm_path)
    .execute(pool)
    .await
    .context("inserting solution")?;
    Ok(id)
}

/// The OSM extract a solution was solved against, if any. Used to redraw its
/// route geometry.
pub async fn solution_osm_path(pool: &SqlitePool, id: &str) -> Result<Option<String>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT osm_path FROM solutions WHERE id = ?1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(path,)| path))
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
    row.map(
        |(problem_id, json): (String, String)| -> Result<(String, Solution), anyhow::Error> {
            let solution = serde_json::from_str(&json).context("deserializing solution")?;
            Ok((problem_id, solution))
        },
    )
    .transpose()
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
        Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
        LatLon, Load, Minutes, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
    };
    use tempfile;

    fn test_problem() -> Problem {
        Problem {
            name: "test".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Depot".into(),
                location: LatLon::new(-33.87, 151.21),
                timezone: None,
            }],
            stores: vec![Store {
                id: StoreId(0),
                chain: Chain::Coles,
                name: "Coles Test".into(),
                location: LatLon::new(-33.92, 151.23),
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
    async fn list_problems_returns_newest_first_and_counts() {
        let db_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let pool = open_db(&db_path).await.unwrap();
        let problem = test_problem();

        import_problem(&pool, &problem).await.unwrap();
        import_problem(
            &pool,
            &Problem {
                name: "second".into(),
                ..problem
            },
        )
        .await
        .unwrap();

        let rows = list_problems(&pool, 10).await.unwrap();
        assert_eq!(rows.len(), 2);
        // Newest first.
        assert_eq!(rows[0].name, "second");
        assert_eq!(rows[1].name, "test");
        // Counts are parsed from the problem JSON.
        assert_eq!(rows[0].stores, 1);
        assert_eq!(rows[0].vehicles, 1);
        assert_eq!(rows[0].depots, 1);
        // Limit is applied.
        let one = list_problems(&pool, 1).await.unwrap();
        assert_eq!(one.len(), 1);
        drop(pool);
    }

    // Test fixture mirrors the crate's other fixture-heavy tests (the
    // code-quality skill accepts test-fixture duplication); marked explicitly
    // rather than restructured, since the literals ARE the fixture data.
    // jscpd:ignore-start
    #[tokio::test]
    async fn list_solutions_returns_newest_first_and_filters_by_problem() {
        let db_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let pool = open_db(&db_path).await.unwrap();
        let problem = test_problem();

        let problem_id = import_problem(&pool, &problem).await.unwrap();
        let solution = solver::solve(&problem);
        let sol_id1 = store_solution(&pool, &problem_id, &solution, false, None)
            .await
            .unwrap();

        let other = Problem {
            name: "other".into(),
            ..problem
        };
        let other_id = import_problem(&pool, &other).await.unwrap();
        let sol_id2 = store_solution(&pool, &other_id, &solution, true, None)
            .await
            .unwrap();

        let all = list_solutions(&pool, None, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|r| r.id == sol_id1));
        assert!(all.iter().any(|r| r.id == sol_id2));
        let lns: Vec<_> = all.iter().filter(|r| r.used_lns).collect();
        assert_eq!(lns.len(), 1);

        let filtered = list_solutions(&pool, Some(&other_id), 10).await.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, sol_id2);
        assert_eq!(filtered[0].served, 1);
        assert_eq!(filtered[0].total, 1);
        drop(pool);
    }
    // jscpd:ignore-end

    #[tokio::test]
    async fn store_and_load_solution() {
        let db_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let pool = open_db(&db_path).await.unwrap();
        let problem = test_problem();
        let problem_id = import_problem(&pool, &problem).await.unwrap();

        let solution = solver::solve(&problem);
        let solution_id = store_solution(&pool, &problem_id, &solution, false, None)
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
