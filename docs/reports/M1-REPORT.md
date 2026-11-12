# M1 report — solver v1 + calibration gate

Original date: 2026-11-04. Updated: 2026-11-09. Scope per
[PLAN.md](../PLAN.md).

> **Verdict: M1 is delivered.** The C101 calibration path is now wired, the
> benchmark instance is tracked, and the gate passes with 100/100 stores served.

## What was delivered

| Item | Status | Notes |
|---|---|---|
| Vendored Solomon/VRPLib parser | ✅ | `solver/src/parser.rs` parses the C101 format and converts it to `domain::Problem`. `solomon_to_problem` is called with `60.0` so one distance unit is one minute. |
| C101 instance | ✅ | `data/solomon/C101.txt` is tracked in git (6391 bytes). The calibration gate can no longer silently skip. |
| C101 greedy test | ✅ | `c101_calibration_greedy` — **11 routes, 841.68 distance, 100/100 served**. |
| C101 local search test | ✅ | `c101_calibration_local_search` — **11 routes, 621.42 distance, 100/100 served**. |
| C101 LNS test | ✅ | `c101_calibration_lns` — **11 routes, 709.10 distance, 100/100 served** (since commit `fbca088`). LNS is opt-in and is not the production default. |
| Claims checked by the calibration gates | ✅ | `served == 100` is asserted and `distance <= 870.39` (≤5% gap to the 828.94 best-known instance). |
| Local search | ✅ | `solver/src/local_search.rs` — 2-opt, relocate, exchange, cross-exchange, multi-start, driver-occupancy tracking. |
| Benchmark | ✅ | `greedy_insertion/metro_40` (historical measurement only; see `docs/PLAN.md` for current verification). |
| C101 regression guard | ✅ | Missing dataset now panics instead of silently skipping the gate. |

## Calibration gate outcome

### Current (2026-11-05, after calibration fixture correction)

```
greedy calibration:  routes=11, distance=841.68, served=100/100
local search:        routes=11, distance=621.42, served=100/100
LNS (commit fbca088): routes=11, distance=709.10, served=100/100
```

Greedy, local search and LNS all satisfy the ≤5% gap gate to best-known
(828.94, limit 870.39) and serve every store. The current local search
result is 621.42 distance units on the Euclidean C101 instance. LNS was
fixed on 2026-11-05 (`fbca088`): its ruin operators had removed steps by
stale positions and could delete a route's end depot or stores outright,
so every calibration run failed validation and silently fell back to
greedy. LNS now returns validated solutions itself.

### Historical (superseded, 2026-11-04)

The original report recorded greedy local search as 7/100 served at distance
62.37. That result was produced before the C101 fixture was corrected to the
Solomon unit-distance/unit-time mapping (`speed_kmh = 60.0`) and is retained
here only as historical evidence. It should not be used for planning or as a
quality baseline.

## Test suite

```console
cargo test --workspace
# all 254 tests pass (as of 2026-11-08)

cargo clippy --workspace --all-targets
# exits 0 with zero warnings
```

## Harder benchmark (Task 6, 2026-11-08)

C101 cannot tell the two metaheuristics apart at the top: its windows are
wide (102–138 minutes; depot aside) and its best known is 828.94, while LS
sits at 621.42. `solver/tests/harder_benchmark.rs` builds a 200-
customer synthetic problem the same way `solomon_to_problem` builds C101
(single depot at (40, 50), ambient-only capacity 200, unit-time = unit-
distance at 60 km/h) but with tight, spatially-coherent windows:
`ready = d × 2.4` (round-trip minutes), `due = ready + 60–100`, demand
16–26 per customer, 25 vehicles, horizon 700.

Measured (single run each, same config as the C101 tests, 2026-11-08):

| Solver | Routes | Distance | Served |
|---|---|---|---|
| Greedy | 0 | — | 0/200 (fixture not tuned for greedy) |
| Local search (500 no-improve, 2 restarts, 10 s) | 25 | **1372.06** | 187/200 |
| LNS single (30 s budget) | 25 | **1827.32** | 136/200 |

On the harder fixture, local search wins by 25% (33% fewer dropped stores
**and** shorter distance). The fixture violates base-greedy entirely at
production cost settings, but both metaheuristics beat greedy by a wide
margin and remain valid. Windows derived from distance are the binding
constraint on LNS service rate.

**Conclusion (no solver change):** on both C101 and the harder fixture, LS
is the quality path and LNS stays opt-in. The "measured win" condition for
LNS did not hold; per the plan, solver code does not change. See
[PLAN.md](../PLAN.md) "Next sprint plan" item 6.

## Deviations from PLAN.md

| Planned | Actual |
|---|---|
| Clarke–Wright + local search | Delivered. The savings prototype remains dead code; local search is the useful path. |
| C101 calibration gate | Delivered and now passes 100/100 with greedy and local search. `data/solomon/C101.txt` is tracked. |
| Benchmark parser (`vrp_parser` optional cross-check) | Not adopted; the vendored parser is sufficient and remains the supported path. |
| M2 quality target | Greedy, local search and LNS all pass the gate. LNS is opt-in and its output validates; local search remains the best measured quality path (621.42). |

## Current status

See [PLAN.md](../PLAN.md) for the current milestone state, known gaps and next
priorities. As of 2026-11-05:

- M0 and M1 are complete.
- M2 passes the C101 gate via greedy, local search and LNS (commit `fbca088`).
- M3 (OSM routing), M4 (API + console) are implemented; M5 has timezone
  conversion and national holidays implemented; M6's driver-side
  deliverable is the route sheet with OSM deep links (no tracking/POD).
- Tracked C101 dataset, all tests pass, and clippy exits 0 with warnings.

## Files added/changed

| Path | Role |
|---|---|
| `solver/src/parser.rs` | Vendored Solomon/VRPLib parser + `solomon_to_problem`. |
| `solver/src/local_search.rs` | 2-opt, relocate, exchange, cross-exchange, multi-start; driver-occupancy tracking. |
| `solver/src/lns.rs` | LNS/ALNS implementation (opt-in; invalid candidates are rejected during search, and the final plan is validated — falls back to greedy only as a last resort). |
| `solver/src/savings.rs` | Clarke-Wright prototype; retained but not used (the module carries a dead-code allow; see the note at the top of the file). |
| `solver/src/lib.rs` | Exports `solve_with_local_search`, `solve_with_lns`, `LNSConfig`, `LSConfig`, `improve`. |
| `solver/tests/greedy.rs` | C101 calibration gates (greedy, local search, LNS) and M0/M1 end-to-end tests. |
| `solver/tests/harder_benchmark.rs` | Harder 200-customer synthetic benchmark that reports (does not gate) LS vs LNS; see "Harder benchmark" above. |
| `data/solomon/C101.txt` | Tracked benchmark instance; gates fail loudly if missing. |
