# Security and dependency audit

Date: 2026-11-07. Run from the repository root:

```console
cargo audit
```

`cargo audit` scans `Cargo.lock`, which covers every platform and feature,
not just this machine's build. Configuration lives in
[`.cargo/audit.toml`](../.cargo/audit.toml) (this cargo-audit build reads
the config from `.cargo/`, not the repository root); this page records why
each entry exists. Currently the audit exits 0: one advisory is ignored for
the reason below, and four informational warnings remain (unmaintained or
unsound crates, none of them vulnerabilities).

## Ignored advisories

| Advisory | Crate | Dependency path | Why ignored |
|---|---|---|---|
| RUSTSEC-2023-0071 (medium) | rsa 0.9.10 | none — `Cargo.lock` only, via feature-gated resolution | Not reachable from any workspace member on any target (`cargo tree --target all -i rsa` finds nothing), and upstream never published a fixed version. |

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
- **2026-11-06** — first audit recorded: 3 advisories ignored (quick-xml,
  rsa) and 6 warnings noted.

## Rules

- Never add a `.cargo/audit.toml` ignore without a dated reason here.
- After bumping egui/eframe or osmpbf, delete the corresponding ignore,
  re-run `cargo audit`, and update this page.
- The audit does not replace the operator checks in
  [TRUCK-CONSTRAINTS.md](TRUCK-CONSTRAINTS.md); it only covers crate
  vulnerabilities.
