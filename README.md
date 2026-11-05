# grocery-routes

Route + delivery planning for Australian supermarket distribution — Coles,
Woolworths, ALDI and IGA/Metcash networks. Rust workspace, built from scratch.

- Roadmap, milestones and current gate outcomes: [docs/PLAN.md](docs/PLAN.md)
- Historical milestone reports: [docs/M0-REPORT.md](docs/M0-REPORT.md) ·
  [docs/M1-REPORT.md](docs/M1-REPORT.md)

## Quickstart

```console
cargo run -p cli -- gen-sim --chain coles --city sydney --stores 40 --depots 3 --vehicles 12 --seed 42 --out data/demo/problem.json
cargo run -p cli -- solve --problem data/demo/problem.json --out data/demo/solution.json
cargo run -p cli -- export --problem data/demo/problem.json --solution data/demo/solution.json --out-dir data/demo
```

`data/demo/` is gitignored; `data/solomon/C101.txt` is tracked so the C101
calibration gates run in a clean checkout. Everything generated is synthetic; no
real store addresses are shipped.

## Checks

```console
cargo test --workspace
cargo clippy --workspace --all-targets
cargo bench -p solver --no-run
```

The workspace currently builds with warnings under `clippy::all` (warn level);
it is not enforced with `-D warnings`.

## Layout

| Path | Role |
|---|---|
| `crates/domain` | timeline model + validation rules (the contract) |
| `crates/sim` | seeded synthetic network generator |
| `crates/solver` | haversine matrix, greedy insertion, local search, LNS/ALNS and C101 gates |
| `crates/routing` | OSM PBF ingest, directed graph, truck constraints, cached matrices |
| `crates/cli` | `gen-sim`, `solve`, `export` |
| `crates/api` | axum + SQLite API, SSE solve events, import/solve/solution/KPI endpoints |
| `apps/console` | egui operator console |
| `docs/` | roadmap, plan status, milestone reports |
