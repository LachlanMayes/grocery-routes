//! End-to-end CLI test: `gen-sim` -> `solve` -> `export`.
//!
//! This test exercises the `sim` and `solver` crates through the binary. While
//! either of them is still `unimplemented!()`, the corresponding step exits
//! with a panic and this test is expected to be red — the CLI wiring itself is
//! covered by the other tests until those crates land.

use std::fs;
use std::path::Path;
use std::process::Command;

use domain::{Problem, Solution};

/// The freshly built binary, as located by Cargo.
const BIN: &str = env!("CARGO_BIN_EXE_grocery-routes");

/// Runs `command`, asserting a successful exit status.
fn expect_success(mut command: Command) -> std::process::Output {
    let output = command.output().expect("spawning grocery-routes");
    assert!(
        output.status.success(),
        "grocery-routes failed with {}:\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

/// Reads a file as UTF-8 text, panicking with the path on failure.
fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

#[test]
fn gen_sim_solve_and_export_round_trip() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let problem_path = dir.path().join("problem.json");
    let solution_path = dir.path().join("solution.json");
    let out_dir = dir.path().join("report");

    // gen-sim: 8 stores, 2 depots, 3 vehicles, seed 7.
    let mut gen_sim = Command::new(BIN);
    gen_sim.args([
        "gen-sim",
        "--chain",
        "coles",
        "--city",
        "sydney",
        "--stores",
        "8",
        "--depots",
        "2",
        "--vehicles",
        "3",
        "--seed",
        "7",
    ]);
    gen_sim.arg("--out").arg(&problem_path);
    expect_success(gen_sim);
    assert!(
        problem_path.exists(),
        "gen-sim did not write {}",
        problem_path.display()
    );
    let problem: Problem =
        serde_json::from_str(&read_text(&problem_path)).expect("problem JSON parses");
    assert_eq!(problem.stores.len(), 8);
    assert_eq!(problem.depots.len(), 2);
    assert_eq!(problem.vehicles.len(), 3);

    // solve: writes a plan that must pass validation.
    let mut solve = Command::new(BIN);
    solve
        .arg("solve")
        .arg("--problem")
        .arg(&problem_path)
        .arg("--out")
        .arg(&solution_path);
    expect_success(solve);
    assert!(
        solution_path.exists(),
        "solve did not write {}",
        solution_path.display()
    );
    let solution: Solution =
        serde_json::from_str(&read_text(&solution_path)).expect("solution JSON parses");
    assert_eq!(solution.validate(&problem), Ok(()));

    // export: both report files appear, the GeoJSON parses.
    let mut export = Command::new(BIN);
    export
        .arg("export")
        .arg("--problem")
        .arg(&problem_path)
        .arg("--solution")
        .arg(&solution_path)
        .arg("--out-dir")
        .arg(&out_dir);
    expect_success(export);
    let sheet_path = out_dir.join("route-sheet.html");
    let geojson_path = out_dir.join("routes.geojson");
    assert!(sheet_path.exists(), "missing {}", sheet_path.display());
    assert!(geojson_path.exists(), "missing {}", geojson_path.display());
    let document: serde_json::Value =
        serde_json::from_str(&read_text(&geojson_path)).expect("GeoJSON parses");
    assert_eq!(document["type"], "FeatureCollection");
    let features = document["features"].as_array().expect("features array");
    assert_eq!(features.len(), solution.routes.len() + problem.stores.len());
}

#[test]
fn enforce_curfews_fails_gracefully_without_osm() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let problem_path = dir.path().join("problem.json");
    let solution_path = dir.path().join("solution.json");

    let mut gen_sim = Command::new(BIN);
    gen_sim.args([
        "gen-sim",
        "--stores",
        "4",
        "--depots",
        "1",
        "--vehicles",
        "2",
        "--seed",
        "5",
        "--timezones",
        "--horizon-start",
        "2026-11-05T00:00:00Z",
    ]);
    gen_sim.arg("--out").arg(&problem_path);
    expect_success(gen_sim);

    // Inject a curfew rule (only the CLI path is under test here; the router
    // itself is covered by unit tests).
    let mut value: serde_json::Value =
        serde_json::from_str(&read_text(&problem_path)).expect("problem JSON parses");
    value["curfews"] = serde_json::json!({
        "rules": [{
            "road_class": "residential",
            "start": 1320,
            "end": 360,
            "days_of_week": null
        }]
    });
    fs::write(
        &problem_path,
        serde_json::to_string_pretty(&value).expect("re-serialise problem"),
    )
    .expect("write problem with curfews");

    let output = Command::new(BIN)
        .arg("solve")
        .arg("--problem")
        .arg(&problem_path)
        .arg("--out")
        .arg(&solution_path)
        .arg("--enforce-curfews")
        .output()
        .expect("spawning grocery-routes");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "solve must fail without --osm when enforcing curfews"
    );
    assert!(
        stderr.contains("requires --osm"),
        "stderr should explain the missing extract, got: {stderr}"
    );
    assert!(
        !solution_path.exists(),
        "no solution should be written when enforcement cannot run"
    );
}

