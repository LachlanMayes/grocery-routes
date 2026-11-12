# `console` - egui operator client

egui operator client. Embeds the API in-process — one command:

```
cargo run -p console
```

Embedded API listens on `127.0.0.1:8080` (SQLite `data/demo/api.sqlite`,
created if missing). Flags: `--db <path>`, `--addr <addr>`, `--no-embedded-api`
to talk to a standalone API instead; if the port is already in use the console
falls back to the external API automatically. Covers import via a native Windows file
dialog (Browse…) or a typed/pasted path, or pasted JSON, plus solve,
solutions, KPIs and events. The Solutions tab shows a structured view -
stat cards, a per-route table with a named stop timeline, an unassigned-stores
list, and a collapsible raw-JSON debug section. The KPIs tab shows card-style
stats, a serve-rate progress bar and a per-route bar chart (Cost or
Distance). Recent-problem/solution dropdowns (backed by
`GET /problems` and `GET /solutions`, Refresh button to re-fetch) mean IDs rarely need
copying by hand; IDs still auto-fill across tabs on success, and each success row offers a
copy button. See `docs/API.md`.

## Map tab

The Map tab draws a solution's routes on OpenStreetMap raster tiles via the
`walkers` crate (`HttpTiles` + `sources::OpenStreetMap`). Load a solution by
ID (or from the recent-solutions dropdown), and the camera auto-fits the
solution's extent; each route is a coloured polyline through its stops, with
a filled square for depots and a dot for stores. The coloured `R1..Rn`
chips above the map toggle route visibility, and **Fit extents** re-centres
the camera.

Zoom: the **Zoom −** / **Zoom +** buttons step the zoom level (shown beside
them), the mouse wheel zooms without holding Ctrl, double-click zooms in and
right-double-click zooms out; drag still pans.

Two honest caveats:

- Legs are **straight lines** between stops, not road-following geometry.
  Drawing true road geometry would need a new API endpoint that returns the
  routed polyline for each leg (the OSM router's `CachedMatrix` holds
  distances/durations, not geometry); that is follow-up work.
- Tiles are downloaded from `tile.openstreetmap.org` at runtime, so the tab
  needs network access and must respect the OpenStreetMap tile usage policy
  (attribution is shown under the map). No tiles are cached to disk beyond
  the in-memory session.
- Route lines follow real roads **only when the solution was solved with an
  OSM extract** (`--osm`): the console then fetches `GET
  /solutions/{id}/geometry` and draws the road polylines. Otherwise (a
  haversine-solved plan) it falls back to straight lines between stops. The
  geometry endpoint runs one point-to-point route search per leg, so a map
  load on a big plan can take a few seconds.
