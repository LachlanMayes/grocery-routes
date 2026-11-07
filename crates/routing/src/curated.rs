//! Curated, hand-verified truck constraints folded onto OSM ways.
//!
//! OSM tags (`maxweight`, `maxheight`, `hgv`, `access`) are usually absent —
//! the M0 Overpass study found `maxweight` on 3.2% of drivable Sydney ways,
//! `maxheight` on 0.8% and `hgv` on 0.4%. Where a legal limit exists but is
//! not tagged, this module lets an operator attach a hand-verified limit to
//! a specific OSM way id via a JSON file:
//!
//! ```json
//! {
//!   "entries": [
//!     {
//!       "way_id": 123456789,
//!       "maxweight_tonnes": 12.5,
//!       "maxheight_m": null,
//!       "note": "Bridge load limit, signposted",
//!       "source": "NSW TfNSW heavy vehicle road manager, verified on site",
//!       "verified_by": "j.smith",
//!       "verified_at": "2026-10-01"
//!     }
//!   ]
//! }
//! ```
//!
//! Every entry must carry provenance (`source`, `verified_by`,
//! `verified_at`); entries missing it fail to load, so the file cannot
//! silently accumulate unverifiable limits. Curated limits can only
//! *tighten* what the graph already knows, never loosen it.
//!
//! This models what the operator has verified — nothing more. Time-based
//! curfews (e.g. peak-hour restrictions) are deliberately out of scope
//! because routing is not yet departure-time aware.

use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

use crate::graph::Graph;

/// One curated constraint on a single OSM way.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuratedEntry {
    /// OSM way id the constraint applies to.
    pub way_id: i64,
    /// Maximum legal weight in tonnes, if signposted/verified.
    pub maxweight_tonnes: Option<f64>,
    /// Maximum legal height in metres, if signposted/verified.
    pub maxheight_m: Option<f64>,
    /// What the limit is and why (e.g. "Bridge load limit, signposted").
    pub note: String,
    /// Where the limit came from (e.g. "TfNSW, verified on site 2026-09").
    pub source: String,
    /// Who verified it (name or handle).
    pub verified_by: String,
    /// When it was verified, ISO 8601 date (`YYYY-MM-DD`).
    pub verified_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CuratedFile {
    entries: Vec<CuratedEntry>,
}

/// Why a curated constraints file could not be loaded.
#[derive(Debug, Error)]
pub enum CuratedError {
    /// The file is not JSON matching the documented schema.
    #[error("constraints file is not valid JSON: {0}")]
    Parse(#[from] serde_json::Error),
    /// An entry failed validation.
    #[error("entry for way {way_id}: {reason}")]
    InvalidEntry {
        /// OSM way id of the offending entry (0 if unknown).
        way_id: i64,
        /// What is wrong with the entry.
        reason: String,
    },
    /// Two entries target the same way.
    #[error("duplicate curated entry for way {way_id}")]
    DuplicateWay {
        /// OSM way id with more than one entry.
        way_id: i64,
    },
}

/// A loaded and validated set of curated constraints.
#[derive(Clone, Debug, Default)]
pub struct CuratedConstraints {
    by_way: HashMap<i64, CuratedEntry>,
}

/// Upper bounds used to reject typos like `125` instead of `12.5`.
const MAX_REASONABLE_WEIGHT_T: f64 = 200.0;
const MAX_REASONABLE_HEIGHT_M: f64 = 10.0;

impl CuratedConstraints {
    /// Parses and validates a curated constraints JSON document.
    ///
    /// # Errors
    /// Returns [`CuratedError`] for malformed JSON, missing/empty provenance,
    /// missing limits, implausible values, bad dates or duplicate way ids.
    pub fn from_json_str(raw: &str) -> Result<Self, CuratedError> {
        let file: CuratedFile = serde_json::from_str(raw)?;
        let mut by_way = HashMap::with_capacity(file.entries.len());
        for entry in file.entries {
            validate_entry(&entry)?;
            let way_id = entry.way_id;
            if by_way.insert(way_id, entry).is_some() {
                return Err(CuratedError::DuplicateWay { way_id });
            }
        }
        Ok(Self { by_way })
    }

    /// Number of curated entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_way.len()
    }

