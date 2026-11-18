# M3 report — road reality

Date: 2026-11-07 (historical snapshot; current state in [PLAN.md](../PLAN.md)).
Scope and gates per [PLAN.md](../PLAN.md).

> Historical snapshot. Real-extract timing/memory remain operator steps
> (see [PLAN.md](../PLAN.md) "Known gaps").

## What was delivered (in software)

- **OSM ingest**: `osmpbf` → directed graph with highway classification,
  one-way handling and turn restrictions awareness.
- **Truck constraints**: OSM tag filters (weight/height/HGV/access) plus a
  curated per-way layer (`routing::curated::CuratedConstraints`) that can only
  tighten limits, with provenance requirements.
- **Matrices**: `CachedMatrix` (Dijkstra all-pairs, per strategy) and an
  opt-in contraction hierarchy (`routing::ch` over `fast_paths`).
- **Time-dependent curfews**: `routing::timed` computes time-dependent legs
  (a truck waits at the previous node until a traversal is clear of every
  window); `solver::curfew` re-schedules solved plans.
- **Corridor audit**: `audit` lists every traversed way and flags those with
  no truck-limit data.

## Measured (real extract, 2026-11-09)

OSM extract: Geofabrik NSW (`nsw-latest.osm.pbf`, 258 MB). Problem:
`data/demo/console-test-problem.json` (8 stores, 2 depots, 4 vehicles).

Ingest now collects only nodes that lie on a drivable way (and handles
dense nodes, which Geofabrik extracts use); the NSW graph has **4.89 M
road nodes** (down from 36 M when every OSM node was loaded).

Release-mode wall times (incl. cargo cache hit, solve only):

| Method | Wall time | Routes | Served | Distance |
|---|---|---|---|---|
| Dijkstra (`--osm`) | 205.9 s | 2 | 7/8 | 320.2 km |
| CH (`--osm --ch`) | 126.8 s | 2 | 7/8 | 359.6 km |

CH is ~1.6× faster on this extract. 18 of 100 problem-pair entries are
unreachable (nearest-node snapping onto a disconnected road component); the
matrix still covers 82 % of pairs. CH remains opt-in, and the per-pair
distance differs slightly between methods because CH is time-optimal while
Dijkstra here is distance-based.

The full Australia extract (928 MB) is not tractable for interactive use
in debug mode; NSW is the recommended granularity for development.

## Not done (data, not code)

- Real OSM extract timing/memory (needs an extract on disk).
- Human verification of corridor legality; `constraints/truck-constraints.json`
  ships empty by design.
- Curfew-aware optimisation inside the search (M5 follow-up; construction
  shipped later in M5).

## Checks

- `cargo test --workspace`: passes (2026-11-07).
- `cargo clippy --workspace --all-targets`: 0 warnings.
- `cargo audit`: exit 0.
