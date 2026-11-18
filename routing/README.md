# `routing` — OSM ingest, graph, matrices, truck constraints, CH, curfews, audit

- `osmpbf` → directed graph, highway classification, one-way handling.
- Truck constraints: OSM tag filters + curated per-way layer (tightening only).
- `CachedMatrix` (Dijkstra all-pairs, per strategy) and opt-in contraction
  hierarchy (`ch`).
- Time-dependent curfews (`timed`) and corridor audit (`audit`).
- `TruckProfile` and `MatrixStrategy`.

Measured on the NSW extract (258 MB, 4.89 M road nodes: CH 126.8 s vs Dijkstra 205.9 s); see `docs/PLAN.md`.
