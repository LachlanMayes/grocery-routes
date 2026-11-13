//! Address search over an imported G-NAF database.
//!
//! [`parse_query`] is a deliberately simple heuristic: a leading number, a
//! street (with an optional trailing type such as `St`), a comma-separated
//! locality, and anywhere in the text a state code and a four-digit postcode.
//! Candidates are fetched with a `LIKE` scan and ranked in Rust; this is fine
//! for interactive use, not a general geocoder replacement.

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::Address;

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
    let mut segments: Vec<Vec<String>> = Vec::new();
    for segment in raw.split(',') {
        let mut tokens = Vec::new();
        for token in segment.split_whitespace() {
            if !classify_token(token, &mut query) {
                tokens.push(token.to_string());
            }
        }
        if !tokens.is_empty() {
            segments.push(tokens);
        }
    }

    if let Some(words) = segments.first() {
        split_street_segment(&mut query, words, segments.len() > 1);
    }
    if let Some(locality_tokens) = segments.get(1) {
        query.locality = Some(locality_tokens.join(" "));
    }
    query
}

/// True when `token` is a state code or a four-digit postcode, which are
/// folded into `query` rather than kept as street or locality words.
fn classify_token(token: &str, query: &mut GeocodeQuery) -> bool {
    let upper = token.to_ascii_uppercase();
    if STATE_CODES.contains(&upper.as_str()) {
        query.state = Some(upper);
        return true;
    }
    if token.len() == 4 && token.chars().all(|c| c.is_ascii_digit()) {
        query.postcode = Some(token.to_string());
        return true;
    }
    false
}

/// Splits the first segment into number, street, optional street type and
/// (when no comma-separated locality followed) trailing locality words.
fn split_street_segment(query: &mut GeocodeQuery, words: &[String], has_locality_segment: bool) {
    let mut words = words.to_vec();
    if let Some(first) = words.first()
        && let Ok(number) = first.parse::<u32>()
    {
        query.number = Some(number);
        words.remove(0);
    }
    // Find a street type anywhere after the first street word.
    let type_index = words
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, word)| canonical_street_type(&word.to_ascii_uppercase()).is_some())
        .map(|(index, _)| index);
    let Some(index) = type_index else {
        query.street = words.join(" ");
        return;
    };
    query.street_type = Some(words[index].to_ascii_uppercase());
    query.street = words[..index].join(" ");
    if !has_locality_segment && index + 1 < words.len() {
        query.locality = Some(words[index + 1..].join(" "));
    }
}

