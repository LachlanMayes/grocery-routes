# `solver` — matrices, greedy insertion, local search, LNS/ALNS

- Distance matrices: haversine and road-network (via `routing`).
- Greedy insertion solver with full-route rescheduling on every candidate.
- Local search (`improve`) and LNS/ALNS with multi-start (`--repeat`).
- Curfew re-scheduling (`curfew`, `enforce_curfews`) and curfew-aware
  construction (`curfew_aware`, `Legs`).
- Relay post-pass (`relay`, `--relay`).
- C101 calibration gate: greedy 841.68, local search 621.42 (best), LNS
  709.10 single / 679.85 ×8.

Local search remains the quality path; LNS stays opt-in.
