# grocery-routes — plan

Route + delivery planning for Australian supermarket distribution: Coles,
Woolworths, ALDI and IGA/Metcash store networks. Rust, built from scratch as a
passion project; designed to hold up at 10+ trucks across states, executed
one-chain-one-metro at a time.

## Status (2026-11-09)

M0–M4 are implemented and verified. The seven gaps listed in the previous
status were closed in software (commits `98a3f96` … `aded0ee`), and the
next-sprint plan (tasks 1–6) is also closed: truck-constraints tooling
(`03b984d`), relay/rest post-pass (`b17b5a0`), curfew-aware construction
(`e5a8130`), and the LNS-vs-LS harder benchmark (`a263534`).

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

- `cargo test --workspace`: all **254** tests pass (2026-11-09).
- `cargo clippy --workspace --all-targets`: exits 0 with **zero warnings**
  (whole-tree sweep, 2026-11-06).
- Latest commits: `fc88c9b` (console: eframe/egui 0.37 upgrade, glow
  renderer), `decd8cd` (dependency-audit record), `91f9abf` (maintenance:
  clippy sweep 91 -> 0, dependency patch updates), `497ac2b` (quality +
  review pass: complexity refactors, shared helpers, strategy-aware audit),
  `ce8039e` (review fixes), `4445fc0` (docs refresh), `aded0ee` (G-NAF),
  `37e5925` (corridor audit), `c9eaa8f` (contraction hierarchy), `2b88822`
  (LNS multi-start), `a9f5a75` (state holidays), `8f11f8a` (Christmas peak
  + returnables), `98a3f96` (curfew enforcement), `03b984d` (truck
  constraints tooling), `b17b5a0` (relay post-pass), `e5a8130`
  (curfew-aware construction), `a263534` (LNS-vs-LS harder benchmark).
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
| `apps/console` | egui desktop ops console | M4 | Implemented operator console (eframe 0.37, glow renderer), backed by the API server binary (`cargo run -p api`); no live tracking / proof-of-delivery integration (M6). No `walkers` map/PMTiles work is shipped. |

## Milestones

- **M0 — Foundations** — **done**. Workspace, domain model + validation,
  Sydney→Melbourne relay fixture, sim generator, CLI and export are in place.
- **M1 — Solver v1** — **done**. Vendored Solomon/VRPLib parser, C101 gate
  wired, greedy and local search validated, C101 dataset tracked.
  [docs/M1-REPORT.md](M1-REPORT.md) is current as of 2026-11-06.
- **M2 — Solver v2** — **done**. LNS validity fixed (`fbca088`); multi-start
  added and measured (`2b88822`); local search remains the quality path and
  LNS stays opt-in. `rayon` was not added — scoped threads are used.
- **M3 — Road reality** — **done in software; CH timing now measured on a
  real extract** (NSW, 258 MB, 4.89 M road nodes: CH 126.8 s vs Dijkstra
  205.9 s). Corridor legality still needs human verification of truck
  limits (data, not code).
- **M4 — Product shell** — **done**. axum + SQLite endpoints, SSE solve
  events, import validation, request body limit, OSM cache per path; egui
  console covers import/solve/solutions/KPIs/events.
- **M5 — Multi-day optimisation + AU/grocery depth** — **done**. NHVR fatigue rules (between-shift
  rest, work caps), per-chain receiving defaults, horizon→local clock,
  national + state/territory holidays, Christmas-peak profile, returnables
  ledger, curfew enforcement, and relay/rest placement (post-pass over
  (driver, shift) pairs with backtracking, `--relay`, `solver::relay`) are
  all implemented. Curfew-aware *construction* now also exists
  (`solver::curfew_aware`, `--curfew-aware`); LS/LNS improvement phases
  still operate on the static matrix.
- **M6 — Driver side** — **decision made (2026-11-05)**: route sheet + OSM
  deep links is the deliverable; no live tracking, proof of delivery,
  driver app or further console surface.

