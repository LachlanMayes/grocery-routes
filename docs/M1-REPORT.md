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
C101 calibration: routes=6, distance=62.37, served=7/100
```

- The greedy solver on Euclidean C101 coordinates serves only 7/100 stores with 6 routes at 62.37 distance units.
- This is far from the best-known (10 routes, 828.94 distance, 100/100 served). The simple greedy insertion heuristic is insufficient for C101's tight time windows.
- This sets the M2 quality target per PLAN.md: the local search / metaheuristic must achieve **≤5% gap to best-known** on C101 and show clear gains on larger instances.
- The calibration test records the true greedy baseline for regression tracking.

## Deviations from PLAN.md

| Planned | Actual |
|---|---|
| Clarke–Wright + local search (2-opt, relocate, exchange) | **Deferred** — the savings + LS prototype (`crates/solver/src/savings.rs`) had driver-occupancy and unassigned-tracking bugs. The working greedy solver is retained as the M1 baseline. Local search will be M2. |
| C101 calibration *gate* (sets M2 target) | **Done** — recorded **6 routes / 62.37 dist / 7% served**. The gate measures and records the greedy baseline; M2 target remains ≤5% gap per PLAN.md. |

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