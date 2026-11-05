//! Address search over an imported G-NAF database.
//!
//! [`parse_query`] is a deliberately simple heuristic: a leading number, a
//! street (with an optional trailing type such as `St`), a comma-separated
//! locality, and anywhere in the text a state code and a four-digit postcode.
//! Candidates are fetched with a `LIKE` scan and ranked in Rust; this is fine
//! for interactive use, not a general geocoder replacement.

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::{Address, haversine_km};

/// A parsed address query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeocodeQuery {
    pub number: Option<u32>,
    pub street: String,
    pub street_type: Option<String>,
    pub locality: Option<String>,
    pub state: Option<String>,
    pub postcode: Option<String>,
}

/// One geocoding result.
#[derive(Clone, Debug, PartialEq)]
pub struct AddressMatch {
    pub address: Address,
    pub score: i32,
}

const STATE_CODES: [&str; 8] = ["ACT", "NSW", "NT", "QLD", "SA", "TAS", "VIC", "WA"];

/// Parses a free-text Australian address into a [`GeocodeQuery`].
#[must_use]
pub fn parse_query(raw: &str) -> GeocodeQuery {
    let mut query = GeocodeQuery::default();
    let mut cleaned: Vec<Vec<String>> = Vec::new();
    for segment in raw.split(',') {
        let mut tokens = Vec::new();
        for token in segment.split_whitespace() {
            let upper = token.to_ascii_uppercase();
            if STATE_CODES.contains(&upper.as_str()) {
                query.state = Some(upper);
                continue;
            }
            if token.len() == 4 && token.chars().all(|c| c.is_ascii_digit()) {
                query.postcode = Some(token.to_string());
                continue;
            }
            tokens.push(token.to_string());
        }
        if !tokens.is_empty() {
            cleaned.push(tokens);
        }
    }

    if let Some(street_tokens) = cleaned.first() {
        let mut words: Vec<String> = street_tokens.clone();
        if let Some(first) = words.first()
            && let Ok(number) = first.parse::<u32>()
        {
            query.number = Some(number);
            words.remove(0);
        }
        // Find a street type anywhere after the first street word; words
        // after it are treated as the locality when no comma was used.
        let type_index = words
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, word)| canonical_street_type(&word.to_ascii_uppercase()).is_some())
            .map(|(index, _)| index);
        if let Some(index) = type_index {
            query.street_type = Some(words[index].to_ascii_uppercase());
            query.street = words[..index].join(" ");
            if cleaned.get(1).is_none() && index + 1 < words.len() {
                query.locality = Some(words[index + 1..].join(" "));
            }
        } else {
            query.street = words.join(" ");
        }
    }
    if let Some(locality_tokens) = cleaned.get(1) {
        query.locality = Some(locality_tokens.join(" "));
    }
    query
}

/// Searches the database, best match first.
///
/// Returns an empty list when the query has neither a street nor a locality.
pub fn geocode(conn: &Connection, query: &GeocodeQuery, limit: usize) -> Result<Vec<AddressMatch>> {
    if query.street.is_empty() && query.locality.is_none() {
        return Ok(Vec::new());
    }
    let mut sql = String::from(
        "SELECT pid, number_first, number_last, street, street_type, locality, state, \
         postcode, lat, lon FROM address WHERE 1 = 1",
    );
    let mut params: Vec<String> = Vec::new();
    let street_token = normalise(&query.street)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    if !street_token.is_empty() {
        sql.push_str(" AND UPPER(street) LIKE ?");
        params.push(format!("%{street_token}%"));
    }
    if let Some(locality) = &query.locality {
        sql.push_str(" AND UPPER(locality) LIKE ?");
        params.push(format!("%{}%", normalise(locality)));
    }
    if let Some(state) = &query.state {
        sql.push_str(" AND UPPER(state) = ?");
        params.push(state.to_ascii_uppercase());
    }
    if let Some(postcode) = &query.postcode {
        sql.push_str(" AND postcode = ?");
        params.push(postcode.clone());
    }
    sql.push_str(" LIMIT 2000");

    let mut stmt = conn.prepare(&sql).context("preparing geocode query")?;
    let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
    let mut matches: Vec<AddressMatch> = Vec::new();
    while let Some(row) = rows.next()? {
        let address = row_to_address(row)?;
        let score = rank(query, &address);
        matches.push(AddressMatch { address, score });
    }
    matches.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.address.locality.cmp(&b.address.locality))
            .then_with(|| a.address.street.cmp(&b.address.street))
            .then_with(|| a.address.number_first.cmp(&b.address.number_first))
            .then_with(|| a.address.pid.cmp(&b.address.pid))
    });
    matches.truncate(limit);
    Ok(matches)
}

