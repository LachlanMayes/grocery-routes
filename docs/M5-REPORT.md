# M5 report — multi-day optimisation + AU/grocery depth

Date: 2026-11-09 (current state; see [PLAN.md](PLAN.md)).
Scope and gates per [PLAN.md](PLAN.md).

## What was delivered

- **NHVR fatigue rules**: between-shift rest, work caps.
- **Per-chain receiving defaults** and horizon→local clock conversion.
- **Holidays**: national + state/territory calendars, `holidays` CLI.
- **Christmas-peak profile**: `gen-sim --christmas-peak`.
- **Returnables ledger**: `domain::ledger`, route-sheet section, `export`
  writes `returnables.csv`.
- **Curfew enforcement** (`routing::timed`, `solver::curfew`): post-solve
  feasibility restore; `solve --enforce-curfews --network-timezone <IANA>`.
- **Curfew-aware construction** (Task 5, `e5a8130`): `Legs` enum threaded
  through the greedy scheduler; `solver::curfew_aware`, `solve --curfew-aware`.
- **Relay/rest placement** (Task 4, `b17b5a0`): `solver::relay` post-pass
  with backtracking over (driver, shift) pairs; `solve --relay`.
- **G-NAF geocoder** (`crates/geo-au`): flat-file ingest → SQLite,
  `geocode`/`reverse`; validated against synthetic fixtures only.

## What remains (operator steps / optional)

- Real G-NAF release validation and wiring geocoding into store import.
- Real OSM extract for CH timing.
- Human verification of corridor legality.
- Curfew-aware LS/LNS improvement phases (construction is aware; improvement
  loops still use the static matrix).
- LNS promotion (closed negatively; see [M1-REPORT.md](M1-REPORT.md)).

## Checks

- `cargo test --workspace`: 254 tests pass (2026-11-09).
- `cargo clippy --workspace --all-targets`: 0 warnings.
- `cargo audit --no-fetch`: exit 0.