    /// True when the file declares no constraints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_way.is_empty()
    }

    /// The curated entry for a way, if any.
    #[must_use]
    pub fn entry_for(&self, way_id: i64) -> Option<&CuratedEntry> {
        self.by_way.get(&way_id)
    }

    /// All curated entries, ordered by way id for deterministic output.
    pub fn entries(&self) -> Vec<&CuratedEntry> {
        let mut entries: Vec<_> = self.by_way.values().collect();
        entries.sort_by_key(|entry| entry.way_id);
        entries
    }

    /// Folds curated limits into the graph.
    ///
    /// For every edge whose OSM way id has a curated entry, the edge's
    /// `maxweight_tonnes`/`maxheight_m` become the *tightest* of (existing,
    /// curated). Curated limits never loosen anything. Returns the number of
    /// edges tightened.
    pub fn apply(&self, graph: &mut Graph) -> usize {
        let mut tightened = 0;
        for edge in graph.edges_mut() {
            let Some(way_id) = edge.way_id else {
                continue;
            };
            let Some(entry) = self.by_way.get(&way_id) else {
                continue;
            };
            let mut changed = false;
            if let Some(weight) = entry.maxweight_tonnes {
                let merged = tighten(edge.maxweight_tonnes, weight);
                if merged != edge.maxweight_tonnes {
                    edge.maxweight_tonnes = merged;
                    changed = true;
                }
            }
            if let Some(height) = entry.maxheight_m {
                let merged = tighten(edge.maxheight_m, height);
                if merged != edge.maxheight_m {
                    edge.maxheight_m = merged;
                    changed = true;
                }
            }
            if changed {
                tightened += 1;
            }
        }
        tightened
    }
}

/// Tightest of existing and curated; curated fills in a missing limit.
fn tighten(existing: Option<f64>, curated: f64) -> Option<f64> {
    Some(existing.map_or(curated, |current| current.min(curated)))
}

