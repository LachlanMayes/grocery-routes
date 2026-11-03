# grocery-routes

Route + delivery planning for Australian supermarket distribution — Coles,
Woolworths, ALDI and IGA/Metcash networks. Rust workspace, built from scratch.

- Roadmap and gates: [docs/PLAN.md](docs/PLAN.md)
- Current state and evidence: [docs/M0-REPORT.md](docs/M0-REPORT.md)

## Quickstart

```console
cargo run -p cli -- gen-sim --chain coles --city sydney --stores 40 --depots 3 --vehicles 12 --seed 42 --out data/demo/problem.json
cargo run -p cli -- solve --problem data/demo/problem.json --out data/demo/solution.json
cargo run -p cli -- export --problem data/demo/problem.json --solution data/demo/solution.json --out-dir data/demo
```

`data/` is gitignored. Everything generated is synthetic; no real store
addresses are shipped.

## Checks

```console
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo bench -p solver --no-run
```

## Layout

| Path | Role |
|---|---|
| `crates/domain` | timeline model + validation rules (the contract) |
| `crates/sim` | seeded synthetic network generator |
| `crates/solver` | route construction (greedy insertion at M0) |
| `crates/cli` | `gen-sim`, `solve`, `export` |
| `docs/` | plan + milestone reports |
