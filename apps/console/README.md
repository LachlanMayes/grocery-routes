# `console` - egui operator client

egui operator client. Embeds the API in-process — one command:

```
cargo run -p console
```

Embedded API listens on `127.0.0.1:8080` (SQLite `data/demo/api.sqlite`,
created if missing). Flags: `--db <path>`, `--addr <addr>`, `--no-embedded-api`
to talk to a standalone API instead; if the port is already in use the console
falls back to the external API automatically. Covers import/solve/solutions/
KPIs/events. Auto-fills IDs across tabs on success and offers `Copy ID`
buttons. See `docs/API.md`.