/// Nearest addresses to a coordinate, closest first (metres).
pub fn reverse(conn: &Connection, lat: f64, lon: f64, limit: usize) -> Result<Vec<(Address, f64)>> {
    const DELTA_DEGREES: f64 = 0.02;
    let mut stmt = conn
        .prepare(
            "SELECT pid, number_first, number_last, street, street_type, locality, state, \
             postcode, lat, lon FROM address \
             WHERE lat BETWEEN ?1 AND ?2 AND lon BETWEEN ?3 AND ?4",
        )
        .context("preparing reverse query")?;
    let mut rows = stmt.query(rusqlite::params![
        lat - DELTA_DEGREES,
        lat + DELTA_DEGREES,
        lon - DELTA_DEGREES,
        lon + DELTA_DEGREES,
    ])?;
    let mut results: Vec<(Address, f64)> = Vec::new();
    while let Some(row) = rows.next()? {
        let address = row_to_address(row)?;
        let metres = haversine_km(lat, lon, address.lat, address.lon) * 1000.0;
        results.push((address, metres));
    }
    results.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.pid.cmp(&b.0.pid)));
    results.truncate(limit);
    Ok(results)
}

fn row_to_address(row: &rusqlite::Row<'_>) -> rusqlite::Result<Address> {
    Ok(Address {
        pid: row.get(0)?,
        number_first: row.get(1)?,
        number_last: row.get(2)?,
        street: row.get(3)?,
        street_type: row.get(4)?,
        locality: row.get(5)?,
        state: row.get(6)?,
        postcode: row.get(7)?,
        lat: row.get(8)?,
        lon: row.get(9)?,
    })
}

/// Ranking heuristic: exact street and number matches dominate.
fn rank(query: &GeocodeQuery, address: &Address) -> i32 {
    let mut score = 0;
    let wanted_street = normalise(&query.street);
    let found_street = normalise(&address.street);
    if !wanted_street.is_empty() {
        if found_street == wanted_street {
            score += 200;
        } else if found_street.starts_with(&wanted_street) {
            score += 120;
        } else {
            score += 60;
        }
    }
    if let Some(wanted_type) = &query.street_type {
        let found = address.street_type.as_deref().unwrap_or_default();
        if types_equivalent(wanted_type, found) {
            score += 40;
        } else {
            score -= 20;
        }
    }
    if let Some(locality) = &query.locality {
        let wanted = normalise(locality);
        let found = normalise(&address.locality);
        if found == wanted {
            score += 120;
        } else if found.contains(&wanted) {
            score += 60;
        }
    }
    if let Some(state) = &query.state
        && !address.state.eq_ignore_ascii_case(state)
    {
        score -= 40;
    }
    if let Some(postcode) = &query.postcode
        && address.postcode.as_deref() != Some(postcode.as_str())
    {
        score -= 40;
    }
    if let Some(number) = query.number {
        if address.number_matches(number) {
            score += 150;
        } else {
            score -= 60;
        }
    }
    score
}

