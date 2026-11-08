# M3 report — road reality

Date: 2026-11-07 (historical snapshot; current state in [PLAN.md](PLAN.md)).
Scope and gates per [PLAN.md](PLAN.md).

> Historical snapshot. Real-extract timing/memory remain operator steps
> (see [PLAN.md](PLAN.md) "Known gaps").

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

## Measured (synthetic)

Contraction hierarchy on a synthetic 15×15 grid (50,625 ordered pairs):

| Method | Time |
|---|---|
| Dijkstra all-pairs | 22.08 s |
| CH prepare | 15 ms |
| CH queries | 0.68 s |

~30× speed-up. CH is opt-in (`solve --ch`).

## Not done (data, not code)

- Real OSM extract timing/memory (needs an extract on disk).
- Human verification of corridor legality; `constraints/truck-constraints.json`
  ships empty by design.
- Curfew-aware optimisation inside the search (M5 follow-up; construction
  shipped later in Task 5).

## Checks

- `cargo test --workspace`: passes (2026-11-07).
- `cargo clippy --workspace --all-targets`: 0 warnings.
- `cargo audit`: exit 0.
