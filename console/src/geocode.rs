//! Address and coordinate lookup helpers for the console route builder.
//!
//! Supports direct `lat, lon` entry offline, optional local Australian G-NAF
//! SQLite databases via [`geo_au`], and OpenStreetMap Nominatim search when a
//! free-text address is not found locally.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Which builder field group requested an address lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GeocodeTarget {
    Depot,
    Stop,
}

/// One resolved address / location candidate returned to the UI.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GeocodeMatch {
    pub label: String,
    pub lat: f64,
    pub lon: f64,
    pub source: String,
}

/// Completed address lookup response posted back to the UI thread.
#[derive(Clone, Debug)]
pub(crate) struct GeocodeResponse {
    pub target: GeocodeTarget,
    pub query: String,
    pub matches: Vec<GeocodeMatch>,
}

/// Parses `"lat, lon"`, `"lat lon"`, or `"(lat, lon)"` coordinate pairs when valid.
#[must_use]
pub(crate) fn parse_lat_lon_pair(raw: &str) -> Option<(f64, f64)> {
    let trimmed = raw
        .trim()
        .trim_start_matches(['(', '['])
        .trim_end_matches([')', ']']);
    let parts: Vec<&str> = if trimmed.contains(',') {
        trimmed.split(',').map(str::trim).collect()
    } else {
        trimmed.split_whitespace().collect()
    };
    if parts.len() != 2 {
        return None;
    }
    let lat: f64 = parts[0].parse().ok()?;
    let lon: f64 = parts[1].parse().ok()?;
    if !lat.is_finite() || !lon.is_finite() {
        return None;
    }
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    Some((lat, lon))
}

/// Looks up up to `limit` matches in a local imported G-NAF SQLite database.
pub(crate) fn lookup_gnaf(
    db_path: &Path,
    query_str: &str,
    limit: usize,
) -> Result<Vec<GeocodeMatch>> {
    let conn = geo_au::open_database(db_path)?;
    let parsed = geo_au::parse_query(query_str);
    let hits = geo_au::geocode(&conn, &parsed, limit)?;
    Ok(hits
        .into_iter()
        .map(|hit| GeocodeMatch {
            label: hit.address.display(),
            lat: hit.address.lat,
            lon: hit.address.lon,
            source: "G-NAF".to_string(),
        })
        .collect())
}

#[derive(Deserialize)]
struct NominatimItem {
    lat: String,
    lon: String,
    display_name: String,
}

/// Parses the JSON array returned by the OpenStreetMap Nominatim `jsonv2` endpoint.
pub(crate) fn parse_nominatim_json(body: &str) -> Result<Vec<GeocodeMatch>> {
    let items: Vec<NominatimItem> =
        serde_json::from_str(body).context("parsing OpenStreetMap Nominatim response")?;
    let mut matches = Vec::with_capacity(items.len());
    for item in items {
        let Ok(lat) = item.lat.parse::<f64>() else {
            continue;
        };
        let Ok(lon) = item.lon.parse::<f64>() else {
            continue;
        };
        if !lat.is_finite()
            || !lon.is_finite()
            || !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
        {
            continue;
        }
        matches.push(GeocodeMatch {
            label: item.display_name,
            lat,
            lon,
            source: "OpenStreetMap".to_string(),
        });
    }
    Ok(matches)
}

