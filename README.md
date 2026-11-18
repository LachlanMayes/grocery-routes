# grocery-routes

Transportation Management System (TMS) and delivery route optimization suite for
custom commercial fleets and Australian supermarket distribution (`General`,
`Coles`, `Woolworths`, `ALDI`, and `IGA/Metcash` networks). Pure Rust workspace
with zero proprietary runtime dependencies (`unsafe` forbidden workspace-wide).

## Key capabilities

- **Multi-compartment VRPTW & Relay Solver (`solver`, `domain`)** — Multi-temperature
  (Ambient / Chilled / Frozen) vehicle routing with time windows, NHVR driver fatigue
  rules (between-shift rest & work caps), greedy insertion, iterative local search,
  opt-in parallel LNS/ALNS (`--repeat N`), two-shift depot relay hand-overs (`--relay`),
  receiving-window curfews, DST-aware local clocks, national + state public holiday
  calendars, and returnables cage/pallet ledgers.
- **Real Road Routing & Corridor Auditing (`routing`, `geo-au`)** — OpenStreetMap
  `.osm.pbf` road graph ingestion with heavy-vehicle filtering (`--maxweight`,
  `--maxheight`, `--hgv`), Contraction Hierarchies (`--ch`), time-dependent road
  curfews, provenance-tracked curated way limits, corridor audits (`audit`), and a
  local SQLite geocoder (`import-gnaf`) supporting Australian G-NAF datasets.
- **TMS Control Tower Desktop Console & HTTP API (`console`, `api`)** — Single-binary
  `egui` operator desktop application with an embedded `axum` + SQLite backend: visual
  route setup, Australian carrier rate card presets, 3-view cargo bay cubing visualizer,
  interactive dispatch board with live CoR validation, printable HTML Run Sheets / BOLs
  with ePOD signature blocks, OTIF/detention/ESG (`CO₂e`) KPIs, and interactive
  OpenStreetMap route maps.

## Documentation

- Roadmap, milestones and current gate outcomes: [docs/PLAN.md](docs/PLAN.md)
- CLI reference (every subcommand and flag): [docs/CLI.md](docs/CLI.md)
- HTTP API reference: [docs/API.md](docs/API.md)
- Truck-constraint enforcement model (what is and is not verified):
  [docs/TRUCK-CONSTRAINTS.md](docs/TRUCK-CONSTRAINTS.md)
- Corridor research (desktop findings for the synthetic Sydney plan):
  [docs/CORRIDOR-RESEARCH.md](docs/research/CORRIDOR-RESEARCH.md)
- Step-by-step completion guide for the corridor legality task:
  [docs/CORRIDOR-CHECKLIST.md](docs/research/CORRIDOR-CHECKLIST.md)
- G-NAF geocoding (acquisition, licensing, limits): [docs/GNAF.md](docs/GNAF.md)
- Security and dependency audit: [docs/SECURITY.md](docs/SECURITY.md)
- Milestone reports: [docs/M0-REPORT.md](docs/reports/M0-REPORT.md) ·
  [docs/M1-REPORT.md](docs/reports/M1-REPORT.md)

## Quickstart

```console
cargo run -p cli -- gen-sim --chain coles --city sydney --stores 40 --depots 3 --vehicles 12 --seed 42 --out data/demo/problem.json
cargo run -p cli -- solve --problem data/demo/problem.json --out data/demo/solution.json
cargo run -p cli -- export --problem data/demo/problem.json --solution data/demo/solution.json --out-dir data/demo
```

Add `--timezones --horizon-start 2026-11-05T00:00:00Z` to `gen-sim` (and
re-run solve/export) to get DST-aware local times and national-holiday
flags in the route sheet; every node is deep-linked to OpenStreetMap, and
`export` also writes a returnables ledger (`returnables.csv`). Add
`--christmas-peak` to `gen-sim` for the peak trading profile.

For road-network routing:

```console
cargo run -p cli -- solve --problem data/demo/problem.json --out data/demo/solution.json \
  --osm data/osm/nsw-latest.osm.pbf --hgv --maxweight 42.5 --maxheight 4.3 \
  --constraints constraints/truck-constraints.json
```

