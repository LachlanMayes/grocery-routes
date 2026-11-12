# M4 report — product shell

Date: 2026-11-07 (historical snapshot; current state in [PLAN.md](../PLAN.md)).
Scope and gates per [PLAN.md](../PLAN.md).

## What was delivered

- **API** (`api`): axum + SQLite endpoints, SSE solve events, import
  validation, 16 MiB request body limit, OSM graph cache keyed by canonical
  PBF path. Endpoints: `POST /problems`, `GET /problems/{id}`, `POST /solve`,
  `GET /solutions/{id}`, `GET /solutions/{id}/kpis`, `GET /events`.
- **Console** (`console`): egui operator client covering import/solve/
  solutions/KPIs/events, talks to the API on `127.0.0.1:8080`.
- **Console usability pass** (2026-11-08): auto-fill of problem/solution IDs
  across tabs on success, `Copy ID` buttons, `Go to Solve` / `View Solution` /
  `View KPIs` navigation, docked `Panel::left` sidebar, 12-row paste box;
  verified end-to-end via Windows UI Automation.

## Not done (by design, M6 decision 2026-11-05)

- Live tracking, proof of delivery, driver app, further console surface.
  The route sheet (with OSM deep links, local times and holiday flags) is the
  driver-side deliverable.

## Checks

- `cargo test --workspace`: passes.
- `cargo clippy --workspace --all-targets`: 0 warnings.
- Console builds with eframe/egui 0.37 (glow renderer) as of 2026-11-07.
