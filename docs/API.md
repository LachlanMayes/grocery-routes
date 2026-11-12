# API reference (`api`)

Run with `cargo run -p api -- --db data/demo/api.sqlite` (creates the SQLite DB
if missing). Default listen address `127.0.0.1:8080`; override with `--addr`.
Request bodies are capped at 16 MiB. The standalone API is launched with
`cargo run -p api -- --db <path>`; the desktop console (`cargo run -p
console`) embeds this API in-process by default, and `--no-embedded-api`
points it at a standalone API instead.
talks to this API.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/problems` | Import a problem JSON |
| `GET` | `/problems` | List recent imported problems (summaries) |
| `GET` | `/problems/{id}` | Retrieve a problem |
| `POST` | `/solve` | Solve a problem |
| `GET` | `/solutions` | List recent solutions (summaries) |
| `GET` | `/solutions/{id}` | Retrieve a solution |
| `GET` | `/solutions/{id}/kpis` | KPI summary for a solution |
| `GET` | `/events` | SSE stream of live solve events |

### `POST /problems`

Body: `{ "problem": <domain::Problem> }`. Returns `{ "id": "<string>" }`.
Validates dense 0..len ids for depots/stores/vehicles/drivers, depot
references, time windows, shifts and IANA timezones; returns `400 Bad Request`
on failure.

### `GET /problems`

List the most recently imported problems, newest first. Query params:
`limit` (optional, default 20, clamped to 1..=100). Returns an array of:

```json
[{ "id": "...", "name": "sim coles ...", "created_at": "2026-11-11 20:59:15",
   "stores": 8, "vehicles": 4, "depots": 2 }]
```

Store/vehicle/depot counts are parsed from the stored JSON on read (the
problems table has no count columns), so the endpoint is meant for recent-
items dropdowns, not bulk export.

### `GET /problems/{id}`

Returns the stored `domain::Problem` (or `404`).

### `POST /solve`

Body:

```json
{
  "problem_id": "<id>",
  "lns": false,
  "osm_path": null,
  "maxweight": null,
  "maxheight": null,
  "hgv": false
}
```

- `lns` — use LNS/ALNS instead of greedy.
- `osm_path` — OSM PBF for road-network distances (optional).
- `maxweight` / `maxheight` / `hgv` — truck filters, used only with `osm_path`.

Returns:

```json
{ "id": "<solution_id>", "served": 8, "total": 8, "distance_km": 304.43, "cost_cents": 116723 }
```

Broadcasts a `SolveEvent` to `/events` clients on completion.

### `GET /solutions`

List the most recently solved solutions, newest first. Query params: `limit`
(optional, default 20, clamped to 1..=100) and `problem_id` (optional,
restrict to one problem). Returns an array of:

```json
[{ "id": "...", "problem_id": "...", "served": 8, "total": 8,
   "distance_km": 304.43, "cost_cents": 116723,
   "solved_at": "2026-11-11 20:59:22", "used_lns": false }]
```

### `GET /solutions/{id}`

Returns `{ "id", "problem_id", "solution": <domain::Solution> }` (or `404`).

### `GET /solutions/{id}/kpis`

Returns:

```json
{ "served": 8, "total": 8, "serve_rate": 1.0, "distance_km": 304.43, "cost_cents": 116723, "routes": 3 }
```

### `GET /events`

Server-Sent Events stream. Each event is a JSON `SolveEvent`:

```json
{ "solution_id": "...", "problem_id": "...", "served": 8, "total": 8, "distance_km": 304.43, "cost_cents": 116723 }
```

## Notes

- The API solves with greedy or LNS only; relay, curfew enforcement/awareness,
  contraction hierarchies and the curated constraints file are CLI-only features.
- OSM graphs are cached per canonical PBF path for the lifetime of the process.
- The console pre-fills the following tab after a successful import/solve,
  offers **Copy ID** buttons on success rows, and shows **recent
  problems/solutions** dropdowns (backed by `GET /problems` and
  `GET /solutions`) in the Solve, Solutions and KPIs tabs; the Refresh button
  re-fetches the lists.
