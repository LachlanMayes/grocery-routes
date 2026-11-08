# `console` — egui operator client

egui client for the local API (`http://127.0.0.1:8080`). Needs two processes:

```
cargo run -p api -- --db data/demo/api.sqlite
cargo run -p console
```

Covers import/solve/solutions/KPIs/events. Auto-fills IDs across tabs on
success and offers `Copy ID` buttons. See `docs/API.md`.
