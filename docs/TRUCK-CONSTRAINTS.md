# Truck constraints — enforcement model and limits

Date: 2026-11-06. Scope per [PLAN.md](PLAN.md).

This document states precisely what is and is not enforced when routing
heavy vehicles, and what an operator must do to get legally usable route
plans out of the system.

## What the stack enforces today

| Layer | What it does | Where |
|---|---|---|
| OSM tag filter | Drops edges that betray a physical/legal restriction **when OSM tags it**: `maxweight`, `maxheight`, `hgv=no`, `access=no`/`access=private`; `hgv=delivery` treated as forbidden for goods vehicles. Filtering happens per route; unreachable edges are excluded from path-finding. | `crates/routing/src/truck_constraints.rs` |
| One-way awareness | Honors `oneway=yes` and reverses for `oneway=-1`. | `crates/routing/src/osm_ingest.rs` |
| Curated way constraints | Folds **hand-verified** per-way limits (weight/height) into the graph after ingest. Tightening only; never loosens an existing limit. CLI: `solve --osm … --constraints constraints/truck-constraints.json`. | `crates/routing/src/curated.rs` |
| Refusal to route | If no legal path exists between depot and store under the profile, the leg is rejected rather than routed through the barrier. | `crates/routing/src/router.rs` |
| Curfew enforcement | A curfew names a road class (OSM `highway` value) and a local day/time window. After solving, each leg is recomputed time-dependently: a truck **waits at the previous node** until its traversal would be clear of every window, and a store whose waits no longer fit its receiving windows, shift, duration limit or driver occupancy moves to `unassigned` with a recorded reason. CLI: `solve --enforce-curfews --network-timezone <IANA>` (needs `--osm` and `Problem.horizon_start`). | `crates/routing/src/timed.rs`, `crates/solver/src/curfew.rs` |
| Corridor audit | Reports every OSM way a solved plan traverses with its limit tags, and explicitly flags ways with **no limit data**. CLI: `audit`. This is reporting, not enforcement. | `crates/routing/src/audit.rs` |

Curfew modelling choices worth knowing:

- **Presence-interval semantics.** A traversal is blocked when the interval
  `[enter, enter + duration)` overlaps a window; the truck waits before
  entering. Waiting is the only remedy applied; the plan is not re-optimised
  under curfews (route order and fleet assignment are fixed).
