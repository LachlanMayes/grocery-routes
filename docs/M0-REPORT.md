# M0 report — foundations

Date: 2026-11-03. Scope and gates per [PLAN.md](PLAN.md). Every measurement below
was produced by the command shown next to it; anything not yet built is listed
under "Not done (by design)".

> Historical snapshot, not a current verification report. Test counts and lint
> claims reflect 2026-11-03; the Solomon C101 rows were corrected on 2026-11-05
> (see [M1-REPORT.md](M1-REPORT.md) and [PLAN.md](PLAN.md)).

## What exists

| Crate | Contents |
|---|---|
| `domain` | Timeline model (`Minutes` from horizon start, multi-day capable), banners/stores/depots/vehicles/drivers, per-temperature `Load`, receiving windows, `CostModel` (per-route fixed, per-km, per-hour, reserved lateness penalty), `Solution`/`RoutePlan`, and `Solution::validate` — 19 violation kinds covering structure, capacity, windows, driver-shift coverage (single-shift and union coverage for depot hand-over dwells) and driver occupancy |
| `sim` | Seeded generator: five capitals, four banners, up to four suburb-scale store clusters, fringe depots, illustrative per-banner receiving windows, three vehicle classes, one driver per vehicle; counts clamped to ≥ 1; byte-identical output per seed |
| `solver` | Haversine distance matrix, deterministic greedy insertion (cheapest feasible insertion, earliest-deadline store order, explicit tie-breaks), full-route rescheduling on every candidate, single driver+shift per route (documented M0 limitation), cost computed from `CostModel` |
| `cli` | `grocery-routes` binary: `gen-sim` / `solve` / `export`; solve re-validates and fails loudly; export writes `route-sheet.html` (escaped, per-route tables) and `routes.geojson` (LineString per route + Point per store) |

## Evidence

| Check | Command | Result |
|---|---|---|
| Tests | `cargo test --workspace` | 48 passed, 0 failed — domain 19 (7 unit + 12 validation), sim 8, solver 12 (5 unit incl. cost arithmetic + 6 greedy + 1 proptest at 256 cases), cli 9 (4 lib + 4 export + 1 e2e) |
| Lints | `cargo clippy --workspace --all-targets` | clean on 2026-11-03; not enforced with `-D warnings` in current checks |
| Formatting | `cargo fmt --all` | applied (exit 0) |
| Bench (compiles + runs) | `cargo bench -p solver` | `greedy_insertion/metro_40`: **1.10 ms** [1.077, 1.132], 100 samples (858 µs before the post-review driver-occupancy checks) |
| Demo (40 stores / 3 depots / 12 vehicles, seed 42) | `cargo run -q -p cli -- gen-sim …` then `solve` then `export` | **40/40 stores served**, 8 routes, 825.5 km, Σ duration 1d 09:36, cost **$3,454.98**; solve wall time 141 ms including cargo overhead (re-measured after the review fixes) |
| Stress (200 stores / 5 depots / 60 vehicles, `--chain woolworths --city melbourne`, seed 7) | same pipeline | **200/200 served**, 41 routes, 4,060.4 km, Σ duration 6d 20:47, cost **$17,219.60**; solve wall time 1,054 ms including cargo overhead |
| End-to-end via the real binary | `cargo test -p cli --test e2e` | passes |

`data/demo/` holds the generated problem, solution, route sheet and GeoJSON from
the table above (gitignored).

## Gate results

### 1. Multi-day relay fixture — PASS

- `domain`: `multi_day_relay_with_two_shifts_is_expressible_and_valid` — a
  Sydney → Albury (hand-over) → Melbourne overnight run with two drivers on
  contiguous shifts validates cleanly; `a_shift_gap_is_rejected` proves the
  validator catches a shift that does not cover the first leg.
- `solver`: `multi_day_horizon_store_is_served_on_day_two` — the greedy handles
  a 48-hour horizon, serving a store whose window opens on day 2 (~714 km at
  75 km/h).
- Outcome: no redesign needed; a single day remains the degenerate case.
  Relay *optimisation* (rest placement, fatigue rules) is still M5 as planned.

### 2. Overpass truck-tag count — PARTIAL (rate-limited), decision recorded

Method: Overpass `out count` with key filters over a Greater Sydney bbox
(`-34.05,150.75,-33.60,151.30`), via multiple mirrors, 2026-11-03. Several
queries returned 504/timeout; the table lists everything that succeeded, each
row re-verified on its stated source.

| Query (ways, bbox) | Count | Source |
|---|---|---|
| `["maxweight"]` | 5,612 | maps.mail.ru mirror |
| `["maxweight"]["bridge"]` | 112 | maps.mail.ru |
| `["maxweight"]["tunnel"]` | 1 | maps.mail.ru |
| `["maxheight"]` | 1,328 | overpass-api.de |
| `["hgv"]` | 694 | overpass-api.de |
| `["highway"="motorway"]` | 1,424 | maps.mail.ru |
| `["highway"="residential"]` | 54,499 | maps.mail.ru |
| any `["highway"]` tag | 352,978 | maps.mail.ru |