/// Upper-cased, punctuation-stripped, whitespace-collapsed text.
pub(crate) fn normalise(value: &str) -> String {
    value
        .to_ascii_uppercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Canonical form of a street type (`ST` -> `STREET`, ...).
#[must_use]
pub fn canonical_street_type(value: &str) -> Option<&'static str> {
    const TYPES: &[(&str, &str)] = &[
        ("STREET", "STREET"),
        ("ST", "STREET"),
        ("ROAD", "ROAD"),
        ("RD", "ROAD"),
        ("AVENUE", "AVENUE"),
        ("AVE", "AVENUE"),
        ("AV", "AVENUE"),
        ("DRIVE", "DRIVE"),
        ("DR", "DRIVE"),
        ("COURT", "COURT"),
        ("CT", "COURT"),
        ("CRT", "COURT"),
        ("PLACE", "PLACE"),
        ("PL", "PLACE"),
        ("CRESCENT", "CRESCENT"),
        ("CRES", "CRESCENT"),
        ("PARADE", "PARADE"),
        ("PDE", "PARADE"),
        ("HIGHWAY", "HIGHWAY"),
        ("HWY", "HIGHWAY"),
        ("LANE", "LANE"),
        ("LN", "LANE"),
        ("CLOSE", "CLOSE"),
        ("CL", "CLOSE"),
        ("TERRACE", "TERRACE"),
        ("TCE", "TERRACE"),
        ("BOULEVARD", "BOULEVARD"),
        ("BLVD", "BOULEVARD"),
        ("CIRCUIT", "CIRCUIT"),
        ("CCT", "CIRCUIT"),
        ("GROVE", "GROVE"),
        ("GR", "GROVE"),
        ("ESPLANADE", "ESPLANADE"),
        ("ESP", "ESPLANADE"),
        ("WAY", "WAY"),
        ("WALK", "WALK"),
        ("RISE", "RISE"),
        ("MEWS", "MEWS"),
        ("SQUARE", "SQUARE"),
        ("SQ", "SQUARE"),
        ("LOOP", "LOOP"),
        ("BEND", "BEND"),
        ("GARDENS", "GARDENS"),
        ("GDNS", "GARDENS"),
        ("TRACK", "TRACK"),
        ("TRK", "TRACK"),
        ("VIEW", "VIEW"),
        ("VISTA", "VISTA"),
    ];
    let upper = value.trim().to_ascii_uppercase();
    TYPES
        .iter()
        .find(|(alias, _)| *alias == upper)
        .map(|(_, canonical)| *canonical)
}

/// True when two street types mean the same thing.
#[must_use]
pub fn types_equivalent(a: &str, b: &str) -> bool {
    match (canonical_street_type(a), canonical_street_type(b)) {
        (Some(a), Some(b)) => a == b,
        _ => normalise(a) == normalise(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comma_form() {
        let query = parse_query("12 Smith St, Sydney NSW 2000");
        assert_eq!(query.number, Some(12));
        assert_eq!(query.street, "Smith");
        assert_eq!(query.street_type.as_deref(), Some("ST"));
        assert_eq!(query.locality.as_deref(), Some("Sydney"));
        assert_eq!(query.state.as_deref(), Some("NSW"));
        assert_eq!(query.postcode.as_deref(), Some("2000"));
    }

    #[test]
    fn parses_no_comma_form() {
        let query = parse_query("14-16 King Street Newtown");
        // The leading range is not a plain number; the first token is kept.
        assert_eq!(query.number, None);
        assert_eq!(query.street, "14-16 King");
        assert_eq!(query.street_type.as_deref(), Some("STREET"));
        assert_eq!(query.locality.as_deref(), Some("Newtown"));
    }

    #[test]
    fn canonical_types_match_aliases() {
        assert_eq!(canonical_street_type("st"), Some("STREET"));
        assert_eq!(canonical_street_type("Rd"), Some("ROAD"));
        assert_eq!(canonical_street_type("Zebra"), None);
        assert!(types_equivalent("St", "STREET"));
        assert!(!types_equivalent("St", "Road"));
    }
}