Optional solver switches: `--local-search` (iterative 2-opt/relocate/exchange
local search), `--ls` (LNS; add `--repeat 8` for parallel multi-start; conflicts
with `--local-search`), `--ch` (contraction-hierarchy matrices, time-optimal),
`--relay` (multi-driver hand-over post-pass over (driver, shift) pairs),
`--curfew-aware` (build routes through the `Legs` provider so receiving
windows are respected at insertion time), and
`--enforce-curfews --network-timezone Australia/Sydney` (re-schedules the
plan so no traversal enters a curfewed road class during its window).

`constraints/truck-constraints.json` ships **empty** by design — see
[docs/TRUCK-CONSTRAINTS.md](docs/TRUCK-CONSTRAINTS.md) before trusting any
heavy-vehicle route. To see which corridors a plan uses and which lack
limit data:

```console
cargo run -p cli -- audit --problem data/demo/problem.json --solution data/demo/solution.json \
  --osm data/osm/nsw-latest.osm.pbf --hgv --maxweight 42.5 --out-dir data/demo
```

Add `--ch` when the plan was solved with `solve --ch`: the audit then
replays the same time-optimal paths, verifying the ways the plan really
uses.

Calendars and geocoding:

```console
cargo run -p cli -- holidays --year 2026 --state VIC
cargo run -p cli -- import-gnaf --dir data/gnaf --db data/gnaf.sqlite
cargo run -p cli -- geocode --db data/gnaf.sqlite "12 Smith St, Sydney NSW 2000"
```

The audit report is not legal verification. The G-NAF importer has been
validated against the Aug 2026 release (16.97 M addresses, 0 skipped) — read
[docs/GNAF.md](docs/GNAF.md).

`data/demo/` is gitignored; `data/solomon/C101.txt` is tracked so the C101
calibration gates run in a clean checkout. Everything generated is synthetic; no
real store addresses are shipped.

## Operator console

The console is a single app that embeds the API in-process — one command,
no second terminal:

```console
cargo run -p console
```

It starts an embedded API on `127.0.0.1:8080` backed by
`data/demo/api.sqlite` (created if missing). To run the UI against a
standalone API instead, start it (`cargo run -p api -- --db data/demo/api.sqlite`)
and launch the console with `--no-embedded-api`; if port 8080 is already in
use, the console falls back to the external API automatically. Pass `--db` or
`--addr` to change the embedded database or address (`console --help`).

- **Orders & Network** — Select saved problems (**Set active problem for Solve** /
  **Load into Builder**) or configure custom fleets under **Set Up Route (Custom Depot &
  Places)**: enter a **Main Depot** address or `lat, lon` (geocoded via local G-NAF DB
  or OpenStreetMap Nominatim, or picked on the **Interactive Map Picker**), add stores
  individually or via **Bulk Paste…**, manage depots, vehicles, inline **Driver Shift
  (m)** schedules (**Auto-Fit Shifts to Windows**, **Clear Places**), and **Rate Card
  & Meta** (four Australian **Carrier Rate Card Presets** — `Metro Van`, `12-Pallet
  Rigid`, `22-Pallet Semi`, `Linehaul B-Double` — plus a live **Benchmark 100 km / 4 hr
  run** preview). Trigger 1-click **Solve Route Now**, **Import & Use Route**, or **Save
  JSON…**, with collapsible drawers for **Synthetic Scenario Generator** presets and
  **File Import / Raw Problem JSON**.
- **Load Planner** — Optimize physical truck capacity across Ambient/Chilled/Frozen zones
  with an interactive 3-view visualizer (top-down cargo deck with LIFO stop badges and
  hover tooltips, side elevation with Center-of-Gravity marker, and rear door
  cross-section). Configure bay dimensions (`length`, `width`, `height`, `door
  clearance`, `bulkhead thickness`, usable volume override / packing factor, `payload
  weight`, `tare weight`, `external height`) or pick fleet presets (`Delivery Van` to
  `B-Double`), configure handling units (roll cages, AU/Euro pallets, crates, or custom
  dimensions, stacking, and 90°/hybrid rotation), inspect bottlenecks, and push
  capacities and HGV weight/height limits straight into the draft fleet and
  **Optimizer** tab.
