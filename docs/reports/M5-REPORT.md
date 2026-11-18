# M5 report — multi-day optimisation + AU/grocery depth

Date: 2026-11-09 (current state; see [PLAN.md](../PLAN.md)).
Scope and gates per [PLAN.md](../PLAN.md).

## What was delivered

- **NHVR fatigue rules**: between-shift rest, work caps.
- **Per-chain receiving defaults** and horizon→local clock conversion.
- **Holidays**: national + state/territory calendars, `holidays` CLI.
- **Christmas-peak profile**: `gen-sim --christmas-peak`.
- **Returnables ledger**: `domain::ledger`, route-sheet section, `export`
  writes `returnables.csv`.
- **Curfew enforcement** (`routing::timed`, `solver::curfew`): post-solve
  feasibility restore; `solve --enforce-curfews --network-timezone <IANA>`.
- **Curfew-aware construction** (`e5a8130`): `Legs` enum threaded
  through the greedy scheduler; `solver::curfew_aware`, `solve --curfew-aware`.
- **Relay/rest placement** (`b17b5a0`): `solver::relay` post-pass
  with backtracking over (driver, shift) pairs; `solve --relay`.
- **G-NAF geocoder** (`geo-au`): flat-file ingest → SQLite,
  `geocode`/`reverse`; validated against the Aug 2026 release
  (16,970,406 addresses, 0 skipped).

## What remains (out of scope for software release)

- Corridor legality verification (operator responsibility — per-bridge load
  ratings and human sign-off of `constraints/truck-constraints.json`; see
  [PLAN.md](../PLAN.md) known gaps item 1).
- Curfew-aware LS/LNS improvement phases (construction is aware; improvement
  loops still use the static matrix).
- LNS promotion (closed negatively; see [M1-REPORT.md](M1-REPORT.md)).
- G-NAF store-import wiring (done; `import-stores` command).

## Checks

- `cargo test --workspace`: 254 tests pass (2026-11-09).
- `cargo clippy --workspace --all-targets`: 0 warnings.
- `cargo audit --no-fetch`: exit 0.
