# grocery-routes — plan

Route + delivery planning for Australian supermarket distribution: Coles,
Woolworths, ALDI and IGA/Metcash store networks. Rust, built from scratch as a
passion project; designed to hold up at 10+ trucks across states, executed
one-chain-one-metro at a time.

## Status (2026-11-05, updated)

M0–M4 are implemented and verified. M5 now includes NHVR fatigue rules,
chain receiving defaults, **horizon→local clock conversion** (`Problem.horizon_start`,
DST-aware via jiff) and **computed Australian national public holidays**
(Easter via Gregorian computus, weekend observance with the Christmas/Boxing
cascade, and the 2011-style coincident-holiday substitute; state/
territory-specific holidays are not modelled). Invalid IANA timezone names
are rejected by `Problem::validate_timezones` at the CLI/API boundaries
(not by `Solution::validate`, which solvers assert on). The route sheet renders local (arrive)
times when anchored, deep links every node to OpenStreetMap and flags
deliveries falling on a national holiday. M6 driver-side scope is settled:
the route sheet (with OSM deep links) is the deliverable — there is no live
tracking or proof of delivery, and no further console UI is planned for it.

Current checks:

- `cargo test --workspace`: all **166** tests pass.
- `cargo clippy --workspace --all-targets`: exits 0 with warnings (no errors).
- Latest commits: `4f6d16f` (review fixes), `75345aa` (LNS validity fix),
  `a11f6c8` (anchor/local time/holidays), `6d73dae` (curated constraints).
- `data/solomon/C101.txt` is tracked, so the calibration gates fail loudly
  rather than silently skipping if the dataset is missing.

Current C101 calibration output (2026-11-05):

- greedy: `routes=11, distance=841.68, served=100/100` — within the ≤5% gap gate.
- local search: `routes=11, distance=621.42, served=100/100`.
- LNS: `routes=11, distance=709.10, served=100/100` — **valid** (since
  `75345aa`; store-loss/acceptance bugs fixed). LNS is opt-in; local search
  remains the best measured quality path.

`docs/M1-REPORT.md` is current as of 2026-11-05.
`docs/TRUCK-CONSTRAINTS.md` states exactly what is enforced for heavy
vehicles and what is not (see "Known gaps").

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
| Solomon C101 gap measurement | end of M1 | Measured repeatedly; current run is greedy 841.68, local search 621.42 and LNS 709.10 with 100/100 served. The ≤5% gap gate (≤870.39) passes for all three; local search remains the best path. |
| Overpass truck-tag count (Sydney bbox; `maxweight`, `maxheight`, `hgv`) | M0 | PARTIAL, decision recorded: `maxweight` 5,612 (3.2% of drivable ways), `maxheight` 1,328 (0.8%), `hgv` 694 (0.4%). M3 uses OSM tags where present; curated way-specific constraints are supported by `routing::curated` but `constraints/truck-constraints.json` ships empty (no verified data) — see `docs/TRUCK-CONSTRAINTS.md`. |
| Two-shift multi-day fixture (Sydney→Melbourne overnight relay) | M0 | Complete — fixture passes in `crates/domain/tests/validation.rs`; multi-day horizons are the degenerate single-day case for the solver. |
| `vrp_parser` cross-check (optional) | M1 | Not adopted as a dependency; Solomon/VRPLib parser is vendored in `crates/solver/src/parser.rs`. |

## Workspace

| Crate | Role | Milestone | Status |
|---|---|---|---|
| `crates/domain` | Timeline model, chains/stores/vehicles/drivers, validation rules, multi-day fixture, horizon anchor + local-time conversion (`clock`), AU national holidays (`holidays`) | M0/M5 | Implemented; NHVR fatigue (Standard/Basic/Advanced), chain receiving defaults, DST-aware local times, computed national holidays (Easter computus + observed-date cascade), invalid IANA names rejected by validate. Curfews stored, not enforced (needs road-class edges + departure-time routing). |
| `crates/sim` | Seeded synthetic network generator (city × chain) | M0 | Implemented; generator and tests pass. `City::iana_timezone` stamps zones with `gen-sim --timezones`. |
| `crates/solver` | Haversine matrix + deterministic greedy insertion (M0) → local search (M1) → LNS/ALNS (M2) | M0–M2 | Implemented; greedy default, local search and LNS pass the C101 gate (841.68 / 621.42 / 709.10). LNS is opt-in; invalid incumbents are rejected during search. |
| `crates/routing` | OSM PBF ingest → directed graph → truck tag filtering → cached matrices; curated per-way constraints | M3 | Implemented for ingest/filter/cache; truck filtering honors OSM `hgv`/`access`/weight/height; curated tightening via `routing::curated` (empty data file); no `fast_paths`/contraction hierarchy yet. |
| `crates/geo-au` | G-NAF ingest + address index | later | Not started. |
| `crates/cli` | `gen-sim` (`--timezones`, `--horizon-start`) / `solve` (`--ls`/`--osm`/`--constraints`/truck flags) / `export` (route sheet + GeoJSON with OSM deep links + local times + holiday footnote) | M0/M3 | Implemented; C101 dataset is tracked. |
| `crates/api` | axum + SQLite; SSE events; import/solve/solution/KPIs | M4 | Implemented; endpoint integration tests pass; validate imports; request body limit; OSM cache per path; API serves live events. |
| `apps/console` | egui desktop ops console | M4 | Implemented operator console; Tokio runtime on background thread, single SSE connection with backoff; no live tracking / proof-of-delivery integration (M6). No `walkers` map/PMTiles work is shipped. |

