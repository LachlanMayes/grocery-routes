# `routing` — OSM ingest, graph, matrices, truck constraints, CH, curfews, audit

- `osmpbf` → directed graph, highway classification, one-way handling.
- Truck constraints: OSM tag filters + curated per-way layer (tightening only).
- `CachedMatrix` (Dijkstra all-pairs, per strategy) and opt-in contraction
  hierarchy (`ch`).
- Time-dependent curfews (`timed`) and corridor audit (`audit`).
- `TruckProfile` and `MatrixStrategy`.

Real-extract timing/memory are operator steps (see `docs/PLAN.md`).
