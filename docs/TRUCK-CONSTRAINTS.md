# Truck constraints — enforcement model and limits

Date: 2026-11-05. Scope per [PLAN.md](PLAN.md) (known gap 4).

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

## What is NOT enforced

| Gap | Why |
|---|---|
| Legal bridge/tunnel weight & height limits not present in OSM tags and not curated | OSM tag coverage in Sydney bbox (M0 Overpass count, 2026-11-03): `maxweight` 3.2%, `maxheight` 0.8%, `hgv` 0.4% of ways. **The empty `constraints/truck-constraints.json` means no such limit is assumed anywhere.** Plans through un-annotated ways are not legally verified — see "Operator responsibilities". |
| Time-based curfews (e.g. peak-hour restrictions, local-area curfews) | The router is not departure-time aware; a curfew applies to a time window, not to an edge. Curfews are stored in the model (`Problem::curfews`) and documented as data-only. Enforcement requires time-dependent routing, which is not implemented. |
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

1. Before dispatching on a plan produced with `--osm`, verify each
   non-obvious leg against the sources below — or accept that the plan is
   an optimisation proposal, not an authorisation to drive.
2. If a limit is found that OSM does not tag, add it to
   `constraints/truck-constraints.json` with full provenance
   (`constraints/README.md`) and re-solve.
3. Record the check (who, when, which sources) in your operations folder;
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

## Future work (explicitly not done)

- Departure-time-aware routing to enforce curfews and school-zone
  restrictions.
- Permit-class route networks loaded from NHVR/state sources.
- Automatic extraction of candidate constraints from street-view signage
  (still requires human sign-off before loading).
