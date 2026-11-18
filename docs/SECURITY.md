# Security and dependency audit

Date: 2026-11-07. Run from the repository root:

```console
cargo audit
```

`cargo audit` scans `Cargo.lock`, which covers every target platform and feature. Configuration lives in
[`.cargo/audit.toml`](../.cargo/audit.toml) (this cargo-audit build reads
the config from `.cargo/`, not the repository root); this page records why
each entry exists. Currently the audit exits 0: two advisories are ignored for
the reasons below, and four informational warnings remain (unmaintained or
unsound crates, none of them vulnerabilities).

## Ignored advisories

| Advisory | Crate | Dependency path | Why ignored |
|---|---|---|---|
| RUSTSEC-2023-0071 (medium) | rsa 0.9.10 | none - `Cargo.lock` only, via feature-gated resolution | Not reachable from any workspace member on any target (`cargo tree --target all -i rsa` finds nothing), and upstream never published a fixed version. |
| RUSTSEC-2026-0459 (medium 6.8) | libsqlite3-sys 0.30.1 (bundled SQLite 3.53.2) | `api -> sqlx 0.8.6 -> sqlx-sqlite 0.8.6`; `cli, geo-au -> rusqlite 0.32.1` (shared `links = "sqlite3"` copy) | Patched in libsqlite3-sys >=0.39.0 (SQLite 3.54.x), but **no resolution can reach it today**: `sqlx-sqlite` 0.9.0 (the latest sqlx) caps libsqlite3-sys at `<0.38.0`, and cargo's `links = "sqlite3"` permits only one `libsqlite3-sys` in the graph, so rusqlite 0.41/0.42 (sys 0.39/0.40) conflicts with sqlx's copy. Upgrading sqlx to 0.9.0 does not help. Exposure here: the vulnerability returns uninitialised page-cache bytes when a process opens a database **together with a hostile `-wal` sibling** whose page size differs; both SQLite files this project opens are operator-controlled (its own `api.sqlite`, and the G-NAF flat-file import from official government sources), so the attack requires an attacker already placed inside the operator's file tree. Re-check on every sqlx release; delete this ignore when sqlx adopts libsqlite3-sys >=0.39.0 (and let rusqlite follow the same line so `links` stays satisfiable). |

## Warnings (no fix available)

These do not fail `cargo audit`; they are recorded so an upgrade is checked
against them.

| Advisory | Crate | Path | Note |
|---|---|---|---|
| RUSTSEC-2026-0186 (unsound) | memmap2 0.5.10 | `routing -> osmpbf 0.3.8` | osmpbf 0.3.8 is the latest release and still requires memmap2 0.5. Re-check on every osmpbf release. |
| RUSTSEC-2026-0370, RUSTSEC-2026-0371, RUSTSEC-2026-0372 | zvariant 5.15.1, zbus_names 4.3.5, zvariant_derive 5.15.1 | `console -> eframe 0.37 -> egui-winit -> accesskit_winit -> accesskit_unix -> zbus` | Unmaintained-crate notices (2026-10-16) covering the zbus serialisation family. Linux-only: `cargo tree -i zbus` prints nothing without `--target all`, so it is not compiled for the Windows target. Re-check on accesskit/egui releases. |

## History

- **2026-11-07** — eframe/egui upgraded 0.31 -> 0.37, keeping the glow
  renderer (not the new wgpu default) to keep the dependency tree small.
  This cleared the quick-xml advisories (RUSTSEC-2026-0194/0195) and the
  paste and ttf-parser unmaintained warnings, and pruned the whole wgpu
  stack; the zbus-family versions were updated but remain flagged.
- **2026-11-12** - clean-checkout audit re-check for the console work surfaced
  RUSTSEC-2026-0459 (libsqlite3-sys via C SQLite CVE-2026-119375, issued
  2026-11-09, memory-exposure / uninitialised page-cache read from a crafted
  DB + `-wal` sibling pair). sqlx 0.9.0 was tried speculatively and does not
  help (caps libsqlite3-sys at <0.38.0); the attempted rusqlite 0.42/sqlx 0.9
  bump was reverted to keep the graph buildable. The advisory is ignored with
  a dated reason above; no operator-facing path opens a second-party SQLite
  file. `cargo audit` exits 0 with 2 ignores.
- **2026-11-09** - audit re-check: still 1 ignore (rsa,
  unreachable, upstream unfixed) and 4 informational warnings (zbus family
  on the Linux-only accessibility path, `memmap2` 0.5.10 via `osmpbf`
  0.3.8). No new advisories; `cargo audit --no-fetch` exits 0.

## Rules

- Never add a `.cargo/audit.toml` ignore without a dated reason here.
- After bumping egui/eframe or osmpbf, delete the corresponding ignore,
  re-run `cargo audit`, and update this page.
- The audit does not replace the operator checks in
  [TRUCK-CONSTRAINTS.md](TRUCK-CONSTRAINTS.md); it only covers crate
  vulnerabilities.
