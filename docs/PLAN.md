# grocery-routes — plan

Route + delivery planning for Australian supermarket distribution: Coles,
Woolworths, ALDI and IGA/Metcash store networks. Rust, built from scratch as a
passion project; designed to hold up at 10+ trucks across states, executed
one-chain-one-metro at a time.

## Status (2026-11-06)

M0–M4 are implemented and verified. The seven gaps listed in the previous
status were closed in software (commits `3b71be7` … `95eb982`):

- **Curfew enforcement** — `routing::timed` computes time-dependent legs
  (a truck waits at the previous node until a traversal is clear of every
  window) and `solver::curfew` re-schedules solved plans, dropping stores
  whose waits no longer fit and reporting every breach. CLI:
  `solve --enforce-curfews --network-timezone <IANA>`. This is a
  *feasibility restore*, not a curfew-aware optimiser: routes are built on
  the static matrix, then made legal.
- **State/territory holidays** — `domain::holidays::state_holidays_in_year`
  builds full calendars for all eight jurisdictions from computed rules,
  with a sourced table for the irregular AFL Grand Final Friday (2026–2027;
  other years omit it rather than guess) and jurisdiction-specific ANZAC
  substitution. CLI: `holidays --year Y [--state VIC]`.
- **Christmas-peak profile** — `gen-sim --christmas-peak` (illustrative
  demand uplift and wider receiving windows; fixture data).
- **Returnables ledger** — `domain::ledger` balances cages issued against
  empties collected per banner; the route sheet gains a ledger section and
  `export` writes `returnables.csv`.
- **Contraction hierarchy** — `routing::ch` wraps `fast_paths`;
  `solve --ch` builds matrices time-optimally once prepared. Measured
  (debug, synthetic 15×15 grid, 50,625 ordered pairs): Dijkstra all-pairs
  22.08 s vs CH prepare 15 ms + queries 0.68 s (~30×). CH is opt-in.
- **LNS multi-start** — `solve --ls --repeat N` runs N seeded searches in
  parallel and keeps the best. C101: 679.85 with 8 runs × 120 s versus
  709.10 for a single run; local search (621.42) remains the best measured
  path.
- **G-NAF geocoder** — new `crates/geo-au`: flat-file ingest → SQLite,
  `geocode`/`reverse`; CLI `import-gnaf`, `geocode`, `reverse`. Validated
  against synthetic fixtures only; see [GNAF.md](GNAF.md).
- **Corridor audit** — CLI `audit` lists every OSM way a plan traverses and
  flags the ones with no truck-limit data. Legal verification of corridors
  remains an operator responsibility.

M6 driver-side scope is unchanged: the route sheet (with OSM deep links,
local times and holiday flags) is the deliverable — no live tracking, no
proof of delivery, no further console UI.

Current checks:

- `cargo test --workspace`: all **241** tests pass.
- `cargo clippy --workspace --all-targets`: exits 0 with warnings (no errors).
- Latest commits: `0483981` (quality + review pass: complexity refactors,
  shared helpers, strategy-aware audit), `d8cb599` (review fixes), `52982d9`
  (docs refresh), `95eb982` (G-NAF), `2905f93` (corridor audit), `86a5321`
  (contraction hierarchy), `200d446` (LNS multi-start), `db79960` (state
  holidays), `1cd22cd` (Christmas peak + returnables), `3b71be7` (curfew
  enforcement).
- `data/solomon/C101.txt` is tracked, so the calibration gates fail loudly
  rather than silently skipping if the dataset is missing.

Current C101 calibration output (2026-11-06):

- greedy: `routes=11, distance=841.68, served=100/100` — within the ≤5% gap gate.
- local search: `routes=11, distance=621.42, served=100/100` — best measured.
- LNS (single run): `routes=11, distance=709.10, served=100/100` — valid.
- LNS ×8: 679.85 at 120 s/run; 714.74 at 30 s/run (wall-clock bounded).
  Multi-start helps LNS but does not beat local search.

`docs/M1-REPORT.md` is current as of 2026-11-06.
`docs/TRUCK-CONSTRAINTS.md` states exactly what is enforced for heavy
vehicles and what is not; `docs/GNAF.md` covers the geocoder.

## Why this shape (cost/time)

- $0 in licences. Open data only (OpenStreetMap, G-NAF). Infra free locally,
  ~AU$10–40/month for a VPS when hosted.
- 100% Rust workspace; egui desktop console; no browser or JS required for the
  operator UI.
