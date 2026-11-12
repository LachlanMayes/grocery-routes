# grocery-routes

Route + delivery planning for Australian supermarket distribution — Coles,
Woolworths, ALDI and IGA/Metcash networks. Rust workspace, built from scratch.

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

Optional solver switches: `--ls` (LNS; add `--repeat 8` for parallel
multi-start), `--ch` (contraction-hierarchy matrices, time-optimal),
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

In the Import tab, click **Browse…** to pick a problem JSON from a native
Windows file dialog (or type/paste the path directly), then pick it from the
**Recent problems** dropdown in the Solve tab (no need to copy IDs by hand).
Successful imports and solves pre-fill the following tab, and each success row
has a **Copy ID** button. The Solutions tab shows a structured view: stat
cards (served/routes/distance/cost), a per-route table (vehicle, drivers,
stops, km, duration, cost) with a named stop timeline for the selected route,
an unassigned-stores list, and a collapsible raw-JSON debug section. The
KPIs tab shows card-style stats, a serve-rate progress bar and a per-route
bar chart (egui_plot; Cost or Distance metric). The Map tab draws each
route on OpenStreetMap tiles - following real roads when the solution was
solved with an OSM extract (road polylines via `GET /solutions/{id}/geometry`),
or straight legs otherwise - with depot/store markers, route visibility
toggles, camera auto-fit, zoom controls and the required OSM attribution.

## Checks

```console
cargo test --workspace
cargo clippy --workspace --all-targets
cargo bench -p solver --no-run
```

The workspace builds with zero `clippy::all` warnings as of 2026-11-09
(whole-tree sweep); `-D warnings` is still not enforced. 254 tests pass.

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