#[test]
fn solve_rejects_invalid_timezone_without_panicking() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let problem_path = dir.path().join("problem.json");
    let solution_path = dir.path().join("solution.json");

    let mut gen_sim = Command::new(BIN);
    gen_sim.args([
        "gen-sim",
        "--stores",
        "4",
        "--depots",
        "1",
        "--vehicles",
        "2",
        "--seed",
        "3",
        "--timezones",
    ]);
    gen_sim.arg("--out").arg(&problem_path);
    expect_success(gen_sim);

    // Corrupt every stamped timezone.
    let text = read_text(&problem_path).replace("Australia/Sydney", "Australia/Sydnee");
    fs::write(&problem_path, text).expect("write corrupted problem");

    let output = Command::new(BIN)
        .arg("solve")
        .arg("--problem")
        .arg(&problem_path)
        .arg("--out")
        .arg(&solution_path)
        .output()
        .expect("spawning grocery-routes");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "solve should fail for an invalid timezone"
    );
    assert!(
        !stderr.contains("panicked"),
        "invalid input must fail gracefully, stderr: {stderr}"
    );
    assert!(
        stderr.contains("invalid IANA timezone"),
        "stderr should name the problem, got: {stderr}"
    );
    assert!(
        !solution_path.exists(),
        "no solution should be written for invalid input"
    );
}

#[test]
fn anchored_pipeline_renders_local_times_and_map_links() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let problem_path = dir.path().join("problem.json");
    let solution_path = dir.path().join("solution.json");
    let out_dir = dir.path().join("report");

    let mut gen_sim = Command::new(BIN);
    gen_sim.args([
        "gen-sim",
        "--stores",
        "6",
        "--depots",
        "1",
        "--vehicles",
        "2",
        "--seed",
        "11",
        "--timezones",
        "--horizon-start",
        "2026-11-05T00:00:00Z",
    ]);
    gen_sim.arg("--out").arg(&problem_path);
    expect_success(gen_sim);

    let problem: Problem =
        serde_json::from_str(&read_text(&problem_path)).expect("problem JSON parses");
    assert!(problem.horizon_start.is_some());
    assert!(problem.stores.iter().all(|store| store.timezone.is_some()));

    let mut solve = Command::new(BIN);
    solve
        .arg("solve")
        .arg("--problem")
        .arg(&problem_path)
        .arg("--out")
        .arg(&solution_path);
    expect_success(solve);

    let mut export = Command::new(BIN);
    export
        .arg("export")
        .arg("--problem")
        .arg(&problem_path)
        .arg("--solution")
        .arg(&solution_path)
        .arg("--out-dir")
        .arg(&out_dir);
    expect_success(export);

    let sheet = read_text(&out_dir.join("route-sheet.html"));
    assert!(
        sheet.contains("Local (arrive)"),
        "anchored route sheet should render local times"
    );
    assert!(
        sheet.contains("https://www.openstreetmap.org/?mlat="),
        "route sheet should deep-link nodes to OpenStreetMap"
    );
}
