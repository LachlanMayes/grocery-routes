# `geo-au` — G-NAF flat-file ingest and geocoding

- Imports unzipped G-NAF flat files into SQLite (`import-gnaf`).
- `geocode` (free-text) and `reverse` (coordinate) lookups.
- Queries use `LIKE` scans (no FTS/R-tree); see `docs/GNAF.md`.
- Validated against the G-NAF Aug 2026 release (16,970,406 addresses,
  0 skipped); `geocode`/`reverse` return sensible results.
