//! End-to-end ingest + geocoding tests over the synthetic G-NAF fixture in
//! `tests/fixtures/`.

use geo_au::{geocode, import_directory, parse_query, reverse};
use rusqlite::Connection;

/// Imports the fixture once per test into a fresh database.
fn fixture_db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("gnaf.sqlite");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let stats = import_directory(&fixture, &db_path).expect("import fixture");
    assert_eq!(stats.addresses, 6);
    assert_eq!(stats.geocodes, 6);
    assert_eq!(stats.states, 2);
    assert_eq!(stats.localities, 3);
    assert_eq!(stats.street_localities, 4);
    let conn = Connection::open(&db_path).expect("open imported db");
    (dir, conn)
}

fn search(conn: &Connection, text: &str, limit: usize) -> Vec<geo_au::AddressMatch> {
    let query = parse_query(text);
    geocode(conn, &query, limit).expect("geocode")
}

#[test]
fn exact_number_street_and_suburb_wins() {
    let (_dir, conn) = fixture_db();
    let results = search(&conn, "12 Smith St, Sydney NSW 2000", 5);
    assert!(!results.is_empty());
    let best = &results[0];
    assert_eq!(best.address.pid, "1000");
    assert_eq!(best.address.locality, "Sydney");
    assert_eq!(best.address.state, "NSW");
    assert!((best.address.lat - -33.8688).abs() < 1e-6);
    assert!((best.address.lon - 151.2093).abs() < 1e-6);
    assert_eq!(best.address.display(), "12 Smith STREET, Sydney NSW 2000");
}

#[test]
fn street_only_query_returns_every_address_on_the_street_best_first() {
    let (_dir, conn) = fixture_db();
    let results = search(&conn, "Smith Street", 10);
    let pids: Vec<&str> = results
        .iter()
        .map(|result| result.address.pid.as_str())
        .collect();
    assert!(pids.contains(&"1000"));
    assert!(pids.contains(&"1001"));
    assert!(pids.contains(&"1005"));
    assert_eq!(results.len(), 3, "all Smith Street addresses: {pids:?}");
}

#[test]
fn number_ranges_match_their_span() {
    let (_dir, conn) = fixture_db();
    let results = search(&conn, "15 Smith Street", 10);
    let pids: Vec<&str> = results
        .iter()
        .map(|result| result.address.pid.as_str())
        .collect();
    assert!(
        pids.contains(&"1001"),
        "14-16 should match number 15: {pids:?}"
    );
}

#[test]
fn state_and_postcode_filters_exclude_other_states() {
    let (_dir, conn) = fixture_db();
    // Bourke Street exists only in Melbourne (VIC 3000).
    let victorian = search(&conn, "Bourke Street, Melbourne VIC 3000", 5);
    assert_eq!(victorian.len(), 1);
    assert_eq!(victorian[0].address.state, "VIC");

    // A contradictory state filter ranks the row far down or removes it.
    let wrong_state = search(&conn, "Bourke Street, NSW", 5);
    assert!(
        wrong_state.is_empty() || wrong_state[0].address.state == "NSW",
        "state filter must not return a VIC address as the best match"
    );
}

#[test]
fn reverse_finds_the_nearest_address() {
    let (_dir, conn) = fixture_db();
    let results = reverse(&conn, -33.8688, 151.2093, 3).expect("reverse");
    assert!(!results.is_empty());
    assert_eq!(results[0].0.pid, "1000");
    assert!(results[0].1 < 5.0, "distance metres: {}", results[0].1);
    for pair in results.windows(2) {
        assert!(pair[0].1 <= pair[1].1, "results must be distance-sorted");
    }
    // A coordinate far from every fixture address finds nothing.
    let empty = reverse(&conn, -20.0, 130.0, 3).expect("reverse empty");
    assert!(empty.is_empty());
}

#[test]
fn unknown_streets_return_no_matches() {
    let (_dir, conn) = fixture_db();
    assert!(search(&conn, "99 Zebra Lane, Sydney", 5).is_empty());
}

#[test]
fn empty_query_returns_nothing() {
    let (_dir, conn) = fixture_db();
    assert!(search(&conn, "   ", 5).is_empty());
}
