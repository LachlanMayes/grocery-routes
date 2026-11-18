# Corridor research - synthetic Sydney plan (2026-11-10)

Desktop research for the corridor used by the demo problem
(`data/demo/console-test-problem.json`, solved with `solve --ch --osm`). It
is a starting point for operator sign-off, **not legal verification**. Nothing
below has been checked on site.

Method: `audit` against the NSW extract (`nsw-latest.osm.pbf`, 258 MB) listed
1,277 traversed ways, 1,170 of which carry no truck-limit tags. Way tags were
collected from the public Overpass API (name, ref, maxweight, maxheight, hgv,
access, bridge, tunnel) and cross-checked against published sources.

## What the plan traverses

| Corridor | Ref | Ways | Role |
|---|---|---|---|
| Hume Highway / Orange Grove Road / Cambridge St | A28 | ~230 | arterials, south-west |
| Princes Highway / General Holmes Drive / Acacia Rd | A1 | ~130 | arterials, south-east |
| Western Motorway + M4 East Tunnel | M4 | ~50 | motorway |
| M5 Motorway | M5 | ~74 | motorway |
| Southern Cross Drive | M1 | ~48 | motorway |
| King Georges Road | A3 | ~50 | arterial |
| Canterbury Road | A34 | ~28 | arterial |
| Western Distributor | A4 | ~12 | motorway |
| Cross City Tunnel | - | 5 ways, 1.3 km | tunnel |
| M4 East Tunnel | M4 | 3 ways, 1.5 km | tunnel |
| Alfords Point Road | A6 | ~10 | arterial |

## Findings (official sources)

### Tunnels

| Tunnel | Published clearance | Source |
|---|---|---|
| Cross City Tunnel (ways 389141181, 244269360, 50887066, 389140309, 782822601) | 4.4 m | TfNSW "Using Sydney's tunnels"; Linkt vehicle height limits |
| M4 East Tunnel (ways 703986355, 704119174, 1226839416) | 5.1 m | TfNSW "M4 Tunnels - 5.1 metres"; Linkt ("WestConnex M4: 5.1m") |
| M5 East tunnel section (way 92024832) | 4.4 m (set to lower of Cooks River 4.4 / Main 4.6) | TfNSW "M5 East Cooks River - 4.4 m", "M5 East Main - 4.6 m" |

The 4.3 m HGV planned for this problem clears every tunnel traversed.

### Bridge clearances — published dataset matched to the corridor (2026-11-10)

Transport for NSW publishes **NSW State Roads Vertical Clearances** (XLSX,
October 2026, CC-BY, 1,214 structures, `https://opendata.transport.nsw.gov.au/data/dataset/nsw-state-roads-vertical-clearances`;
the file is loadable via the CKAN API). Matching its bridge locations by
geometry to all 1,277 traversed ways (within 60 m) gave **93 matches, every
one ≥ 4.36 m**. Thirteen are below 4.6 m:

- 4.36 m UNDER COOKS RIVER → way 4331862
- 4.39 m ON EASTERN DISTRIBUTOR → way 105038171
- 4.41 m HARBOUR GOODS YARD → way 782822601
- 4.42 m WB CR CTY TUNNEL → way 244269360 (Cross City Tunnel)
- 4.43 m ANZAC RD → way 173396362
- 4.48 m MR165-VICTORIA ROAD → way 1227240768
- 4.52 m ON GEN.HOLMES DRIVE → way 383319524
- 4.52 m G HOLMES DRIVE → way 156273325
- 4.54 m ROSLYN ST → way 2956653
- 4.55 m OVER MR173 → way 389141181 (×2, also "OV WILLIAM ST")
- 4.55 m OVER MR173 → way 2956653
- 4.56 m OVER GEORGE'S RIVER → way 147103906

All clear the 4.3 m vehicle, with the tightest at **4.36 m (Cooks River)**.
Caveats: the dataset gives the clearance of bridges over State Roads (i.e.
structures you pass under on State Roads), is not published for
regional/local roads, and carries TfNSW's "information purposes only, no
liability" disclaimer. It does **not** provide bridge load ratings — weight
checks still need the road manager.

### Load limits

No bridge on the traversed corridor appears in restricted-access notices found
in public sources. Under the Heavy Vehicle (Mass, Dimension and Loading)
National Regulation, most heavy vehicles are limited to **4.3 m height**;
general mass limits apply per structure with no signage or notice.

- Princes Highway / General Holmes Drive / Hume Highway: no published
  structure-specific limits found.
- 92 bridge ways were traversed; none surfaced in restricted notices, but
  per-bridge signage was **not** checked on site.

### Not researched

- Per-bridge load registers (Council/RMS bridge registers) for the 92 bridges;
  these are not fully machine-readable and require the road manager.
- Bridge-specific restrictions on A28/A1 arterials (e.g. orange bridges) -
  must be confirmed against TfNSW registers or on site.
- The M5 East tunnel way 92024832: OSM does not distinguish the Cooks River
  tube (4.4 m) from the Main tube (4.6 m); set to 4.4 m pending confirmation.
- The Princes Highway Offramp tunnel (way 173716163): tunnel with no name or
  ref in OSM; no clearance could be attributed to a published source. **Do not
  dispatch a vehicle over 4.3 m on this leg without on-site confirmation.**

## Draft constraint entries

`constraints/draft-desktop-research-2026-11.json` holds the tunnel entries
above, validated with `cargo run -p cli -- constraints --file ...` (9 entries,
format OK). They are marked `verified_by: "desktop-research (unverified on-site); pending operator sign-off"`
and **must not be copied into `truck-constraints.json` without human review**.

## What remains

1. Human sign-off on the 9 draft entries (or their replacement with
   on-site-checked values) before loading them into `truck-constraints.json`.
2. TfNSW bridge register check for the 92 bridge ways (Orange Grove Road,
   Hume Highway, Princes Highway corridors).
3. On-site signage confirmation at the M5 East tunnels and the unnamed
   offramp tunnel (way 173716163).
4. Re-solve with `--constraints constraints/truck-constraints.json` after
   sign-off and re-run `audit` to confirm the flagged set shrinks.

This file documents findings; it is not legal advice or legal verification.
