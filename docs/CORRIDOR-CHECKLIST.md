# Corridor legality checklist — task completion guide (2026-11-10)

This is everything you need to complete the corridor legality task for the
synthetic Sydney plan. It is a work checklist, not legal verification. The
repo documents findings in `docs/CORRIDOR-RESEARCH.md`; the numbers below are
from that research plus the `audit` output.

Context: `data/demo/console-test-problem.json` (8 stores, 2 depots, 4 vehicles),
solved with `--osm data/osm/nsw-latest.osm.pbf --hgv --maxweight 42.5 --maxheight 4.3 --ch`,
then `cargo run --release -p cli -- audit --problem data/demo/console-test-problem.json --solution data/demo/ctp-sol-ch.json --osm data/osm/nsw-latest.osm.pbf --ch --out-dir data/demo`.

## What is already done (desktop, 2026-11-10)

| Item | Status |
|---|---|
| Corridor list (1,277 traversed ways, 1,170 flagged) | ✅ `data/demo/corridor-audit.csv` |
| Per-way OSM tags (name/ref/bridge/tunnel/limits) | ✅ from Overpass API |
| Tunnel clearances from official sources | ✅ TfNSW "Using Sydney's tunnels"; Linkt height limits |
| Draft constraint entries (9) | ✅ `constraints/draft-desktop-research-2026-11.json`, format-validated |
| Research report | ✅ `docs/CORRIDOR-RESEARCH.md` |
| `truck-constraints.json` | ⬜ **intentionally empty — this is what you complete** |

The 4.3 m HGV clears every tunnel on the plan (Cross City 4.4 m, M4 East
5.1 m, M5 East ≥4.4 m). No restricted-access notices were found for the
arterials or motorways traversed. Bridges were **not** checked.

## What you must do

### Step 1 — Review the tunnel findings and draft entries

Check each entry in `constraints/draft-desktop-research-2026-11.json` against
the signage/notices you can access. Entries (all height-only, verified by
desktop research only):

| way_id | value | entry |
|---|---|---|
| 389141181, 244269360, 50887066, 389140309, 782822601 | 4.4 m | Cross City Tunnel |
| 703986355, 704119174, 1226839416 | 5.1 m | M4 East Tunnel |
| 92024832 | 4.4 m (lower of 4.4/4.6) | M5 East tunnel — **confirm on site which tube** |

### Step 2 — Resolve the two unidentified tunnels

- **Way 173716163** ("Princes Highway Offramp", motorway_link, 0.51 km, 12 traversals):
  OSM gives no name/ref/clearance. Check the overhead signs at the M5 East
  Princes Highway exit. If it is the M5 East tunnel, the entry applies
  (4.4 m); otherwise record the real clearance or reroute over-4.3 m vehicles.
- **Way 92024832** ("M5 Motorway", tunnel): confirm tube (Cooks River 4.4 m /
  Main 4.6 m). The draft uses 4.4 m conservatively.

### Step 3 — Verify the 92 bridge ways (the biggest open exposure)

**First, the published data.** TfNSW's NSW State Roads Vertical Clearances
dataset (XLSX, Oct 2026, CC-BY, 1,214 structures) can be matched to the
corridor: 93 clearances within 60 m, minimum **4.36 m (Cooks River)** (see
[docs/CORRIDOR-RESEARCH.md](CORRIDOR-RESEARCH.md)). That clears the 4.3 m
height question for under-bridges on State Roads from an official source. It
is State Roads only, height only, and information-purpose only — no load
ratings, and regional/local roads are excluded.

**What's left** is the load-rating check for the 92 bridge ways (structures
our road passes over, not under — i.e. the OSM `bridge=yes` ways) plus
structures on regional/local roads. Grouped by road:

