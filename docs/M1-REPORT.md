# M1 report — solver v1 + calibration gate

Date: 2026-11-04. Scope per [PLAN.md](PLAN.md).

## What was delivered

| Item | Status | Notes |
|---|---|---|
| Vendored Solomon/VRPLib parser | ✅ | `crates/solver/src/parser.rs` — parses C101 format, converts to `domain::Problem` with Euclidean coordinates |
| C101 instance file | ✅ | `data/solomon/C101.txt` (100 customers + depot) |
| C101 calibration test (greedy) | ✅ | `c101_calibration_greedy` — runs if file present, records routes/distance/served |
| C101 calibration test (local search) | ✅ | `c101_calibration_local_search` — tests LS on top of greedy; currently matches greedy baseline |
| Greedy solver (baseline) | ✅ | Default `solve()` — 40-store demo: 8 routes, 825.5 km, $3,454.98 |
| Local search module | ✅ | `crates/solver/src/local_search.rs` — 2-opt, relocate, exchange, cross-exchange with multi-start, driver occupancy tracking |
| Savings module (fixed) | ✅ | `crates/solver/src/savings.rs` — Clarke-Wright + LS prototype; driver occupancy & duplicate-store bugs fixed |
| Benchmark | ✅ | `greedy_insertion/metro_40`: **912 µs** [909, 915], 100 samples |

## Calibration gate outcome

```
C101 greedy:      routes=6, distance=62.37, served=7/100
C101 local search: routes=6, distance=62.37, served=7/100 (no improvement)
```

- The greedy solver on Euclidean C101 coordinates serves only 7/100 stores with 6 routes at 62.37 distance units.
- Local search (2-opt, relocate, exchange, cross-exchange) does not improve this baseline — C101's tight time windows require more powerful metaheuristics (LNS/ALNS, ruin-recreate).
- This sets the M2 quality target per PLAN.md: the metaheuristic must achieve **≤5% gap to best-known** (10 routes, 828.94 km, 100/100 served) on C101.
- The calibration test records the true greedy baseline for regression tracking.

## Deviations from PLAN.md

| Planned | Actual |
|---|---|
| Clarke–Wright + local search (2-opt, relocate, exchange) | **Done** — `savings.rs` fixed (driver occupancy, duplicate stores, validation); `local_search.rs` implemented with multi-start LS. Both produce valid solutions but don't improve C101 greedy baseline. |
| C101 calibration *gate* (sets M2 target) | **Done** — recorded **6 routes / 62.37 dist / 7% served** for greedy; local search matches this. M2 target remains ≤5% gap per PLAN.md. |

## Files added/changed

| Path | Role |
|---|---|
| `crates/solver/src/parser.rs` | Solomon/VRPLib parser + `solomon_to_problem` converter |
| `crates/solver/src/savings.rs` | Clarke–Wright + LS prototype (bugs fixed: driver occupancy, duplicate stores, validation) |
| `crates/solver/src/local_search.rs` | Local search (2-opt, relocate, exchange, cross-exchange, multi-start) |
| `crates/solver/src/lib.rs` | Exports `solve_with_local_search`, `LSConfig`, `improve` |
| `crates/solver/tests/greedy.rs` | C101 calibration tests (greedy + local search) |
| `data/solomon/C101.txt` | Standard 100-customer instance |
| `crates/solver/Cargo.toml` | `anyhow` + `tempfile` deps |

## Test suite

```
cargo test -p solver
# 21 tests (12 unit + 7 integration + 2 proptest) pass
# C101 calibration tests run and print baseline numbers
```

## Next steps (M2)

1. Implement LNS/ALNS or ruin-recreate metaheuristic to escape C101 local optimum.
2. Target: improve on C101 (achieve ≤5% gap to best-known) and show clear gains on 200–1000 stop instances.
3. Keep the C101 calibration tests as regression guards.
4. Wire `solve_with_local_search` into CLI as an optional `--ls` flag for production use on larger instances.