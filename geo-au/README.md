# `geo-au` — G-NAF flat-file ingest and geocoding

- Imports unzipped G-NAF flat files into SQLite (`import-gnaf`).
- `geocode` (free-text) and `reverse` (coordinate) lookups.
- Queries use indexed prefix lookups (`UPPER(street)`/`UPPER(locality)`/`state`/`postcode`) with escaped `LIKE` fallback (no FTS/R-tree); see `docs/GNAF.md`.
- `import-stores` CLI command geocodes a CSV of store addresses into a
  problem JSON (store-import wiring).
