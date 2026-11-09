# CLI reference (`crates/cli`)

Binary: `cargo run -p cli -- <COMMAND>` (or the built `grocery-routes` binary).
Every subcommand accepts `--help`. Paths are relative to the current working
directory. Exit code is non-zero on any error.

## `gen-sim` — generate a synthetic problem

```
cargo run -p cli -- gen-sim --chain coles --city sydney --stores 40 \
  --depots 3 --vehicles 12 --seed 42 --out data/demo/problem.json
```

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--chain` | enum | `coles` | `coles`, `woolworths`, `aldi`, `iga` |
| `--city` | enum | `sydney` | `sydney`, `melbourne` |
| `--stores` | usize | 40 | M0 solver accepts at most 5,000 nodes |
| `--depots` | usize | 3 | |
| `--vehicles` | usize | 12 | one driver generated per vehicle |
| `--seed` | u64 | 42 | deterministic: same seed → same problem |
| `--horizon-start` | RFC 3339 | — | absolute instant of horizon minute zero (e.g. `2026-11-05T00:00:00Z`); resolves local civil times and holiday flags |
| `--timezones` | bool | false | stamps the city's IANA timezone on every depot/store; pair with `--horizon-start` |
| `--christmas-peak` | bool | false | higher demand + wider receiving windows (illustrative) |
| `--out` | path | required | problem JSON output |

## `solve` — solve a problem

```
cargo run -p cli -- solve --problem data/demo/problem.json \
  --out data/demo/solution.json
```

Road-network variant:

```
cargo run -p cli -- solve --problem data/demo/problem.json \
  --out data/demo/solution.json \
  --osm data/osm/australia-latest.osm.pbf --hgv \
  --maxweight 42.5 --maxheight 4.3 \
  --constraints constraints/truck-constraints.json
```

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--problem` | path | required | |
| `--out` | path | required | |
| `--ls` | bool | false | enable LNS/ALNS metaheuristic (M2) |
| `--repeat` | usize | 1 | requires `--ls`; runs N independent searches in parallel, keeps best |
| `--osm` | path | — | OSM PBF extract; switches to real road travel times |
| `--maxweight` | f64 | — | tonnes, truck constraint filter |
| `--maxheight` | f64 | — | metres, truck constraint filter |
| `--hgv` | bool | false | classify vehicle as HGV |
| `--constraints` | path | — | requires `--osm`; curated JSON folded onto ways (tightening only) |
| `--ch` | bool | false | requires `--osm`; contraction-hierarchy matrices |
| `--enforce-curfews` | bool | false | re-schedule the plan with time-dependent leg times |
| `--network-timezone` | string | first depot's tz | requires `--enforce-curfews`; IANA tz for curfew resolution |
| `--relay` | bool | false | post-pass: convert routes to overnight relay (two shifts) where feasible |
| `--curfew-aware` | bool | false | requires `--osm` + curfew rules + horizon anchor; legs time-dependent during insertion |

## `export` — write route sheet + GeoJSON

```
cargo run -p cli -- export --problem data/demo/problem.json \
  --solution data/demo/solution.json --out-dir data/demo
```

Writes `route-sheet.html` and `routes.geojson` to `--out-dir`. Route sheet
includes local times, holiday flags, OSM deep links, and a returnables ledger
section; `export` also writes `returnables.csv`.

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--problem` | path | required | |
| `--solution` | path | required | |
| `--out-dir` | path | required | alias `--out` |

## `holidays` — print a public holiday calendar

```
cargo run -p cli -- holidays --year 2026 --state VIC
```

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--year` | i16 | required | |
| `--state` | string | — | ACT, NSW, NT, QLD, SA, TAS, VIC, WA; omit for national |

## `audit` — corridor audit of a solved plan

```
cargo run -p cli -- audit --problem data/demo/problem.json \
  --solution data/demo/solution.json --osm data/osm/australia-latest.osm.pbf \
  --hgv --maxweight 42.5 --out-dir data/demo
```

Writes `corridor-audit.csv` listing every traversed way and its truck-limit
data. Add `--ch` when the plan was solved with `solve --ch`.

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--problem` | path | required | |
| `--solution` | path | required | |
| `--osm` | path | required | |
| `--maxweight` | f64 | — | tonnes, context |
| `--maxheight` | f64 | — | metres, context |
| `--hgv` | bool | false | context |
| `--ch` | bool | false | replay CH (time-optimal) paths |
| `--out-dir` | path | required | alias `--out` |

## `import-stores` — geocode a CSV of store addresses into a problem

```
cargo run -p cli -- import-stores \
  --gnaf-db data/gnaf.sqlite \
  --addresses data/demo/addresses.csv \
  --out data/demo/problem.json
```

CSV format: `name,address` header, one row per store. Each address is
geocoded against the G-NAF database; stores with no match are skipped.
The depot is placed at the centroid of the geocoded stores.

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--gnaf-db` | path | required | G-NAF SQLite DB from `import-gnaf` |
| `--addresses` | path | required | CSV with `name,address` header |
| `--out` | path | required | problem JSON output |
| `--vehicles` | usize | 4 | one driver per vehicle |

```
cargo run -p cli -- constraints --file constraints/truck-constraints.json \
  --sqlite data/constraints.sqlite
```

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--file` | path | required | curated JSON to validate |
| `--sqlite` | path | — | optional SQLite export (`curated_entries` table, ordered by way id) |

## `import-gnaf` — build a G-NAF geocoding database

```
cargo run -p cli -- import-gnaf --dir data/gnaf --db data/gnaf.sqlite
```

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--dir` | path | required | unzipped G-NAF flat files (searched recursively) |
| `--db` | path | required | SQLite DB to build (replaced if exists) |

## `geocode` — free-text address lookup

```
cargo run -p cli -- geocode --db data/gnaf.sqlite "12 Smith St, Sydney NSW 2000"
```

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--db` | path | required | G-NAF SQLite DB |
| `address` | string | required | positional |
| `--limit` | usize | 5 | max matches |

## `reverse` — nearest addresses to a coordinate

```
cargo run -p cli -- reverse --db data/gnaf.sqlite --lat=-33.87 --lon=151.21
```

| Flag | Type | Default | Notes |
|---|---|---|---|
| `--db` | path | required | G-NAF SQLite DB |
| `--lat` | f64 | required | negative values need `--lat=-33.87` or a space |
| `--lon` | f64 | required | |
| `--limit` | usize | 5 | max matches |

## See also

- `docs/TRUCK-CONSTRAINTS.md` — enforcement model and curated-entry rule set.
- `docs/GNAF.md` — G-NAF acquisition, licensing, limits.
- `docs/PLAN.md` — roadmap, milestones, current gate outcomes.
