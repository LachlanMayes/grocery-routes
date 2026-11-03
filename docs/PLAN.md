# grocery-routes — plan

Route + delivery planning for Australian supermarket distribution: Coles,
Woolworths, ALDI and IGA/Metcash store networks. Rust, built from scratch as a
passion project; designed to hold up at 10+ trucks across states, executed
one-chain-one-metro at a time.

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

| Gate | When | Decides |
|---|---|---|
| Solomon C101 gap measurement | end of M1 | M2 quality target. No pre-committed "few %": first implementations typically land well above tuned SOTA (~1–2%); aim for ≤5%, then ≤3% only if the measured data supports it |
| Overpass truck-tag count (Sydney bbox; `maxweight`, `maxheight`, `hgv`) | M0 | M3 scope: OSM-tag-driven truck weights vs real distances + a curated truck-constraints file |
| Two-shift multi-day fixture (Sydney→Melbourne overnight relay) | M0 | Multi-day route state expressible now; a single day is the solver's degenerate case |
| `vrp_parser` cross-check (optional) | M1 | Default is a vendored ~50-line Solomon/VRPLib parser; the crate is a cross-check only |

## Workspace

| Crate | Role | Milestone |
|---|---|---|
| `crates/domain` | timeline model, banners/stores/vehicles/drivers, validation rules, multi-day fixture | M0 |
| `crates/sim` | seeded synthetic network generator (city × chain) | M0 |
| `crates/solver` | haversine matrix + deterministic greedy insertion (M0) → local search (M1) → LNS/ALNS (M2) | M0–M2 |
| `crates/routing` | OSM ingest → graph → cached matrices; truck layer = OSM-where-verified + curated constraints | M3 |
| `crates/geo-au` | G-NAF ingest + address index (own Rust loader; minus34 dumps as a shortcut) | later |
| `crates/cli` | `gen-sim` / `solve` / `export` (route sheet + GeoJSON) | M0 |
| `crates/api` | axum + SQLite; SSE for live ops | M4 |
| `apps/console` | egui desktop ops console (`walkers` map widget — active as of Oct 2026, PMTiles/vector support built in) | M4 |

## Milestones

- **M0 — Foundations** (1–2 weekends): workspace; domain model + validation;
  multi-day relay fixture test; sim generator; CLI `gen-sim`/`solve`/`export`;
  proptest invariants + criterion bench stub. Gates: relay fixture; Overpass count.
- **M1 — Solver v1** (2–4 weeks): Clarke-Wright + local search + time-window
  feasibility; bench harness (vendored parser); the C101 calibration gate sets
  the M2 target.
- **M2 — Solver v2** (2–6 weeks): LNS/ALNS, full cost objective, multi-temp /
  multi-depot / heterogeneous fleet; rayon multi-start; deterministic seeds;
  multi-day fixtures stay feasible.
- **M3 — Road reality** (4–8 weeks): OSM AU extract ingest (`osmpbf`) → own
  graph → cached matrices; truck constraints per the M0 gate; validate against
  hand-checked trips. `fast_paths` (contraction hierarchies) as an accelerator
  option; own CH as a later deep-dive.
- **M4 — Product shell**: axum API + SQLite; egui console — import, plan a day,
  view routes, KPIs, what-if re-solve.
- **M5 — Multi-day optimisation + AU/grocery depth**: relay/rest placement,
  NHVR-style fatigue rules, per-chain receiving rules, multi-timezone + state
  holidays (`jiff`), curfews, returnables ledger, Christmas-peak simulation.
- **M6 — Driver side (optional)**: route sheet + maps deep links first; live
  tracking, then proof of delivery.

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
  MSVC linker. `rustfmt` and `clippy` components are wired.
- Workspace lints: `unsafe_code = "forbid"`, `clippy::all` at warn level;
  the check commands run with `-D warnings`.
