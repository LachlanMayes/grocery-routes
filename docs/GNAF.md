# G-NAF geocoding (`geo-au`)

G-NAF (Geoscape's Geocoded National Address File) is the open address
register for Australia: every address, its coordinates and its
administrative parents (street, locality, state, postcode). `geo-au` ingests
the published flat files into a small SQLite database and answers two
questions:

- **geocode**: where is "12 Smith St, Sydney NSW 2000"?
- **reverse**: what addresses are near (-33.8688, 151.2093)?

## Getting the data

1. Download a G-NAF release from [data.gov.au](https://data.gov.au)
   (quarterly; roughly 5 GB unpacked).
2. Unzip the per-state **flat file** sets (the `_psv` variants) into one
   directory — the importer searches it recursively, so the release's own
   folder structure is fine.
3. Import:

```console
cargo run -p cli -- import-gnaf --dir data/gnaf --db data/gnaf.sqlite
cargo run -p cli -- geocode --db data/gnaf.sqlite "12 Smith St, Sydney NSW 2000"
cargo run -p cli -- reverse --db data/gnaf.sqlite --lat=-33.8688 --lon=151.2093
```

`data/` is gitignored: the database is generated locally, never committed.

## What is implemented

| Piece | Detail |
|---|---|
| Flat-file reader | Pipe-separated, header-driven, quote-aware (`geo-au/src/flatfile.rs`). A missing column fails with the file name and the headers it saw. |
| Ingest | STATE, LOCALITY, STREET_LOCALITY, ADDRESS_DETAIL and ADDRESS_DEFAULT_GEOCODE files, per state, into SQLite (`import.rs`). Supports maps are held in memory; address rows stream through the geocode table. |
| Query | Simple parser (number, street + optional type, locality after a comma, state code, 4-digit postcode anywhere) plus a ranking heuristic (`query.rs`). |
| Reverse | Bounding-box scan plus haversine, nearest first. |

## Limits — read before trusting results

- **Not validated against a full G-NAF release.** Ingestion and queries are
  exercised against synthetic fixtures in the repository's own release
  format. Run `import-gnaf` against a real release before relying on it and
  check the printed `skipped` counters (incomplete rows, addresses without a
  geocode) against the release notes.
- **Zip archives are not read.** Unzip first.
- **Queries use `LIKE` scans** with no FTS or R-tree: fine for interactive
  lookups at state-scale data, not a high-throughput geocoding service.
- **No lineage beyond `ADDRESS_DETAIL_PID`.** Alias/primary-secondary
  relationships, `CONFIDENCE` and mesh-block data are not modelled.
- The query parser is a heuristic, not an address grammar; use the comma
  form (`"12 Smith St, Sydney NSW"`) when a query misparses.

## Licensing

G-NAF is free to use under the **Open G-NAF EULA** (CC BY 4.0-based). The
only notable restriction concerns using the data to *send mail*, which does
not apply to routing. Attribution is required when redistributing: this
repository ships **no** G-NAF data, only the importer.

## Status

Validated against the G-NAF Aug 2026 release (GDA2020, `g-naf_aug26_allstates_gda2020_psv_110.zip`,
16,970,406 addresses imported, 0 incomplete rows, 0 without a geocode).
`geocode` and `reverse` return sensible results (e.g. "1 Pitt Street, Sydney NSW 2000"
→ 18 PITT STREET; reverse at (-33.8688, 151.2093) → nearest addresses within
metres). Wiring geocoding into store import remains an optional engineering step
(tracked in [docs/PLAN.md](PLAN.md)).
