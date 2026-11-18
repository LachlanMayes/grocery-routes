# M2 report — solver v2 (LNS + multi-start)

Date: 2026-11-06 (historical snapshot; current state in [PLAN.md](../PLAN.md)).
Scope and gates per [PLAN.md](../PLAN.md).

> Historical snapshot. Test counts and lint claims reflect 2026-11-06; the
> LNS decision was later closed by the harder benchmark in
> [M1-REPORT.md](M1-REPORT.md) (2026-11-09).

## What was delivered

- **LNS validity fixed** (`fbca088`): `solver::lns` now produces valid
  plans; the earlier LNS output failed `Solution::validate`.
- **LNS multi-start** (`2b88822`): `solve --ls --repeat N` runs N seeded
  LNS searches in parallel (scoped threads, no `rayon`) and keeps the best.
- **Curfew re-scheduling remains a restore**, not an optimiser (see M5).

## C101 calibration (2026-11-06)

| Solver | Routes | Distance | Served |
|---|---|---|---|
| Greedy | 11 | 841.68 | 100/100 |
| Local search | 11 | 621.42 | 100/100 |
| LNS (single) | 11 | 709.10 | 100/100 |
| LNS ×8 (120 s/run) | 11 | 679.85 | 100/100 |

Gate: distance ≤ 870.39, all 100 served — passes for all four.

## Decision

Local search (621.42) remains the best measured quality path; LNS stays
opt-in. Multi-start helps LNS (709.10 → 679.85) but does not beat local
search. The "measured win" condition for promoting LNS was later closed
negatively by `solver/tests/harder_benchmark.rs` (LS 1372.06 vs LNS
1827.32 on a harder 200-customer instance).

## Checks

- `cargo test --workspace`: 239 tests pass (2026-11-06).
- `cargo clippy --workspace --all-targets`: 0 warnings.
- `cargo fmt --all`: clean.

## Not done (by design)

- Promoting LNS over local search (no measured win).
- `rayon` (scoped threads used instead).
- Curfew-aware optimisation (M5 follow-up).