| Road | # | way IDs |
|---|---|---|
| M5 Motorway | 18 | 4305946, 5191269, 22575370, 23267078, 23267195, 23361049, 156387611, 172423228, 172510703, 172512017, 172512025, 172512027, 1034786171, 1181941082, 8039303, 23267086, 23267196, 1174223113 |
| Western Motorway (M4) | 11 | 8062651, 8071600, 14087671, 46995964, 172359405, 172359414, 172360395, 173393968, 1107821118, 1107821124, 1187728461 |
| Southern Cross Drive (M1) | 10 | 4869834, 4869837, 5150507, 5150509, 170318737, 170318738, 170318739, 170318740, 1530724293, 1530724295 |
| Western Distributor (A4) | 10 | 3508815, 3997051, 47108004, 172288806, 385523799, 551547086, 551547087, 551547088, 847868258, 1458823053 |
| General Holmes Drive | 5 | 4331807, 1286172457, 1286172459, 1545560222, 1545560228 |
| Orange Grove Road (A28) | 4 | 220416661, 220416662, 277179209, 277179219 |
| Cambridge Street (A28) | 4 | 283740942, 283740962, 285374526, 285374527 |
| Bangor Bypass | 3 | 108065528, 173396362, 175535383 |
| Bathurst Street Offramp | 3 | 4368601, 4368604, 322859082 |
| Syd Einfeld Drive | 3 | 77478291, 253166187, 1271107455 |
| River Road | 2 | 50196585, 661138631 |
| Princes Highway (A1) | 2 | 147103906, 147218105 |
| Cumberland Highway (A28) | 2 | 20135745, 1218482668 |
| Betts Road (A28) | 2 | 27105942, 172556821 |
| Bexley Road | 2 | 152539538, 152539539 |
| Hume Highway (A28) | 1 | 8074021 |
| Alfords Point Road (A6) | 1 | 45610930 |
| Acacia Road (A1) | 1 | 148397038 |
| King Georges Road (A3) | 1 | 92003591 |
| The Grand Parade | 1 | 4590770 |
| Clancy Street Exit | 1 | 173007508 |
| Clancy Street | 1 | 499827599 |
| Kingsgrove Road | 1 | 130845134 |
| Henry Lawson Drive | 1 | 9869966 |
| Sir Bertram Stevens Drive | 1 | 40773768 |
| Scarborough Street | 1 | 172616043 |

The full machine-readable list with lengths and traversal counts is
`data/demo/corridor-audit.csv` (filter `has_limit_data == no`); per-way OSM
tags are in `data/demo/flagged-ways-info.json`.

For each bridge, record: way id, structure name/location, source (register
URL/call reference or on-site sign), limit found (weight and/or height) or
"none — default limits apply", who verified, and the date. One row per way.

### Step 4 — Fold the verified findings into the main file

Copy only the entries you have **personally verified** into
`constraints/truck-constraints.json` (keeping its `entries: []` shape), with
your name in `verified_by`, today's date, and the source. Per-entry example
schema is in `constraints/README.md`. Rules:

- Tighten only; you cannot clear an OSM restriction.
- At least one of `maxweight_tonnes` / `maxheight_m`; provenance mandatory.
- Unknown fields rejected; duplicate way ids rejected.
- Delete (or re-date) stale entries later — a stale entry is worse than none.

Then:

```console
cargo run --release -p cli -- constraints --file constraints/truck-constraints.json --sqlite data/constraints.sqlite
cargo run --release -p cli -- solve --problem data/demo/console-test-problem.json --out data/demo/solution.json \
  --osm data/osm/nsw-latest.osm.pbf --ch --constraints constraints/truck-constraints.json \
  --hgv --maxweight 42.5 --maxheight 4.3
```

### Step 5 — Re-audit and confirm

```console
cargo run --release -p cli -- audit --problem data/demo/console-test-problem.json \
  --solution data/demo/solution.json --osm data/osm/nsw-latest.osm.pbf \
  --ch --hgv --maxweight 42.5 --maxheight 4.3 --out-dir data/demo
```

Expect the flagged count to shrink by the number of ways you entered: before
was **1,170 ways with no limit data**. The curated file cannot add data to the
graph; it can only tighten — the audit will still report untagged ways, and
that residual list **is** your legal exposure: leave it in operations.

### Step 6 — Record the check

Write (or update) an operations record: date, vehicle profile, extract used,
who checked which way, which sources, and the findings file paths. Keep it
out of this repo — the repo ships no verified limits by design.

## Source list

- NSW State Roads Vertical Clearances (CC-BY, Oct 2026) — bridge clearances over State Roads, via CKAN API: https://opendata.transport.nsw.gov.au/data/dataset/nsw-state-roads-vertical-clearances
- Linkt vehicle height limits (Sydney toll roads): https://www.linkt.com.au/help/travelling-toll-roads/vehicle-height-limits/sydney
- Transport for NSW — 4.6m high vehicles / heavy vehicles road access: https://www.transport.nsw.gov.au/operations/roads-and-waterways/business-and-industry/heavy-vehicles
- NHVR (National Heavy Vehicle Regulator) — notices and OSOM: https://www.nhvr.gov.au/
- TfNSW heavy vehicle road manager (NSW road access & bridge registers): via https://www.transport.nsw.gov.au/operations/roads-and-waterways/business-and-industry/heavy-vehicles
- Council bridge load registers — contact the council responsible for local roads
- On-site signage at the choke point — the last word at the location
- Heavy Vehicle (Mass, Dimension and Loading) National Regulation — 4.3 m general height limit: legislation.nsw.gov.au

## Honesty notes

- The draft entries came from published sources only, marked as AI-assisted
  desktop research — they are **not** on-site verification and must not be
  treated as legal sign-off.
- Bridge load limits for the 92 bridge ways remain unverified; there is no
  public machine-readable per-bridge register for NSW.
- This audit covers the synthetic demo problem only. A real dispatch needs
  the same audit run against *its* plan and OSM extract.
- The audit report is never legal verification — your operations folder and
  the signatories on the entries are.