fn validate_entry(entry: &CuratedEntry) -> Result<(), CuratedError> {
    let err = |reason: String| CuratedError::InvalidEntry {
        way_id: entry.way_id,
        reason,
    };
    if entry.way_id <= 0 {
        return Err(err("way_id must be a positive OSM way id".to_string()));
    }
    for (field, value) in [
        ("note", &entry.note),
        ("source", &entry.source),
        ("verified_by", &entry.verified_by),
        ("verified_at", &entry.verified_at),
    ] {
        if value.trim().is_empty() {
            return Err(err(format!("{field} must not be empty")));
        }
    }
    if entry.verified_at.parse::<jiff::civil::Date>().is_err() {
        return Err(err(format!(
            "verified_at '{}' is not a valid ISO 8601 date (YYYY-MM-DD)",
            entry.verified_at
        )));
    }
    if entry.maxweight_tonnes.is_none() && entry.maxheight_m.is_none() {
        return Err(err(
            "entry must set maxweight_tonnes, maxheight_m or both".to_string()
        ));
    }
    for (field, value, cap) in [
        (
            "maxweight_tonnes",
            entry.maxweight_tonnes,
            MAX_REASONABLE_WEIGHT_T,
        ),
        ("maxheight_m", entry.maxheight_m, MAX_REASONABLE_HEIGHT_M),
    ] {
        if let Some(v) = value {
            if !v.is_finite() || v <= 0.0 {
                return Err(err(format!("{field} must be a positive finite number")));
            }
            if v > cap {
                return Err(err(format!("{field} of {v} is implausible (> {cap})")));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, NodeId};

    const GOOD: &str = r#"{
        "entries": [
            {
                "way_id": 123,
                "maxweight_tonnes": 12.5,
                "maxheight_m": null,
                "note": "Bridge load limit, signposted",
                "source": "TfNSW heavy vehicle road manager, verified on site",
                "verified_by": "j.smith",
                "verified_at": "2026-10-01"
            }
        ]
    }"#;

    fn load(raw: &str) -> Result<CuratedConstraints, CuratedError> {
        CuratedConstraints::from_json_str(raw)
    }

    fn graph_with_edge(
        way_id: Option<i64>,
        maxweight: Option<f64>,
        maxheight: Option<f64>,
    ) -> Graph {
        let mut graph = Graph::new();
        graph.add_node(NodeId(1), domain::LatLon::new(-33.8, 151.2));
        graph.add_node(NodeId(2), domain::LatLon::new(-33.9, 151.3));
        graph.add_edge_with_way(
            NodeId(1),
            NodeId(2),
            10.0,
            15,
            Some("primary".into()),
            maxweight,
            maxheight,
            None,
            None,
            false,
            way_id,
        );
        graph
    }

    #[test]
    fn repo_constraints_file_is_valid() {
        // The tracked template must always parse (it ships empty).
        let curated = load(include_str!("../../../constraints/truck-constraints.json")).unwrap();
        assert!(curated.is_empty());
    }

    #[test]
    fn empty_entries_load_and_tighten_nothing() {
        let curated = load(r#"{ "entries": [] }"#).unwrap();
        assert!(curated.is_empty());
        let mut graph = graph_with_edge(Some(123), Some(20.0), None);
        assert_eq!(curated.apply(&mut graph), 0);
        let edge = graph.edge(crate::graph::EdgeId(0)).unwrap();
        assert_eq!(edge.maxweight_tonnes, Some(20.0));
    }

    #[test]
    fn entries_are_ordered_by_way_id() {
        let doc = r#"{
            "entries": [
                {
                    "way_id": 200,
                    "maxweight_tonnes": 10.0,
                    "note": "test b",
                    "source": "test",
                    "verified_by": "test",
                    "verified_at": "2026-10-01"
                },
                {
                    "way_id": 100,
                    "maxheight_m": 4.5,
                    "note": "test a",
                    "source": "test",
                    "verified_by": "test",
                    "verified_at": "2026-10-01"
                }
            ]
        }"#;
        let curated = load(doc).unwrap();
        let ids: Vec<i64> = curated.entries().iter().map(|e| e.way_id).collect();
        assert_eq!(ids, vec![100, 200]);
    }

    #[test]
    fn valid_entry_loads() {
        let curated = load(GOOD).unwrap();
        assert_eq!(curated.len(), 1);
        let entry = curated.entry_for(123).unwrap();
        assert_eq!(entry.maxweight_tonnes, Some(12.5));
    }

    #[test]
    fn missing_provenance_fails_load() {
        for bad in [
            GOOD.replace(
                r#""source": "TfNSW heavy vehicle road manager, verified on site","#,
                "",
            ),
            GOOD.replace(r#""verified_by": "j.smith","#, ""),
            GOOD.replace(r#""verified_at": "2026-10-01""#, r#""verified_at": """#),
        ] {
            assert!(load(&bad).is_err(), "expected error for: {bad}");
        }
    }

    #[test]
    fn bad_date_fails_load() {
        let bad = GOOD.replace("2026-10-01", "2026-13-40");
        assert!(load(&bad).is_err());
    }

    #[test]
    fn unknown_field_fails_load() {
        let bad = GOOD.replace(
            r#""note": "Bridge load limit, signposted","#,
            "\"note\": \"x\", \"bogus\": true,",
        );
        assert!(load(&bad).is_err());
    }

    #[test]
    fn entry_without_limits_fails_load() {
        let bad = GOOD.replace(
            r#""maxweight_tonnes": 12.5,"#,
            r#""maxweight_tonnes": null,"#,
        );
        assert!(load(&bad).is_err());
    }

    #[test]
    fn implausible_values_fail_load() {
        for bad_weight in ["0.0", "-3.0", "1250.0", "1e999"] {
            let bad = GOOD.replace("12.5", bad_weight);
            assert!(
                load(&bad).is_err(),
                "expected error for weight {bad_weight}"
            );
        }
    }

    #[test]
    fn duplicate_way_id_fails_load() {
        let dup = GOOD.replace(
            r#""verified_at": "2026-10-01"
            }
        ]"#,
            r#""verified_at": "2026-10-01"
            },
            {
                "way_id": 123,
                "maxheight_m": 4.5,
                "note": "duplicate",
                "source": "x",
                "verified_by": "y",
                "verified_at": "2026-10-02"
            }
        ]"#,
        );
        assert!(matches!(
            load(&dup),
            Err(CuratedError::DuplicateWay { way_id: 123 })
        ));
    }

    #[test]
    fn apply_tightens_and_never_loosens() {
        let curated = load(
            r#"{
            "entries": [{
                "way_id": 123,
                "maxweight_tonnes": 12.5,
                "maxheight_m": 4.0,
                "note": "test",
                "source": "test source",
                "verified_by": "test",
                "verified_at": "2026-10-01"
            }]
        }"#,
        )
        .unwrap();

        // Existing limit above curated: tightened down.
        let mut graph = graph_with_edge(Some(123), Some(20.0), None);
        assert_eq!(curated.apply(&mut graph), 1);
        let edge = graph.edge(crate::graph::EdgeId(0)).unwrap();
        assert_eq!(edge.maxweight_tonnes, Some(12.5));
        assert_eq!(edge.maxheight_m, Some(4.0));

        // Existing limit already tighter than curated: kept.
        let mut graph = graph_with_edge(Some(123), Some(5.0), Some(3.5));
        curated.apply(&mut graph);
        let edge = graph.edge(crate::graph::EdgeId(0)).unwrap();
        assert_eq!(edge.maxweight_tonnes, Some(5.0));
        assert_eq!(edge.maxheight_m, Some(3.5));

        // Other way ids are untouched.
        let mut graph = graph_with_edge(Some(999), None, None);
        assert_eq!(curated.apply(&mut graph), 0);
        let edge = graph.edge(crate::graph::EdgeId(0)).unwrap();
        assert_eq!(edge.maxweight_tonnes, None);

        // Synthetic edges without a way id are untouched.
        let mut graph = graph_with_edge(None, None, None);
        assert_eq!(curated.apply(&mut graph), 0);
    }
}
