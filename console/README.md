# `console` - egui operator client

egui operator client. Embeds the API in-process — one command:

```
cargo run -p console
```

Embedded API listens on `127.0.0.1:8080` (SQLite `data/demo/api.sqlite`,
created if missing). Flags: `--db <path>`, `--addr <addr>`, `--no-embedded-api`
to talk to a standalone API instead; if the port is already in use the console
falls back to the external API automatically. Covers custom general route setup
(**Set Up Route** with depot/place address geocoding via G-NAF or OpenStreetMap
Nominatim plus **Interactive Map Picker**, or optional **Synthetic Scenario
Generator** presets), importing via a native Windows file dialog (Browse…) or a
typed/pasted path, or pasted JSON, plus solve, solutions, KPIs and events. The
Solutions tab shows a structured view — stat cards, a per-route table with a
named stop timeline, an unassigned-stores list, jump actions, and a collapsible
raw-JSON debug section. The KPIs tab provides an analytics dashboard with
executive summary cards, multi-mode per-route charts (**Cost Breakdown**,
**Distance**, **Duration**, **Stops**, **Load Utilization %**), a **Dispatch
Timeline (Gantt)** chart, cost & load composition breakdowns, **Solve History
Comparison**, a sortable per-route leaderboard, an unassigned demand impact
panel, and CSV/JSON/clipboard exports. Recent-problem/solution dropdowns (backed by
`GET /problems` and `GET /solutions`, Refresh button to re-fetch) mean IDs rarely need
copying by hand; IDs still auto-fill across tabs on success, and each success row offers a
copy button. See `docs/API.md`.

## Map tab

The Map tab draws a solution's routes on OpenStreetMap raster tiles via the
`walkers` crate (`HttpTiles` + `sources::OpenStreetMap`). Load a solution by
ID (or from the recent-solutions dropdown), and the camera auto-fits the
solution's extent; each route is a coloured polyline through its stops, with
depot, served-store, and unassigned-store diamond markers, collision-avoided
on-map node info pills (**Show node info**), hover tooltips, click-to-select +
**Inspect:** picker backed by a **Selected Node** detail card (coordinates,
address, timezone, demand, receiving windows, and assigned stops). The coloured
`R1..Rn` chips above the map toggle route visibility, and **Fit extents**
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
