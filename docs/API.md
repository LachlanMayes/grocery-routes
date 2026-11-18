# API reference (`api`)

Run with `cargo run -p api -- --db data/demo/api.sqlite` (creates the SQLite DB
if missing). Default listen address `127.0.0.1:8080`; override with `--addr`.
Request bodies are capped at 16 MiB. The standalone API is launched with
`cargo run -p api -- --db <path>`; the desktop console (`cargo run -p
console`) embeds this API in-process by default, and `--no-embedded-api`
points it at a standalone API instead.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/problems` | Import a problem JSON |
| `GET` | `/problems` | List recent imported problems (summaries) |
| `GET` | `/problems/{id}` | Retrieve a problem |
| `POST` | `/solve` | Solve a problem |
| `GET` | `/solutions` | List recent solutions (summaries) |
| `GET` | `/solutions/{id}` | Retrieve a solution |
| `GET` | `/solutions/{id}/geometry` | Road polylines per route (OSM-solved plans) |
| `GET` | `/solutions/{id}/kpis` | KPI summary for a solution |
| `GET` | `/events` | SSE stream of live solve events |

### `POST /problems`

Body: `{ "problem": <domain::Problem> }`. Returns `{ "id": "<string>" }`.
Validates dense `0..len` ids for depots/stores/vehicles/drivers, valid depot
references, finite WGS-84 coordinates (`lat` in `[-90, 90]`, `lon` in
`[-180, 180]`), positive finite vehicle speeds (`avg_speed_kmh > 0`), valid
time windows (`start <= end`), valid shifts (`start < end`), valid IANA
timezones, and the `5,000`-node ceiling (`depots + stores <= 5000`); returns
`400 Bad Request` on failure.

### `GET /problems`

List the most recently imported problems, newest first. Query params:
`limit` (optional, default 20, clamped to 1..=100). Returns an array of:

```json
[{ "id": "...", "name": "sim coles ...", "created_at": "2026-11-11 20:59:15",
   "stores": 8, "vehicles": 4, "depots": 2 }]
```

Store, vehicle, and depot counts are persisted on insert and backfilled
from the problem JSON on read for older databases, so listing recent
problems stays fast for dropdowns without reparsing large payloads.

### `GET /problems/{id}`

Returns the stored `domain::Problem` (or `404`).

### `POST /solve`

Body:

```json
{
  "problem_id": "<id>",
  "lns": false,
  "local_search": false,
  "ch": false,
  "osm_path": null,
  "maxweight": null,
  "maxheight": null,
  "hgv": false
}
```

- `lns` — use LNS/ALNS instead of greedy.
- `local_search` — run iterative local search when `lns` is false.
- `ch` — use Contraction Hierarchies instead of Dijkstra for OSM matrix queries (`osm_path` only).
- `osm_path` — OSM PBF for road-network distances (optional).
- `maxweight` / `maxheight` / `hgv` — truck filters, used with `osm_path` (persisted on the solution for geometry replay). When set, `maxweight` and `maxheight` must be finite and `> 0` (`400 Bad Request` otherwise).

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

### `GET /solutions/{id}/geometry`

Returns the road geometry for a solution's routes: `{ "routes": [[[lat, lon], ...]], "route_legs": [[{ "points": [[lat, lon], ...], "fallback": false }]] }`.
Every consecutive pair of stops is routed over the road network (A*) using the
**persisted truck profile** (`maxweight`, `maxheight`, `hgv`) recorded at solve
time, with exact depot/store coordinates tethered at leg endpoints and seam
vertices deduplicated. Any disconnected leg falls back to a two-point
straight segment marked `"fallback": true`.

Requires the solution to have been solved with `osm_path` (its extract is
recorded); otherwise `routes` and `route_legs` are empty. Each leg costs one
point-to-point search (tens of milliseconds on a metro-area extract, seconds
on the full NSW graph).

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

- The API solves with greedy, Local Search (`local_search`), or LNS/ALNS
  (`lns`), with optional Contraction Hierarchies (`ch`) when `osm_path` is set;
  relay, curfew enforcement/awareness, and the curated constraints file are
  CLI features.
- OSM graphs are cached per canonical PBF path using `OnceCell` so concurrent requests wait without holding the map lock while parsing.
- The console pre-fills the following tab after a successful import/solve,
  offers **Copy ID** buttons on success rows, and shows **recent
  problems/solutions** dropdowns (backed by `GET /problems` and
  `GET /solutions`) in the Solve, Solutions and KPIs tabs; the Refresh button
  re-fetches the lists.
