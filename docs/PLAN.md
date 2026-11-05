# grocery-routes — plan

Route + delivery planning for Australian supermarket distribution: Coles,
Woolworths, ALDI and IGA/Metcash store networks. Rust, built from scratch as a
passion project; designed to hold up at 10+ trucks across states, executed
one-chain-one-metro at a time.

## Status (2026-11-05)

M0–M4 are implemented and verified. M5 has the core AU/grocery depth items
(fatigue rules, chain receiving defaults, stored timezone/curfew fields) but
timezone conversion and curfew enforcement are not wired into the solver or
validator. M6 as originally scoped (live tracking / proof of delivery) is not
implemented; the delivered operator console is the egui desktop UI with import,
solve, solutions, KPIs and SSE events.

Current checks:

- `cargo test --workspace`: all 131 tests pass.
- `cargo clippy --workspace --all-targets`: exits 0 with warnings (no errors).
- Last review-fix commit: `4f6d16f`.
- `data/solomon/C101.txt` is now tracked, so the calibration gates fail loudly
  rather than silently skipping if the dataset is missing.

Current C101 calibration output (2026-11-05):

- greedy: `routes=11, distance=841.68, served=100/100` — within the ≤5% gap gate.
- local search: `routes=11, distance=621.42, served=100/100`.
- LNS prints an invalid result and falls back to greedy (`841.68`); it remains
  opt-in and should not be treated as the calibrated production default.

`docs/M1-REPORT.md` is stale and records the original 7/100 greedy result; the
status above is the measured current state.

## Why this shape (cost/time)

- $0 in licences. Open data only (OpenStreetMap, G-NAF). Infra free locally,
  ~AU$10–40/month for a VPS when hosted.
- 100% Rust workspace; egui desktop console; no browser or JS required for the
  operator UI.
- The solver, the routing engine and the geocoder are the parts built from
  scratch; crates cover the commodity layers (serde, clap, later axum/egui,
  osmpbf). Data (OSM extracts, G-NAF) and infra (SQLite, Docker, VPS) are
  consumed, not reinvented.
- Optional "referee only" tooling: VROOM / `vrp-cli` as external benchmarks to
  score our solver against — never a runtime dependency.

## Calibration gates (outcomes decide scope, not assumptions)

| Gate | When | Outcome / decision |
|---|---|---|
| Solomon C101 gap measurement | end of M1 | Measured repeatedly; current run is greedy 841.68 with 100/100 served, local search 621.42. The ≤5% gap gate (≤870.39) passes. LNS currently falls back to greedy and is not the default. |
| Overpass truck-tag count (Sydney bbox; `maxweight`, `maxheight`, `hgv`) | M0 | PARTIAL, decision recorded: `maxweight` 5,612 (3.2% of drivable ways), `maxheight` 1,328 (0.8%), `hgv` 694 (0.4%). M3 uses OSM tags where present; a curated truck-constraints file/hand-checked validation is not shipped yet. |
| Two-shift multi-day fixture (Sydney→Melbourne overnight relay) | M0 | Complete — fixture passes in `crates/domain/tests/validation.rs`; multi-day horizons are the degenerate single-day case for the solver. |
| `vrp_parser` cross-check (optional) | M1 | Not adopted as a dependency; Solomon/VRPLib parser is vendored in `crates/solver/src/parser.rs`. |

## Workspace

| Crate | Role | Milestone | Status |
|---|---|---|---|
| `crates/domain` | Timeline model, chains/stores/vehicles/drivers, validation rules, multi-day fixture | M0/M5 | Implemented; NHVR fatigue (Standard/Basic/Advanced), chain receiving defaults and local validation tests pass. `timezone`/`curfews` are stored but not applied/enforced. |
| `crates/sim` | Seeded synthetic network generator (city × chain) | M0 | Implemented; generator and tests pass. |
| `crates/solver` | Haversine matrix + deterministic greedy insertion (M0) → local search (M1) → LNS/ALNS (M2) | M0–M2 | Implemented; greedy default, local search passes C101 gate; LNS is opt-in and currently falls back to greedy in calibration. |
| `crates/routing` | OSM PBF ingest → directed graph → truck tag filtering → cached matrices | M3 | Implemented for ingest/filter/cache; truck filtering now honors OSM `hgv`, `access`, weight/height; no `fast_paths`/contraction hierarchy yet. |
| `crates/geo-au` | G-NAF ingest + address index | later | Not started. |
| `crates/cli` | `gen-sim` / `solve` / `export` (route sheet + GeoJSON); `--ls`/`--osm`/truck flags | M0/M3 | Implemented; C101 dataset is tracked. |
| `crates/api` | axum + SQLite; SSE events; import/solve/solution/KPIs | M4 | Implemented; endpoint integration tests pass; validate imports; request body limit; OSM cache per path; API serves live events. |
| `apps/console` | egui desktop ops console | M4 | Implemented operator console; Tokio runtime on background thread, single SSE connection with backoff; no live tracking / proof-of-delivery integration (M6). No `walkers` map/PMTiles work is shipped. |

