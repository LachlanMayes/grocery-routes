# `console` - TMS Control Tower (`egui` operator client)

`egui` operator client (`TMS Control Tower · Orders · Cubing · Dispatch · ESG`). Embeds the API in-process — one command:

```
cargo run -p console
```

Embedded API listens on `127.0.0.1:8080` (SQLite `data/demo/api.sqlite`,
created if missing). Flags: `--db <path>`, `--addr <addr>`, `--no-embedded-api`
to talk to a standalone API instead; if the port is already in use the console
falls back to the external API automatically. Navigation spans seven TMS workspaces:

- **Orders & Network**: Custom general route setup (**Set Up Route** with 1-click
  **Solve Route Now**, **Load into Builder** for saved problems, depot/place
  address geocoding via G-NAF or OpenStreetMap Nominatim, quick window presets +
  `HH:MM` display, **Bulk Paste…**, auto-replacement of the starter placeholder
  on first real stop, **Auto-Fit Shifts to Windows** + inline driver shift
  editing, **Rate Card & Meta** with four Australian **Carrier Rate Card
  Presets** (`Metro Van`, `12-Pallet Rigid`, `22-Pallet Semi`, `Linehaul
  B-Double`) plus a live **Benchmark 100 km / 4 hr run** cost preview, and
  **Interactive Map Picker**, alongside collapsible **Synthetic Scenario
  Generator** presets and **File Import / Raw Problem JSON**).
- **Load Planner**: Physical truck capacity optimization (bay
  length/width/height/volume/payload inputs, handling units, 90°/hybrid packing,
  3-view cargo bay visualizer with LIFO stop badges & CoG elevation, and
  fleet/solver sync).
- **Optimizer**: Solve triggers (`local_search` on by default, `LNS`,
  `Contraction Hierarchy`) with OSM `.pbf` auto-discovery and HGV truck limits.
- **Dispatch & BOL**: Auto-loads on dropdown selection with stat cards, a live
  **Chain of Responsibility (CoR)** validation banner (`capacity`, receiving
  windows, driver shifts & NHVR fatigue), **Copy Route Manifest**, **Export
  Schedule CSV…**, and **Export Run Sheet / BOL (HTML)…** (printable driver
  run sheet & Bill of Lading with CoR pre-departure verification checklist and
  per-stop ePOD signature blocks). Operators can also manually dispatch via
  inline **▲** / **▼** stop re-sequencing, **Unassign**, and **+ Assign to
  Route N** actions that recalculate route distance/ETAs and sync **Dispatch &
  BOL**, **Freight KPIs**, and **Fleet Map** immediately.
- **Freight KPIs**: Executive summary cards (serve rate, **OTIF & Dock Wait**,
  total cost, $/km, $/stop, $/unit, active fleet %, total/avg duration, peak
  compartment load %, **Carbon & Fuel (ESG)** using Australian NGA diesel/CO₂e
  factors), multi-mode per-route charts (**Cost Breakdown**, **Distance**,
  **Duration**, **Stops**, **Load Utilization %**), a **Dispatch Timeline
  (Gantt)** chart, cost & load composition breakdowns, **Solve History
  Comparison**, a sortable per-route leaderboard, an unassigned demand impact
  panel, and CSV/JSON/clipboard exports.
- **Fleet Map**: Interactive road/route map (details below).
- **Control Tower**: Live SSE event stream.

Recent-problem/solution dropdowns (backed by `GET /problems` and
`GET /solutions`, Refresh button to re-fetch) mean IDs rarely need copying by
hand; IDs still auto-fill across tabs on success, and each success row offers
a copy button. See `docs/API.md`.

## Fleet Map tab

The **Fleet Map** tab draws a solution's routes on OpenStreetMap raster tiles via the
`walkers` crate (`HttpTiles` + `sources::OpenStreetMap`). Load a solution by
ID (or from the recent-solutions dropdown), and the camera auto-fits the
solution's extent; each route is a coloured polyline with directional arrowheads
through its stops, featuring labelled depot (`D`), numbered served-stop (`1..N`),
and unassigned-store (`!`) diamond markers, collision-avoided on-map node info pills
(**Show node info**), hover tooltips, click-to-select + **Inspect:** picker backed
by a **Selected Node** detail card (coordinates, address, timezone, demand,
receiving windows, leg km, cumulative load, wait times, unassigned root-cause
diagnosis, **◀ Prev** / **Next ▶** stop navigation, and **Truck Load** jump). The
coloured `R1..Rn` chips above the map toggle route visibility, and **Fit extents**
re-centres the camera.

Zoom: the **Zoom −** / **Zoom +** buttons step the zoom level (shown beside
them), the mouse wheel zooms without holding Ctrl, double-click zooms in and
right-double-click zooms out; drag still pans.

Two honest caveats:

- Tiles are downloaded from `tile.openstreetmap.org` at runtime, so the tab
  needs network access and must respect the OpenStreetMap tile usage policy
  (attribution is shown under the map). No tiles are cached to disk beyond
  the in-memory session.
- Route lines follow real roads **only when the solution was solved with an
  OSM extract** (`--osm`): the console then fetches `GET
  /solutions/{id}/geometry` and draws road polylines with tethered stop
  connectors (plus dashed fallback legs when a leg could not be routed).
  Otherwise (a haversine-solved plan) it falls back to straight lines between
  stops. The geometry endpoint runs one point-to-point route search per leg, so
  a map load on a big plan can take a few seconds.
