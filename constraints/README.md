# Curated truck constraints

`truck-constraints.json` carries **hand-verified**, way-specific truck limits
that OSM does not tag (or tags incorrectly). It is intentionally **empty** in
this repository: no limit is embedded without an operator supplying and
signing off a verified source. Entries are applied by
`routing::curated::CuratedConstraints` and can only **tighten** limits.

## Schema (v1)

```json
{
  "entries": [
    {
      "way_id": 123456789,
      "maxweight_tonnes": 12.5,
      "maxheight_m": 4.3,
      "note": "Bridge load limit, signposted (GM routes only)",
      "source": "TfNSW heavy vehicle road manager, verified on site 2026-09",
      "verified_by": "j.smith",
      "verified_at": "2026-10-01"
    }
  ]
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| `way_id` | positive integer | yes | OSM way id the limit applies to (both travel directions) |
| `maxweight_tonnes` | finite number ≤ 200 | at least one of the two limits | Maximum legal weight, tonnes |
| `maxheight_m` | finite number ≤ 10 | at least one of the two limits | Maximum legal height, metres |
| `note` | non-empty string | yes | What the limit is and why |
| `source` | non-empty string | yes | Where the limit came from (authority, URL, phone call, site visit) |
| `verified_by` | non-empty string | yes | Who verified it (name or handle) |
| `verified_at` | ISO 8601 date `YYYY-MM-DD` | yes | When it was verified |

Unknown fields are rejected. Duplicate `way_id` values are rejected. Entries
missing provenance fail to load — the file cannot accumulate unverifiable
limits silently.

## How to add an entry

1. Get the OSM way id (e.g. from openstreetmap.org — click the road, copy the id
   from the URL/selection panel, or from your PBF extract tooling).
2. Verify the limit with an authoritative source — do **not** copy OSM tags
   (they are the input to this check, not the authority), forum posts or
   unverified signage photos. For NSW: TfNSW heavy vehicle road manager and
   the relevant council/RMS bridge registers; for other states: the state
   road manager and the NHVR. Record exactly which source you used.
3. Add the entry with full provenance and today's date.
4. Re-run the routing checks (`cargo test -p routing`) to make sure the file
   loads, and re-solve a route that exercises the way.
5. Re-verify on a schedule — bridge upgrades and signage change. A stale
   entry is worse than none; delete or re-date it.

## What this does and does not cover

- **Covered**: weight/height limits verified per way, folded into the graph
  before routing (tightening only).
- **Not covered**: B-double/HML permit networks (permit classes, not physical
  limits), commodity-specific rules, and limits that exist but were never
  verified — those must come from the sources above, not from this file.
  Time-based curfews are a different mechanism: they live in
  `Problem::curfews` and are enforced by `solve --enforce-curfews`
  (see `docs/TRUCK-CONSTRAINTS.md`).

See `docs/TRUCK-CONSTRAINTS.md` for the full enforcement model.

## Desktop research draft

`draft-desktop-research-2026-11.json` holds entries researched from published
sources (TfNSW, Linkt) for the synthetic Sydney plan corridor. Every entry is
marked `verified_by: "desktop-research (unverified on-site); pending operator sign-off"`
— it is not on-site verified and must not be copied into `truck-constraints.json`
without human review. See `docs/research/CORRIDOR-RESEARCH.md`.
