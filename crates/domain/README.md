# `domain` — timeline model and validation contract

Pure data model shared by every other crate. No I/O, no solver logic.

- `Minutes` / horizon clock, multi-day capable timeline.
- Entities: `Depot`, `Store`, `Vehicle`, `Driver`, `Shift`, `Problem`, `Solution`,
  `RoutePlan`, `Node`, `Load`, `TimeWindow`, `CostModel`, `ChainRules`,
  `CurfewRules`, `FatigueRules`.
- `Solution::validate` — 19 violation kinds (structure, capacity, windows,
  driver-shift coverage incl. union coverage for depot hand-over dwells).
- `holidays` — national + state/territory calendars.
- `ledger` — returnables (cages issued vs empties collected per banner).
- `clock` — horizon→local civil time resolution.

Every other crate depends on this; changing it is a contract change.