/// Resolves an address or `"lat, lon"` string using coordinates -> local G-NAF DB -> Nominatim.
pub(crate) async fn search_address(
    client: reqwest::Client,
    gnaf_db_path: Option<PathBuf>,
    target: GeocodeTarget,
    raw_query: String,
) -> Result<GeocodeResponse> {
    let query = raw_query.trim().to_string();
    if query.is_empty() {
        bail!("enter an address or \"lat, lon\" coordinates first");
    }

    if let Some((lat, lon)) = parse_lat_lon_pair(&query) {
        return Ok(GeocodeResponse {
            target,
            query,
            matches: vec![GeocodeMatch {
                label: format!("{lat:.5}, {lon:.5}"),
                lat,
                lon,
                source: "Coordinates".to_string(),
            }],
        });
    }

    let mut gnaf_error: Option<anyhow::Error> = None;
    if let Some(db_path) = gnaf_db_path.filter(|p| !p.as_os_str().is_empty())
        && db_path.exists()
    {
        let query_copy = query.clone();
        let result = tokio::task::spawn_blocking(move || lookup_gnaf(&db_path, &query_copy, 5))
            .await
            .context("joining G-NAF lookup task")?;
        match result {
            Ok(hits) if !hits.is_empty() => {
                return Ok(GeocodeResponse {
                    target,
                    query,
                    matches: hits,
                });
            }
            Ok(_) => {}
            Err(err) => {
                gnaf_error = Some(err);
            }
        }
    }

    let response = client
        .get("https://nominatim.openstreetmap.org/search")
        .header("User-Agent", "grocery-routes-console/0.1")
        .timeout(Duration::from_secs(8))
        .query(&[("format", "jsonv2"), ("limit", "5"), ("q", query.as_str())])
        .send()
        .await;

    match response {
        Ok(resp) => {
            if !resp.status().is_success() {
                let status = resp.status();
                let detail = resp.text().await.unwrap_or_default();
                bail!("Nominatim geocoder returned {status}: {detail}");
            }
            let body = resp
                .text()
                .await
                .context("reading Nominatim response body")?;
            let matches = parse_nominatim_json(&body)?;
            Ok(GeocodeResponse {
                target,
                query,
                matches,
            })
        }
        Err(err) => {
            if let Some(gnaf_err) = gnaf_error {
                bail!("G-NAF lookup failed ({gnaf_err}) and OpenStreetMap lookup failed ({err})");
            }
            Err(anyhow::anyhow!(
                "OpenStreetMap address lookup failed ({err}); enter \"lat, lon\" or configure a local G-NAF database"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    #[test]
    fn parse_lat_lon_pair_handles_formats_and_bounds() {
        assert_eq!(
            parse_lat_lon_pair("-33.8688, 151.2093"),
            Some((-33.8688, 151.2093))
        );
        assert_eq!(
            parse_lat_lon_pair("(-37.8136,144.9631)"),
            Some((-37.8136, 144.9631))
        );
        assert_eq!(
            parse_lat_lon_pair("-27.4698 153.0251"),
            Some((-27.4698, 153.0251))
        );
        assert_eq!(parse_lat_lon_pair("91.0, 151.0"), None);
        assert_eq!(parse_lat_lon_pair("-33.8, 181.0"), None);
        assert_eq!(parse_lat_lon_pair("100 George St, Sydney NSW"), None);
    }

    #[test]
    fn parse_nominatim_json_extracts_valid_hits() {
        let raw = r#"[
            {
                "lat": "-33.8688197",
                "lon": "151.2092955",
                "display_name": "Sydney, NSW, Australia"
            },
            {
                "lat": "bad",
                "lon": "151.2",
                "display_name": "Broken"
            }
        ]"#;
        let hits = parse_nominatim_json(raw).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "Sydney, NSW, Australia");
        assert_eq!(hits[0].source, "OpenStreetMap");
        assert!((hits[0].lat - -33.8688197).abs() < 1e-6);
        assert!((hits[0].lon - 151.2092955).abs() < 1e-6);
    }

    #[test]
    fn lookup_gnaf_queries_sqlite_fixture() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE address(
                 pid TEXT PRIMARY KEY,
                 number_first INTEGER,
                 number_last INTEGER,
                 street TEXT NOT NULL,
                 street_type TEXT,
                 locality TEXT NOT NULL,
                 state TEXT NOT NULL,
                 postcode TEXT,
                 lat REAL NOT NULL,
                 lon REAL NOT NULL
             );
             INSERT INTO address VALUES ('PID1', 100, NULL, 'GEORGE', 'STREET', 'SYDNEY', 'NSW', '2000', -33.8650, 151.2080);",
        )
        .unwrap();
        drop(conn);

        let hits = lookup_gnaf(tmp.path(), "100 George St, Sydney NSW 2000", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "G-NAF");
        assert!((hits[0].lat - -33.8650).abs() < 1e-6);
        assert!((hits[0].lon - 151.2080).abs() < 1e-6);
    }
}
