# `api` — axum + SQLite HTTP API

Endpoints for `/problems`, `/solve`, `/solutions`, `/solutions/{id}/geometry`,
`/solutions/{id}/kpis`, and the `/events` SSE solve-event stream. OSM graphs
are cached per canonical PBF path with a 16 MiB request body limit.
See `docs/API.md`. `console` embeds this API in-process by default (`--no-embedded-api` to connect externally).