- **Local day/time resolution.** Windows are minutes of the local day on the
  selected weekdays, resolved through the horizon anchor and the network
  timezone (`--network-timezone` or the first depot's timezone). Without an
  anchor the rules stay inert data.
- **Route shapes.** Enforcement re-schedules one shift at a time; a route
  with a mid-route depot (a relay hand-over) has no single-shift schedule and
  is dropped with every store marked `InvalidRoute`. Current solver output
  never produces that shape.

## What is NOT enforced

| Gap | Why |
|---|---|
| Legal bridge/tunnel weight & height limits not present in OSM tags and not curated | OSM tag coverage in Sydney bbox (M0 Overpass count, 2026-11-03): `maxweight` 3.2%, `maxheight` 0.8%, `hgv` 0.4% of ways. **The empty `constraints/truck-constraints.json` means no such limit is assumed anywhere.** Plans through un-annotated ways are not legally verified — see "Operator responsibilities". |
| Curfew rules that are not encoded in `Problem::curfews` | The enforcer only knows the windows you give it; school zones, event closures and ad-hoc restrictions are not inferred from OSM. |
| Curfew-aware optimisation | Enforcement is a post-solve feasibility restore (waiting and dropping stores), not time-dependent costs inside the search. A plan may therefore use more vehicles or serve fewer stores than a curfew-aware optimiser would. |
| B-double / HML / PBS permit route networks | Permits are a legal class, not a physical property of a way; OSM does not model them reliably. No permit data is embedded. |
| Commodity/vehicle-specific rules (dangerous goods, ADR) | Not modelled; treat as out of scope. |
| State access pre-approvals, NHVR notices | Not modelled. |

## The empty curated file is deliberate

`constraints/truck-constraints.json` ships with `{ "entries": [] }`.
Embedding real limits would require verifying them against authoritative
sources (NHVR notices, state road manager registers, council bridge
registers, on-site signage). Invented or copied-from-nowhere limits would
be worse than none — the solver would treat them as law. The loader
rejects entries without provenance (`source`, `verified_by`,
`verified_at`) so the file cannot silently accumulate unverifiable data.

## What was actually checked in this repository

- OSM ingest correctly parses ways, honors `oneway=-1`, drops non-drivable
  highways, and round-trips edges with their way ids (unit tests).
- Truck filtering applies OSM-tagged limits exactly (unit tests for
  `maxweight`, `maxheight`, `hgv`, `access` with and without tags).
- Curated constraints tighten (never loosen) the graph; bad files fail
  loudly (10 unit tests).
- Routing is covered by Dijkstra correctness tests on synthetic graphs
  with hand-checked shortest paths, plus one-way handling tests.
- Curfew routing is covered by unit tests for waiting, presence-interval
  boundaries, wrap-midnight windows, day masks, alternate routes and
  untagged roads, plus a cross-layer test that drops a store whose window
  the wait breaks. Contraction hierarchies are checked against Dijkstra
  pair by pair.

## What was NOT checked

- **Sydney→Melbourne (or any corridor) against real road limits.** No
  OSM extract is present in this repository, and none was downloaded for
  this work. There has been no comparison of computed routes against
  signage, bridge registers or road-manager notices. Any claim that a
  route plan is legal for a B-double or semi is unfounded until the
  operator verifies it.
- There is no automated test that the Hume Highway, M31 or any specific
  corridor is route-legal for any vehicle class.

## Operator responsibilities

1. Run `audit` against the same extract, profile and matrix strategy used to
   solve (`audit --ch` when the plan was solved with `solve --ch`). It lists
   every traversed way, its limit tags and the ways with **no** limit data;
   start verification with those.
2. Before dispatching on a plan produced with `--osm`, verify each
   non-obvious leg against the sources below — or accept that the plan is
   an optimisation proposal, not an authorisation to drive.
3. If a limit is found that OSM does not tag, add it to
   `constraints/truck-constraints.json` with full provenance
   (`constraints/README.md`) and re-solve.
4. Record the check (who, when, which sources) in your operations folder;
   this repository does not track your verifications.

### Authoritative sources

- NHVR (National Heavy Vehicle Regulator) — vehicle standards and notices.
- State heavy vehicle road managers (e.g. TfNSW in NSW, VicRoads in VIC)
  — road access, notices and bridge registers.
- Local council bridge load/height registers — for local roads under
  15 tonnes.
- On-site signage — the last word at the actual choke point.

## Adding an entry

See `constraints/README.md`. The file is loaded with:

```console
cargo run -p cli -- solve --problem problem.json --out solution.json \
  --osm data/osm/australia-latest.osm.pbf \
  --constraints constraints/truck-constraints.json \
  --maxweight 42.5 --maxheight 4.3 --hgv
```

Validate a constraints file and optionally export it to SQLite (table
`curated_entries`, one row per way id, ordered by way id):

```console
cargo run -p cli -- constraints --file constraints/truck-constraints.json \
  --sqlite data/constraints.sqlite
```

## Rule set for curated entries (v1)

An entry describes one OSM way that carries a physical legal limit OSM does
not tag. The operator supplies the way id, the limit and its provenance —
the numbers below are the schema bounds, not pre-filled law.

| Highway class in OSM | What to verify | Typical bound |
|---|---|---|
| `primary`, `secondary`, `tertiary`, `residential`, `unclassified` | bridge load rating (TfNSW/state bridge register), tunnel clearance | weight 5–42.5 t, height 3.5–4.6 m |
| `service`, `track` | council access restriction signage | weight 5–25 t |
| `motorway`, `trunk` | usually permit territory — do not curate unless a structure limits it; note the structure name | — |

Rules:

- **One entry per way id.** Duplicates fail validation loudly.
- **Tighten only.** The graph keeps the minimum of (OSM tag, curated limit). A curated entry never clears an OSM restriction.
- **Provenance is mandatory.** `source`, `verified_by`, `verified_at` cannot be empty; the loader rejects the file otherwise.
- **At least one limit.** `maxweight_tonnes` or `maxheight_m` (or both) must be set, positive and finite (weight ≤ 200 t, height ≤ 10 m).
- **Both travel directions.** The entry applies to every edge built from that way, one-way or not.
- **Synthetic graph edges carry no way id** and are therefore never tightened — the curated layer only sees OSM-derived graphs.

The OSM coverage that motivates this file (M0 Overpass study of the Sydney bbox, 2026-11-03): `maxweight` on 3.2% of drivable ways, `maxheight` on 0.8%, `hgv` on 0.4%. The rest need a human.

## Future work (explicitly not done)

- Time-dependent costs inside the LS/LNS *improvement* phases (curfew-aware
  optimisation is now construction-aware via `--curfew-aware`; only the
  metaheuristic improvement loops still use the static matrix), school
  zones and event closures as first-class curfew sources.
- Permit-class route networks loaded from NHVR/state sources.
- Automatic extraction of candidate constraints from street-view signage
  (still requires human sign-off before loading).