Denominator: **173,604 drivable-class ways** (motorway…service; measured during
the logic review on the same bbox) — `maxweight` covers 3.2%, `maxheight` 0.8%
and `hgv` 0.4% of the drivable network. The `maxweight` tags are dominated by
street-level load limits, not mapped structures: only 112 of the 5,612 carry
`bridge` and 1 carries `tunnel` (both measured directly). Caveats: per-mirror
snapshots on a single date; the drivable-class denominator was measured via the
regex form, which this environment's proxy timed out on when re-run, so it is
reported once and not independently reproduced here.

Decision (per the M0 gate): **M3 = real OSM distances/durations + a curated,
versioned truck-constraints layer** (street-level load limits, bridge/tunnel
limits, council curfews, B-double/HML route restrictions). OSM truck tags are
used where present, not relied on. Re-run the queries before starting M3.

### 3. Solomon C101 calibration — COMPLETE (corrected 2026-11-05)

The calibration path was fixed after this report was written: the C101
problem uses `solomon_to_problem(inst, 60.0)`, so one distance unit maps to
one minute. Current state: greedy 841.68 with 100/100 served, local search
621.42 with 100/100 served. See [M1-REPORT.md](M1-REPORT.md) and
[PLAN.md](PLAN.md) for the current evidence.

### 4. Benchmark parser — decision recorded, not yet written

`vrp_parser` (v0.2.0, ~61 downloads) will only be an optional cross-check;
M1 vendors a ~50-line Solomon/VRPLib parser in the bench harness.

## Deviations and environment notes

- `rust-toolchain.toml` pins `stable-x86_64-pc-windows-gnu`: this machine has
  no MSVC linker (`link.exe` missing), so `channel = "stable"` (msvc host)
  cannot link anything.
- `criterion` is pinned to 0.5 with `default-features = false` (the registry
  offered 0.8.2; 0.5 was chosen for API stability in a stub bench, upgrade
  deferred) and `crates/solver/Cargo.toml` gained
  `[[bench]] harness = false` so the bench actually executes.
- sim fleet classes were re-tuned mid-integration — from
  (16,10,6)/(24,14,10)/(18,12,0) to (18,12,10)/(28,18,16)/(20,14,0) — after the
  default demo showed 4 unassigned stores: frozen demand 73 > frozen capacity
  64. After the tweak both demos serve everything.
- solver assigns one driver and one shift per route in the greedy (relay
  scheduling M5); multi-window stores are handled as a superset (earliest
  feasible window; sort deadline = earliest window end).
- Driver occupancy (added after the code review): `Solution::validate` rule 7
  rejects a driver scheduled on two routes with overlapping work spans
  (`DriverDoubleBooked`), and the greedy books a driver's span while building
  routes; a store that would require a double-booked driver stays unassigned.
- `union_covers` was corrected to half-open minute semantics
  (`shift.start <= m < shift.end`) after the code review found depot-dwell
  coverage treated shift ends inclusively — one minute more generous than the
  documented `[start, end)` semantics.
- `solve` refuses problems above 5,000 nodes (an O(n²) matrix, ≈200 MB) with a
  clear error instead of aborting on allocation.
- sim's `Problem::name` reports clamped counts when inputs are < 1.
- `gen-sim`/`solve` create missing parent directories for `--out`.
- Ledger-style checks (`total_*` re-derivation) are informational in M0 and not
  re-derived by `validate` — documented on the function.

## Known M0 limits

- Routes use exactly one driver and one shift (relay scheduling is M5).
- A driver is booked for a route's whole work span, so occupancy is
  deliberately conservative until relay-precise attribution exists (M5).
- `solve` accepts at most 5,000 nodes (≈200 MB distance matrix).

## Not done (at the time of this report)

Local search + metaheuristics, the C101 bench harness, OSM routing (`routing`),
G-NAF geocoding (`geo-au`), the API, the egui console, and driver tools — all
per the milestone plan. (As of 2026-11-05 the solver, routing, API and console
are implemented; see [PLAN.md](PLAN.md).)

## Reproduce

```console
cargo test --workspace
cargo clippy --workspace --all-targets
cargo bench -p solver
cargo run -q -p cli -- gen-sim --chain coles --city sydney --stores 40 --depots 3 --vehicles 12 --seed 42 --out data/demo/problem.json
cargo run -q -p cli -- solve --problem data/demo/problem.json --out data/demo/solution.json
cargo run -q -p cli -- export --problem data/demo/problem.json --solution data/demo/solution.json --out-dir data/demo
cargo run -q -p cli -- gen-sim --chain woolworths --city melbourne --stores 200 --depots 5 --vehicles 60 --seed 7 --out data/demo/stress-problem.json
cargo run -q -p cli -- solve --problem data/demo/stress-problem.json --out data/demo/stress-solution.json
```

Commits: `e29fc82` (scaffold) → `f27b20a` (M0 implementation: generator, solver,
CLI, review fixes and this report).
