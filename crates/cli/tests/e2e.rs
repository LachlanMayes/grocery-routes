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