## Milestones

- **M0 — Foundations** — **done**. Workspace, domain model + validation,
  Sydney→Melbourne relay fixture, sim generator, CLI and export are in place.
  Gates: relay fixture passes; Overpass truck-tag count was only partially
  gathered (see the gate table).
- **M1 — Solver v1** — **done**. Vendored Solomon/VRPLib parser, C101 gate wired,
  greedy and local search validated, C101 dataset tracked
  (`data/solomon/C101.txt`). [docs/M1-REPORT.md](M1-REPORT.md) is current as of
  2026-11-05 and retains the superseded original result for history.
- **M2 — Solver v2** — **done sufficiently for the current quality gate**.
  Local search passes C101 at 621.42 with 100/100 served. LNS/ALNS was
  fixed on 2026-11-05 (`75345aa`): ruin operators removed steps by stale
  positions and could delete a route's end depot; candidates that did not
  serve every store exactly once are now rejected during search, and LNS
  passes the gate at 709.10. LNS is opt-in, not the default. `rayon`
  multi-start is not implemented.
- **M3 — Road reality** — **done, with documented gaps**. `osmpbf` → graph →
  CachedMatrix, directed edges, `oneway=-1`, weight/height/HGV/access
  filters, all-pairs Dijkstra, CLI/API OSM flags, and a curated
  per-way constraints layer (`routing::curated`, tightening only,
  provenance-required file — ships empty; see
  [TRUCK-CONSTRAINTS.md](TRUCK-CONSTRAINTS.md)). Missing:
  `fast_paths`/contraction hierarchy; corridor legality against real road
  limits is unverified (no OSM extract in-repo; verification is an
  operator responsibility).
- **M4 — Product shell** — **done**. axum + SQLite endpoints for import,
  solve, solutions and KPIs; SSE solve-event broadcast; endpoint integration
  tests; import validation; request body limit; OSM cache per canonical path.
  The egui console covers import/solve/solutions/KPIs/events; it is not a full
  interactive what-if modelling tool.
- **M5 — Multi-day optimisation + AU/grocery depth** — **mostly done, named
  items outstanding**. NHVR fatigue rules (Standard/Basic/Advanced),
  per-chain receiving defaults and multi-day validation are implemented.
  **Horizon→local clock conversion is implemented** (`Problem.horizon_start`,
  DST-aware `clock` module; local (arrive) times in the route sheet), and
  **Australian national public holidays are computed** (`holidays` module:
  Easter computus, weekend observance with the Christmas/Boxing cascade,
  2011-style coincident-holiday substitute). Not implemented: relay/rest
  placement, state/territory-specific holidays, Christmas-peak simulation,
  returnables ledger, curated curfews or curfew enforcement (routing is not
  departure-time aware).
- **M6 — Driver side** — **decision made (2026-11-05): route sheet + OSM
  deep links is the M6 deliverable**. The export deep links every node to
  OpenStreetMap (no API key), renders local times when the problem is
  anchored and flags national holidays in the plan. No live tracking, no
  proof of delivery, no driver app, no further console surface is planned.
  If interactive driver UX is ever wanted, it reopens as an explicit
  milestone, not as polish.

## Known gaps and next priorities

1. **Corridor legality is unverified.** OSM tag filtering and the curated
   layer are in place, but no OSM extract lives in this repository and no
   route has been checked against bridge/load limits. Before any real
   dispatch, an operator must verify corridors per
   [TRUCK-CONSTRAINTS.md](TRUCK-CONSTRAINTS.md) (or stay haversine-only and
   treat outputs as advisory).
2. **Enforce curfews** once routing is departure-time aware; until then
   `Problem::curfews` remains data-only. The local weekday of any horizon
   minute can now be resolved via the anchor, so the missing piece is
   time-dependent path-finding, not the calendar.
3. **State/territory-specific holidays** (Melbourne Cup, variable King's
   Birthday dates, Grand Final Friday). The national calendar is computed;
   per-jurisdiction data would need a maintained, sourced table.
4. **Christmas-peak simulation** and **returnables ledger** — the two
   remaining named M5 features; not started.
5. **`fast_paths`/contraction hierarchy** for OSM matrix build time;
   matrices are still all-pairs Dijkstra.
6. **LNS is opt-in and not better than local search** (709.10 vs 621.42 on
   C101). Keep LS as the quality path; revisit LNS only with a measured
   win. `rayon` multi-start remains unimplemented.
7. **G-NAF geocoding** is not started; addresses remain synthetic.

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
