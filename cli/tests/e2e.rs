//! End-to-end CLI integration tests via the compiled `grocery-routes` binary.
//!
//! Covers `gen-sim` -> `solve` -> `export` round trips, anchored local-time and
//! public-holiday rendering, curfew error handling, curated truck constraints
//! SQLite export, and `import-gnaf` -> `geocode` -> `reverse`.

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
    let returnables_path = out_dir.join("returnables.csv");
    assert!(sheet_path.exists(), "missing {}", sheet_path.display());
    assert!(geojson_path.exists(), "missing {}", geojson_path.display());
    assert!(
        returnables_path.exists(),
        "missing {}",
        returnables_path.display()
    );
    let document: serde_json::Value =
        serde_json::from_str(&read_text(&geojson_path)).expect("GeoJSON parses");
    assert_eq!(document["type"], "FeatureCollection");
    let features = document["features"].as_array().expect("features array");
    assert_eq!(features.len(), solution.routes.len() + problem.stores.len());
    let returnables = read_text(&returnables_path);
    assert!(returnables.starts_with("store_id,store,chain,visits,"));
    assert!(
        returnables.lines().count() >= 2,
        "sim problems collect returnables, so at least one store row is expected"
    );
}

#[test]
fn gnaf_import_geocode_and_reverse_round_trip() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db = dir.path().join("gnaf.sqlite");
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../geo-au/tests/fixtures");

    let mut import = Command::new(BIN);
    import
        .arg("import-gnaf")
        .arg("--dir")
        .arg(&fixtures)
        .arg("--db")
        .arg(&db);
    let output = expect_success(import);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("imported 6 address(es)"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let mut geocode = Command::new(BIN);
    geocode
        .arg("geocode")
        .arg("--db")
        .arg(&db)
        .arg("12 Smith St, Sydney NSW 2000");
    let output = expect_success(geocode);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pid 1000"), "stdout: {stdout}");
    assert!(stdout.contains("NSW 2000"), "stdout: {stdout}");

    let mut reverse = Command::new(BIN);
    reverse
        .arg("reverse")
        .arg("--db")
        .arg(&db)
        .args(["--lat=-33.8688", "--lon=151.2093"]);
    let output = expect_success(reverse);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pid 1000"), "stdout: {stdout}");
}

#[test]
fn constraints_validate_and_export_sqlite() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let file = dir.path().join("truck-constraints.json");
    fs::write(
        &file,
        r#"{
            "entries": [
                {
                    "way_id": 999999001,
                    "maxweight_tonnes": 12.5,
                    "maxheight_m": 4.3,
                    "note": "Bridge limit (sample)",
                    "source": "TfNSW sample",
                    "verified_by": "test",
                    "verified_at": "2026-10-01"
                }
            ]
        }"#,
    )
    .expect("write constraints");
    let sqlite = dir.path().join("constraints.sqlite");

    let mut cmd = Command::new(BIN);
    cmd.arg("constraints")
        .arg("--file")
        .arg(&file)
        .arg("--sqlite")
        .arg(&sqlite);
    let output = expect_success(cmd);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("1 entries"), "stdout: {stdout}");
    assert!(stdout.contains("wrote 1"), "stdout: {stdout}");
    assert!(sqlite.exists(), "sqlite file should exist");
    // Header for user database is always present.
    let bytes = fs::read(&sqlite).expect("read sqlite");
    assert!(bytes.starts_with(b"SQLite format 3"), "not a sqlite file");
}

#[test]
fn holidays_subcommand_prints_national_and_state_calendars() {
    let output = Command::new(BIN)
        .args(["holidays", "--year", "2026", "--state", "vic"])
        .output()
        .expect("spawning grocery-routes");
    assert!(
        output.status.success(),
        "holidays failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Victoria public holidays 2026"));
    assert!(stdout.contains("2026-11-03  Melbourne Cup"));
    assert!(stdout.contains("2026-09-25  Friday before the AFL Grand Final"));

    let output = Command::new(BIN)
        .args(["holidays", "--year", "2026"])
        .output()
        .expect("spawning grocery-routes");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("National public holidays 2026"));
    assert!(stdout.contains("2026-04-06  Easter Monday"));

    let output = Command::new(BIN)
        .args(["holidays", "--year", "2026", "--state", "xx"])
        .output()
        .expect("spawning grocery-routes");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown state code"),
        "stderr should name the bad code, got: {stderr}"
    );
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