- The solver, the routing engine and the geocoder are the parts built from
  scratch; crates cover the commodity layers (serde, clap, axum/egui,
  `osmpbf`, `fast_paths`, rusqlite). Data (OSM extracts, G-NAF) and infra
  (SQLite, Docker, VPS) are consumed, not reinvented.
- Optional "referee only" tooling: VROOM / `vrp-cli` as external benchmarks to
  score our solver against — never a runtime dependency.

## Calibration gates (outcomes decide scope, not assumptions)

| Gate | When | Outcome / decision |
|---|---|---|
| Solomon C101 gap measurement | end of M1 | Measured repeatedly; current: greedy 841.68, local search 621.42, LNS 709.10 (single) / 679.85 (×8). The ≤5% gap gate (≤870.39) passes; local search remains the best path. |
| Overpass truck-tag count (Sydney bbox; `maxweight`, `maxheight`, `hgv`) | M0 | PARTIAL, decision recorded: `maxweight` 5,612 (3.2% of drivable ways), `maxheight` 1,328 (0.8%), `hgv` 694 (0.4%). M3 uses OSM tags where present; curated way-specific limits via `routing::curated` (ships empty — no verified data); `audit` reports which corridors lack limit data. See `docs/TRUCK-CONSTRAINTS.md`. |
| Two-shift multi-day fixture (Sydney→Melbourne overnight relay) | M0 | Complete — fixture passes in `crates/domain/tests/validation.rs`; multi-day horizons are the degenerate single-day case for the solver. Solver-side relay/rest *placement* remains unimplemented (see below). |
| `vrp_parser` cross-check (optional) | M1 | Not adopted as a dependency; Solomon/VRPLib parser is vendored in `crates/solver/src/parser.rs`. |
| Contraction hierarchy measured win (all-pairs build) | M3 gap | Passed on a synthetic grid in the debug profile (~30×); opt-in via `solve --ch`. Real-extract timing is an operator measurement. |
| LNS multi-start measured win over single-run LNS | M2 gap | Passed (679.85 vs 709.10 on C101) but not over local search; LNS stays opt-in. |

## Workspace

| Crate | Role | Milestone | Status |
|---|---|---|---|
| `crates/domain` | Timeline model, chains/stores/vehicles/drivers, validation rules, multi-day fixture, horizon anchor + local-time conversion (`clock`), national + state holidays (`holidays`), returnables ledger (`ledger`) | M0/M5 | Implemented; NHVR fatigue rules (between-shift rest and work caps), chain receiving defaults and returnables fixtures, DST-aware local times, national and per-jurisdiction holiday calendars, invalid IANA names rejected via `Problem::validate_timezones`. In-shift break placement is not modelled. |
| `crates/sim` | Seeded synthetic network generator (city × chain), Christmas-peak profile | M0 | Implemented; `City::iana_timezone` with `gen-sim --timezones`; `--christmas-peak` uplift/windows are illustrative fixtures. |
| `crates/solver` | Haversine/road matrices + deterministic greedy insertion → local search → LNS/ALNS, multi-start, curfew re-scheduling | M0–M2 | Implemented; greedy default; LS 621.42 / LNS 709.10 single / 679.85 ×8 on C101; `--repeat N` multi-start; curfew enforcement re-schedules and reports drops. |
| `crates/routing` | OSM PBF ingest → directed graph → truck tag filtering → matrices; curated limits; contraction hierarchy; time-dependent curfews; corridor audit | M3 | Implemented; Dijkstra and CH strategies; `routing::timed` curfew waiting; `routing::audit` corridor report. No real extract ships in-repo; corridor legality stays human-verified. |
| `crates/geo-au` | G-NAF flat-file ingest → SQLite; geocode + reverse | later | Implemented and fixture-tested; not yet validated against a real G-NAF release (operator step, see `docs/GNAF.md`). |
| `crates/cli` | `gen-sim` / `solve` / `export` plus `holidays`, `audit`, `import-gnaf`, `geocode`, `reverse` | M0/M3 | Implemented; C101 dataset is tracked. |
| `crates/api` | axum + SQLite; SSE events; import/solve/solution/KPIs | M4 | Implemented; endpoint integration tests pass; OSM matrices default to Dijkstra (CH is CLI-only for now). |
| `apps/console` | egui desktop ops console | M4 | Implemented operator console; no live tracking / proof-of-delivery integration (M6). No `walkers` map/PMTiles work is shipped. |

## Milestones

- **M0 — Foundations** — **done**. Workspace, domain model + validation,
  Sydney→Melbourne relay fixture, sim generator, CLI and export are in place.