## Known gaps and next priorities

1. **Corridor legality needs human verification.** The `audit` command
   reports every traversed way and which ones carry no truck-limit data;
   `constraints/truck-constraints.json` stays empty until someone verifies
   real limits, but the tooling now exists: `constraints --file <json>
   [--sqlite <db>]` validates a curated file and exports
   `curated_entries`/`curated_entry` tables (`03b984d`). Until then, plans
   through un-annotated ways are not legally verified (see
   [TRUCK-CONSTRAINTS.md](TRUCK-CONSTRAINTS.md)).
2. **Curfew enforcement has two modes now.** `--enforce-curfews` is still a
   post-solve feasibility restore (route order and fleet assignment fixed;
   waits and drops are the only remedies). `--curfew-aware` builds routes
   through the `Legs` provider during construction (`solver::curfew_aware`,
   `solve_greedy_with_curfews`), so windows are respected at insertion time.
   The LS/LNS improvement phases still operate on the static matrix, so
   curfew legality there is restored, not optimised.
3. **Relay/rest placement — shipped as a post-pass** (`b17b5a0`):
   `solver::relay` with `schedule_relay_route` backtracking over
   (driver, shift) pairs, `apply_relay`/`apply_relay_with_matrix`,
   `solve_greedy_with_relay`, CLI `--relay`. It keeps the original route
   on any failure, so it can never block a feasible plan. LNS-integrated
   relay (hand-overs inside the search) is not built, by design.
4. **State holiday irregulars**: the AFL Grand Final Friday is in the
   sourced table only for 2026–2027 (other years omit it); regional show
   days and part-day holidays are out of scope; every authority's own
   calendar is authoritative.
5. **G-NAF**: validated against the Aug 2026 release (16,970,406 addresses,
   0 skipped); `geocode`/`reverse` return sensible results. Store import now
   geocodes addresses via the `import-stores` CLI command. Queries are `LIKE`
   scans with no FTS/R-tree.
6. **CH memory/timing on a real extract — measured** (2026-11-09): NSW
   extract (258 MB, 4.89 M road nodes after filtering). CH solve 126.8 s
   vs Dijkstra 205.9 s (~1.6×) on an 8-store problem; 18/100 matrix
   entries unreachable. The full Australia extract (928 MB) is not
   tractable interactively in debug mode; use state-level extracts. The
   hierarchy stays opt-in. Ingest now handles dense nodes and restricts
   the graph to nodes on drivable ways (`osm_ingest.rs`).
7. **LNS decision closed** (`a263534`): harder 200-customer benchmark
   (`crates/solver/tests/harder_benchmark.rs`) measured LS 1372.06/187
   served vs LNS 1827.32/136 served — LS wins by ~25%. LS stays the
   quality path, LNS stays opt-in. No solver change; the condition in
   `docs/M1-REPORT.md` ("Harder benchmark") is recorded there.
8. **Dependency advisories**: `cargo audit` exits 0. One advisory is ignored
   with a dated reason in [SECURITY.md](SECURITY.md): rsa, which no
   workspace target reaches and upstream never fixed. The eframe/egui 0.37
   upgrade (2026-11-07; the console builds eframe with the glow renderer —
   see the workspace `Cargo.toml`) cleared the quick-xml advisories.
   Remaining informational warnings: the zbus family on the Linux-only
   accessibility path, and `memmap2` 0.5.10 (unsound) via `osmpbf` 0.3.8,
   which has no fixed release upstream yet.

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
- Latest local verification (2026-11-07): `cargo test --workspace` passes
  (239 tests); `cargo clippy --workspace --all-targets` exits 0 with **zero
  warnings**; `cargo audit` exits 0 — see [SECURITY.md](SECURITY.md); the
  console starts and renders under eframe 0.37 (glow) in a manual smoke
  run. `-D warnings` is still not enforced.
- `Cargo.lock` is checked in and is the source of truth for dependency
  resolution.
