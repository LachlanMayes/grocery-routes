# M1 report — solver v1 + calibration gate

Date: 2026-11-03. Scope per [PLAN.md](PLAN.md).

## What was delivered

| Item | Status | Notes |
|---|---|---|
| Vendored Solomon/VRPLib parser | ✅ | `crates/solver/src/parser.rs` — parses C101 format, converts to `domain::Problem` with Euclidean coordinates |
| C101 instance file | ✅ | `data/solomon/C101.txt` (100 customers + depot) |
| C101 calibration test | ✅ | `c101_calibration_greedy` — runs if file present, records routes/distance/served |
| Greedy solver (baseline) | ✅ | Still the default `solve()` — 40-store demo: 8 routes, 825.5 km, $3,454.98 |
| Benchmark | ✅ | `greedy_insertion/metro_40`: **912 µs** [909, 915], 100 samples |

## Calibration gate outcome

```
C101 calibration: routes=10, distance=828.94, served=100/100
```

- The greedy solver on Euclidean C101 coordinates exactly matches the best-known distance (828.94) with 10 routes.
- This sets the M2 quality target: the local search / metaheuristic must **not regress** on C101 and should improve on larger instances.

## Deviations from PLAN.md

| Planned | Actual |
|---|---|
| Clarke–Wright + local search (2-opt, relocate, exchange) | **Deferred** — the savings + LS prototype (`crates/solver/src/savings.rs`) had driver-occupancy and unassigned-tracking bugs. The working greedy solver is retained as the M1 baseline. Local search will be M2. |
| C101 calibration *gate* (sets M2 target) | **Done** — recorded 10 routes / 828.94 km. The gate is "measure and record", not "achieve a target". |

## Files added/changed

| Path | Role |
|---|---|
| `crates/solver/src/parser.rs` | Solomon/VRPLib parser + `solomon_to_problem` converter |
| `crates/solver/src/savings.rs` | Prototype Clarke–Wright + LS (kept for M2 reference) |
| `crates/solver/tests/greedy.rs` | C101 calibration test |
| `data/solomon/C101.txt` | Standard 100-customer instance |
| `crates/solver/Cargo.toml` | `anyhow` + `tempfile` deps |

## Test suite

```
cargo test -p solver
# 15 tests (7 unit + 7 integration + 1 proptest) pass
# C101 calibration test runs and prints baseline numbers
```

## Next steps (M2)

1. Implement local search (2-opt, relocate, exchange, cross-exchange) on top of the greedy baseline.
2. Target: improve on C101 (if possible) and show clear gains on 200–1000 stop instances.
3. Keep the C101 calibration test as a regression guard.
4. Move the savings prototype into the main solver once driver occupancy is handled correctly.