/// Searches the database, best match first.
///
/// Runs an indexed prefix query first (`UPPER(street)` / `UPPER(locality)`
/// bounds plus exact state/postcode indexes) and falls back to an escaped
/// `LIKE ... ESCAPE '\'` scan when no prefix match is found. Each phase caps at
/// 2,000 candidates before ranking in Rust, so very broad queries without
/// state, postcode, or locality filters can still hit the cap.
///
/// Returns an empty list when neither a searchable street token nor a valid
/// locality token is present.
pub fn geocode(conn: &Connection, query: &GeocodeQuery, limit: usize) -> Result<Vec<AddressMatch>> {
    let street_token = normalise(&query.street)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let norm_locality = match query.locality.as_deref() {
        Some(raw) => {
            let norm = normalise(raw);
            if norm.is_empty() {
                return Ok(Vec::new());
            }
            Some(norm)
        }
        None => None,
    };
    if street_token.is_empty() && norm_locality.is_none() {
        return Ok(Vec::new());
    }

    let mut matches = fetch_candidates(conn, query, &street_token, norm_locality.as_deref(), true)?;
    if matches.is_empty() {
        matches = fetch_candidates(conn, query, &street_token, norm_locality.as_deref(), false)?;
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

/// Escapes `\`, `%` and `_` for `LIKE ? ESCAPE '\'`.
fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Exclusive upper bound for B-tree prefix range scans `UPPER(col) >= prefix AND UPPER(col) < bound`.
fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let mut chars: Vec<char> = prefix.chars().collect();
    while let Some(last) = chars.pop() {
        if let Some(next) = char::from_u32(u32::from(last) + 1) {
            chars.push(next);
            return Some(chars.into_iter().collect());
        }
    }
    None
}

/// Fetches and scores up to 2,000 candidates using either indexed prefix bounds
/// or escaped infix `LIKE` patterns.
fn fetch_candidates(
    conn: &Connection,
    query: &GeocodeQuery,
    street_token: &str,
    norm_locality: Option<&str>,
    prefix_only: bool,
) -> Result<Vec<AddressMatch>> {
    let mut sql = String::from(
        "SELECT pid, number_first, number_last, street, street_type, locality, state, \
         postcode, lat, lon FROM address WHERE 1 = 1",
    );
    let mut params: Vec<String> = Vec::new();
    let push_filter = |sql: &mut String, params: &mut Vec<String>, column: &str, value: &str| {
        if prefix_only {
            if let Some(upper) = prefix_upper_bound(value) {
                sql.push_str(&format!(
                    " AND UPPER({column}) >= ? AND UPPER({column}) < ?"
                ));
                params.push(value.to_string());
                params.push(upper);
            } else {
                sql.push_str(&format!(" AND UPPER({column}) = ?"));
                params.push(value.to_string());
            }
        } else {
            sql.push_str(&format!(" AND UPPER({column}) LIKE ? ESCAPE '\\'"));
            params.push(format!("%{}%", escape_like(value)));
        }
    };
    if !street_token.is_empty() {
        push_filter(&mut sql, &mut params, "street", street_token);
    }
    if let Some(locality) = norm_locality {
        push_filter(&mut sql, &mut params, "locality", locality);
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
    Ok(matches)
}

/// Nearest addresses to a coordinate, closest first (metres).
pub fn reverse(conn: &Connection, lat: f64, lon: f64, limit: usize) -> Result<Vec<(Address, f64)>> {
    /// Half-width of the search box in degrees of latitude (~2.2 km).
    const DELTA_DEGREES: f64 = 0.02;
    // Longitude degrees shrink with latitude: widen the box east-west so
    // the search reaches the same ground distance in both axes.
    let lon_delta = DELTA_DEGREES / lat.to_radians().cos().abs().max(0.1);
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
        lon - lon_delta,
        lon + lon_delta,
    ])?;
    let mut results: Vec<(Address, f64)> = Vec::new();
    while let Some(row) = rows.next()? {
        let address = row_to_address(row)?;
        let metres = domain::geo::haversine_km(
            domain::LatLon::new(lat, lon),
            domain::LatLon::new(address.lat, address.lon),
        ) * 1000.0;
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
    street_score(query, address)
        + type_score(query, address)
        + locality_score(query, address)
        + state_score(query, address)
        + postcode_score(query, address)
        + number_score(query, address)
}

/// Street-name agreement: exact beats prefix beats partial.
fn street_score(query: &GeocodeQuery, address: &Address) -> i32 {
    let wanted = normalise(&query.street);
    if wanted.is_empty() {
        return 0;
    }
    let found = normalise(&address.street);
    if found == wanted {
        200
    } else if found.starts_with(&wanted) {
        120
    } else {
        60
    }
}

/// Street-type agreement (`St` matches `Street`, anything else is penalised).
fn type_score(query: &GeocodeQuery, address: &Address) -> i32 {
    query.street_type.as_ref().map_or(0, |wanted| {
        let found = address.street_type.as_deref().unwrap_or_default();
        if types_equivalent(wanted, found) {
            40
        } else {
            -20
        }
    })
}

/// Locality agreement: exact beats substring.
fn locality_score(query: &GeocodeQuery, address: &Address) -> i32 {
    let Some(locality) = &query.locality else {
        return 0;
    };
    let wanted = normalise(locality);
    let found = normalise(&address.locality);
    if found == wanted {
        120
    } else if found.contains(&wanted) {
        60
    } else {
        0
    }
}

/// Penalty when the result is in a different state than asked for.
fn state_score(query: &GeocodeQuery, address: &Address) -> i32 {
    match &query.state {
        Some(state) if !address.state.eq_ignore_ascii_case(state) => -40,
        _ => 0,
    }
}

/// Penalty for a mismatched postcode; zero when unasked or matching.
fn postcode_score(query: &GeocodeQuery, address: &Address) -> i32 {
    match &query.postcode {
        Some(postcode) if address.postcode.as_deref() != Some(postcode.as_str()) => -40,
        _ => 0,
    }
}

/// Number-in-range bonus (or penalty) when the query named a number.
fn number_score(query: &GeocodeQuery, address: &Address) -> i32 {
    match query.number {
        Some(number) if address.number_matches(number) => 150,
        Some(_) => -60,
        None => 0,
    }
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

    #[test]
    fn escape_like_escapes_wildcards_and_escape_char() {
        assert_eq!(escape_like(r"100%_test\road"), r"100\%\_test\\road");
        assert_eq!(prefix_upper_bound("SYD").as_deref(), Some("SYE"));
        assert_eq!(prefix_upper_bound("A").as_deref(), Some("B"));
        assert_eq!(prefix_upper_bound(""), None);
    }
}
