# grocery-routes

Route + delivery planning for Australian supermarket distribution — Coles,
Woolworths, ALDI and IGA/Metcash networks. Rust workspace, built from scratch.

- Roadmap, milestones and current gate outcomes: [docs/PLAN.md](docs/PLAN.md)
- Truck-constraint enforcement model (what is and is not verified):
  [docs/TRUCK-CONSTRAINTS.md](docs/TRUCK-CONSTRAINTS.md)
- G-NAF geocoding (acquisition, licensing, limits): [docs/GNAF.md](docs/GNAF.md)
- Milestone reports: [docs/M0-REPORT.md](docs/M0-REPORT.md) ·
  [docs/M1-REPORT.md](docs/M1-REPORT.md)

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
  --osm data/osm/australia-latest.osm.pbf --hgv --maxweight 42.5 --maxheight 4.3 \
  --constraints constraints/truck-constraints.json
```

Optional solver switches: `--ls` (LNS; add `--repeat 8` for parallel
multi-start), `--ch` (contraction-hierarchy matrices, time-optimal), and
`--enforce-curfews --network-timezone Australia/Sydney` (re-schedules the
plan so no traversal enters a curfewed road class during its window).

`constraints/truck-constraints.json` ships **empty** by design — see
[docs/TRUCK-CONSTRAINTS.md](docs/TRUCK-CONSTRAINTS.md) before trusting any
heavy-vehicle route. To see which corridors a plan uses and which lack
limit data:

```console
cargo run -p cli -- audit --problem data/demo/problem.json --solution data/demo/solution.json \
  --osm data/osm/australia-latest.osm.pbf --hgv --maxweight 42.5 --out-dir data/demo
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

The audit report is not legal verification. The G-NAF importer has not
been validated against a real release yet — read
[docs/GNAF.md](docs/GNAF.md) first.

`data/demo/` is gitignored; `data/solomon/C101.txt` is tracked so the C101
calibration gates run in a clean checkout. Everything generated is synthetic; no
real store addresses are shipped.

## Checks

```console
cargo test --workspace
cargo clippy --workspace --all-targets
cargo bench -p solver --no-run
```

The workspace currently builds with warnings under `clippy::all` (warn level);
it is not enforced with `-D warnings`.

## Layout

| Path | Role |
|---|---|
| `crates/domain` | timeline model + validation rules (the contract), horizon→local clock (`clock`), national + state holidays (`holidays`), returnables ledger (`ledger`) |
| `crates/sim` | seeded synthetic network generator, Christmas-peak profile |
| `crates/solver` | haversine/road matrices, greedy insertion, local search, LNS/ALNS with multi-start, curfew re-scheduling |
| `crates/routing` | OSM PBF ingest, directed graph, truck constraints (OSM tags + `curated` per-way limits), cached matrices, contraction hierarchy (`ch`), time-dependent curfews (`timed`), corridor audit (`audit`) |
| `crates/geo-au` | G-NAF flat-file ingest → SQLite; geocode and reverse |
| `crates/cli` | `gen-sim`, `solve`, `export`, `holidays`, `audit`, `import-gnaf`, `geocode`, `reverse` |
| `crates/api` | axum + SQLite API, SSE solve events, import/solve/solution/KPI endpoints |
| `apps/console` | egui operator console |
| `constraints/` | curated truck-constraints file (schema + provenance rules) |
| `docs/` | roadmap, plan status, milestone reports, truck-constraint model, G-NAF notes |