## Milestones

- **M0 — Foundations** — **done**. Workspace, domain model + validation,
  Sydney→Melbourne relay fixture, sim generator, CLI and export are in place.
  Gates: relay fixture passes; Overpass truck-tag count was only partially
  gathered (see the gate table).
- **M1 — Solver v1** — **done**. Vendored Solomon/VRPLib parser, C101 gate wired,
  greedy and local search validated, C101 dataset tracked
  (`data/solomon/C101.txt`). `docs/M1-REPORT.md` is stale; use the status
  numbers at the top of this plan.
- **M2 — Solver v2** — **done sufficiently for the current quality gate, with
  caveats**. Local search is deterministic enough to pass C101 at 621.42 with
  100/100 served. LNS/ALNS and deterministic seeds are implemented, but LNS
  currently produces an invalid plan and falls back to greedy in calibration;
  it is not the default. `rayon` multi-start is not implemented.
- **M3 — Road reality** — **done, with gaps**. `osmpbf` → graph → CachedMatrix,
  directed edges, `oneway=-1`, weight/height/HGV/access filters, all-pairs
  Dijkstra and CLI/API OSM flags. Missing: `fast_paths`/contraction hierarchy
  and no curated constraint file or recorded hand-checked trips.
- **M4 — Product shell** — **done**. axum + SQLite endpoints for import,
  solve, solutions and KPIs; SSE solve-event broadcast; endpoint integration
  tests; import validation; request body limit; OSM cache per canonical path.
  The egui console covers import/solve/solutions/KPIs/events; it is not a full
  interactive what-if modelling tool.
- **M5 — Multi-day optimisation + AU/grocery depth** — **partial**. NHVR
  fatigue rules (Standard/Basic/Advanced), per-chain receiving defaults and
  multi-day validation are implemented. Timezone and curfew fields are stored
  but never applied/enforced. Not implemented: multi-timezone conversion,
  state holidays, relay/rest placement, curated curfews, returnables ledger,
  Christmas-peak simulation.
- **M6 — Driver side (optional)** — **not started**. The CLI has route-sheet /
  GeoJSON export, but there is no live tracking or proof-of-delivery flow.

## Known gaps and next priorities

1. Implement horizon-relative → local clock conversion for `Depot.timezone` and
   `Store.timezone`, or keep them explicitly data-only everywhere outside the
   domain crate.
2. Enforce curfews in routing/validation once road segments carry a road class
   and timestamps have an absolute clock anchor; otherwise keep them marked as
   stored-only.
3. Replace the parameter-less LNS path with either a correct, valid solver
   calibration or a smaller LNS that always returns a validated solution; add
   optional `rayon` multi-start if needed.
4. Build the curated truck-constraints layer (or document precisely that OSM
   tags are the only constraints) and hand-check a small set of trips.
5. Complete M5 gaps chosen for the next milestone: timezone/holidays, optional
   curated curfews, returnables ledger, or Christmas-peak simulation.
6. Decide whether to invest in M6 driver-side UX; otherwise treat export as the
   driver-side deliverable and stop adding UI surface.

## Data & licences

- **OpenStreetMap**: ODbL — attribution required.
- **G-NAF** (Geoscape, via data.gov.au): free under the Open G-NAF EULA
  (CC BY 4.0-based; the restriction concerns using addresses to *send mail*,
  which is irrelevant to routing). ~5 GB unpacked, quarterly releases.
- Store addresses in this repo are **synthetic**. Real chain store lists are
  not scraped into anything public; a small hand-curated set may be kept
  locally.
- All simulated operating parameters (receiving windows, service times, costs,
  fleet mix) are **illustrative fixtures**, not researched operator data.

## Environment notes

- The repo pins `stable-x86_64-pc-windows-gnu` because this machine has no
  MSVC linker. `rustfmt` and `clippy` components are wired; the local
  toolchain reports Rust 1.99.1.
- Workspace lints: `unsafe_code = "forbid"`; `clippy::all` at warn level.
- Latest local verification (2026-11-05): `cargo test --workspace` passes;
  `cargo clippy --workspace --all-targets` exits 0 with existing warnings (not
  `-D warnings` clean).
- `Cargo.lock` is checked in and is the source of truth for dependency
  resolution.