- **M1 — Solver v1** — **done**. Vendored Solomon/VRPLib parser, C101 gate
  wired, greedy and local search validated, C101 dataset tracked.
  [docs/M1-REPORT.md](M1-REPORT.md) is current as of 2026-11-06.
- **M2 — Solver v2** — **done**. LNS validity fixed (`75345aa`); multi-start
  added and measured (`200d446`); local search remains the quality path and
  LNS stays opt-in. `rayon` was not added — scoped threads are used.
- **M3 — Road reality** — **done in software, with operator-verified data
  outstanding**. `osmpbf` → graph → CachedMatrix, directed edges,
  weight/height/HGV/access filters, Dijkstra **and** contraction
  hierarchies, curated per-way constraints, time-dependent curfew
  enforcement, and the corridor-audit report. What remains is data, not
  code: a real OSM extract and human verification of corridor legality.
- **M4 — Product shell** — **done**. axum + SQLite endpoints, SSE solve
  events, import validation, request body limit, OSM cache per path; egui
  console covers import/solve/solutions/KPIs/events.
- **M5 — Multi-day optimisation + AU/grocery depth** — **done except
  solver-side relay/rest placement**. NHVR fatigue rules (between-shift
  rest, work caps), per-chain receiving defaults, horizon→local clock,
  national + state/territory holidays, Christmas-peak profile, returnables
  ledger and curfew enforcement are all implemented. **Relay/rest
  placement** (multi-driver hand-overs mid-route, in-shift breaks) is the
  one named item deliberately left: it needs a scheduling-model change in
  the greedy/LNS core, is not required by any current gate, and is scoped
  here as future work rather than half-built.
- **M6 — Driver side** — **decision made (2026-11-05)**: route sheet + OSM
  deep links is the deliverable; no live tracking, proof of delivery,
  driver app or further console surface.

## Known gaps and next priorities

1. **Corridor legality needs human verification.** The `audit` command
   reports every traversed way and which ones carry no truck-limit data;
   `constraints/truck-constraints.json` stays empty until someone verifies
   real limits. Until then, plans through un-annotated ways are not legally
   verified (see [TRUCK-CONSTRAINTS.md](TRUCK-CONSTRAINTS.md)).
2. **Curfew enforcement is a post-solve feasibility restore.** Route order
   and fleet assignment are fixed; waits and drops are the only remedies.
   A curfew-aware optimiser (time-dependent costs inside the search) would
   need the scheduling model to take leg providers, which is the same
   change relay placement needs.
3. **Relay/rest placement** (M5, deliberately not built): multi-driver
   hand-overs and in-shift break rules. The model and validator already
   represent the outcomes; the solver does not yet produce them.
4. **State holiday irregulars**: the AFL Grand Final Friday is in the
   sourced table only for 2026–2027 (other years omit it); regional show
   days and part-day holidays are out of scope; every authority's own
   calendar is authoritative.
5. **G-NAF**: validated against synthetic fixtures, not a real release;
   queries are `LIKE` scans with no FTS/R-tree; wiring geocoding into store
   import is not started.
6. **CH memory/timing on a real extract is unmeasured**; the hierarchy is
   opt-in and documented as such. `CachedMatrix` caches per strategy.
7. **LNS remains behind local search** on C101 (679.85 vs 621.42 even with
   multi-start); keep LS as the quality path, revisit only with a measured
   win.

## Data & licences

- **OpenStreetMap**: ODbL — attribution required.
- **G-NAF** (Geoscape, via data.gov.au): free under the Open G-NAF EULA
  (CC BY 4.0-based; the restriction concerns using addresses to *send mail*,
  which is irrelevant to routing). ~5 GB unpacked, quarterly releases. The
  importer ships; the data does not (see [GNAF.md](GNAF.md)).
- Store addresses in this repo are **synthetic**. Real chain store lists are
  not scraped into anything public; a small hand-curated set may be kept
  locally.
- All simulated operating parameters (receiving windows, service times, costs,
  fleet mix, peak multipliers, returnables counts, curfew windows) are
  **illustrative fixtures**, not researched operator data.

## Environment notes

- The repo pins `stable-x86_64-pc-windows-gnu` because this machine has no
  MSVC linker. `rustfmt` and `clippy` components are wired; the local
  toolchain reports Rust 1.99.1.
- Workspace lints: `unsafe_code = "forbid"`; `clippy::all` at warn level.
- Latest local verification (2026-11-06): `cargo test --workspace` passes
  (241 tests); `cargo clippy --workspace --all-targets` exits 0 with existing
  warnings (not `-D warnings` clean).
- `Cargo.lock` is checked in and is the source of truth for dependency
  resolution.
