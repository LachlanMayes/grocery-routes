# `api` — axum + SQLite HTTP API

Endpoints for import/solve/solution/KPIs and an SSE solve-event stream.
OSM graphs cached per canonical PBF path. Request body limit 16 MiB.
See `docs/API.md`. The console (`apps/console`) is the egui client.
