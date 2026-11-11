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
list, and a collapsible raw-JSON debug section. Recent-problem/solution dropdowns (backed by
`GET /problems` and `GET /solutions`, Refresh button to re-fetch) mean IDs rarely need
copying by hand; IDs still auto-fill across tabs on success, and each success row offers a
copy button. See `docs/API.md`.