- **Optimizer** — Configure solver mode (**Local search** on by default, **LNS**, and
  **Contraction Hierarchy**), auto-discover or browse `.osm.pbf` road extracts, and set
  validated heavy-vehicle limits (`Max weight`, `Max height`, `HGV`). Successful solves
  automatically populate downstream tabs.
- **Dispatch & BOL** — Interactive dispatch board featuring summary KPIs, a live **CoR &
  Schedule** validation banner (`Solution::validate`), **Copy Route Manifest**, **Export
  Schedule CSV…**, and **Export Run Sheet / BOL (HTML)…** (printable driver run sheets &
  Bills of Lading with CoR pre-departure verification checklists and per-stop ePOD
  signature blocks). Operators can move stops (**▲** / **▼**), **Unassign**, or **+
  Assign to Route N** on the fly while recalculating ETAs and distances and syncing
  **Dispatch & BOL**, **Freight KPIs**, and **Fleet Map** in real time.
- **Freight KPIs** — Analytics dashboard covering executive summary cards (serve rate,
  **OTIF & Dock Wait**, total cost, $/km, $/stop, $/unit, active fleet %, duration, peak
  load %, and **Carbon & Fuel (ESG)** using Australian NGA diesel/CO₂e factors),
  interactive per-route charts (**Cost Breakdown**, **Distance**, **Duration**,
  **Stops**, **Load Utilization %**), a **Dispatch Timeline (Gantt)** view, cost & load
  composition breakdowns, **Solve History Comparison**, a sortable per-route leaderboard,
  an unassigned demand impact panel, and clipboard / CSV / JSON exports.
- **Fleet Map** — Interactive OpenStreetMap view (`walkers`) rendering road-aligned
  polylines (`GET /solutions/{id}/geometry` when solved with `--osm`) or straight-line
  previews with directional arrowheads, labelled markers (`D` depots, `1..N` stop
  numbers, `!` unassigned diamonds), collision-avoided node info pills, click-to-select
  **Selected Node** inspection (**◀ Prev** / **Next ▶** stop stepping, **Truck Load**
  jump, leg telemetry, unassigned root-cause diagnosis), route visibility toggles,
  auto-fit, and zoom controls.
- **Control Tower** — Live Server-Sent Events (`/events`) solve telemetry stream.

## Checks

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo bench -p solver --no-run
```

## Layout

| Path | Role |
|---|---|
| `domain` | timeline model + validation rules (the contract), horizon→local clock (`clock`), national + state holidays (`holidays`), returnables ledger (`ledger`) |
| `sim` | seeded synthetic network generator, Christmas-peak profile |
| `solver` | haversine/road matrices, greedy insertion, local search, LNS/ALNS with multi-start, curfew re-scheduling and curfew-aware construction (`Legs`), relay post-pass |
| `routing` | OSM PBF ingest, directed graph, truck constraints (OSM tags + `curated` per-way limits), cached matrices, contraction hierarchy (`ch`), time-dependent curfews (`timed`), corridor audit (`audit`) |
| `geo-au` | G-NAF flat-file ingest → SQLite; geocode and reverse |
| `cli` | `gen-sim`, `solve`, `export`, `holidays`, `audit`, `constraints`, `import-stores`, `import-gnaf`, `geocode`, `reverse` |
| `api` | axum + SQLite API (`cargo run -p api`), SSE solve events, import/solve/solution/KPI endpoints |
| `console` | egui operator console (`cargo run -p console`) — embeds the API in-process on `127.0.0.1:8080` |
| `constraints/` | curated truck-constraints file (schema + provenance rules) |
| `docs/` | roadmap + plan status (`PLAN.md`), reference (`API`, `CLI`, `GNAF`, `SECURITY`, `TRUCK-CONSTRAINTS`), `reports/` (milestone reports), `research/` (corridor legality) |

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
