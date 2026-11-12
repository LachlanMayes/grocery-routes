# `geo-au` — G-NAF flat-file ingest and geocoding

- Imports unzipped G-NAF flat files into SQLite (`import-gnaf`).
- `geocode` (free-text) and `reverse` (coordinate) lookups.
- Queries use `LIKE` scans (no FTS/R-tree); see `docs/GNAF.md`.
- `import-stores` CLI command geocodes a CSV of store addresses into a
  problem JSON (store-import wiring).
