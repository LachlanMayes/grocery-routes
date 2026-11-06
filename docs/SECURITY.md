# Security and dependency audit

Date: 2026-11-06. Run from the repository root:

```console
cargo audit
```

`cargo audit` scans `Cargo.lock`, which covers every platform and feature,
not just this machine's build. Configuration lives in
[`.cargo/audit.toml`](../.cargo/audit.toml) (this cargo-audit build reads
the config from `.cargo/`, not the repository root); this page records why
each entry exists.
Currently the audit exits 0: three advisories are ignored for the reasons
below, and six informational warnings (unmaintained crates) remain, all
transitives of the desktop UI stack or `osmpbf`.

## Ignored advisories

| Advisory | Crate | Dependency path | Why ignored |
|---|---|---|---|
| RUSTSEC-2026-0194, RUSTSEC-2026-0195 (high) | quick-xml 0.30.0 | `console -> eframe 0.31 -> egui-winit -> accesskit_winit -> accesskit_unix -> zbus -> zbus_xml` | **Linux-only.** `cargo tree -i zbus` prints nothing without `--target all`: the accessibility path is not compiled for the Windows target. Cleared by an egui/eframe upgrade; re-check after one. |
| RUSTSEC-2023-0071 (medium) | rsa 0.9.10 | none — `Cargo.lock` only, via feature-gated resolution | Not reachable from any workspace member on any target (`cargo tree --target all -i rsa` finds nothing), and upstream never published a fixed version. |

## Warnings (no fix available)

These do not fail `cargo audit`; they are recorded so an upgrade is checked
against them.

| Advisory | Crate | Path | Note |
|---|---|---|---|
| RUSTSEC-2026-0186 (unsound) | memmap2 0.5.10 | `routing -> osmpbf 0.3.8` | osmpbf 0.3.8 is the latest release and still requires memmap2 0.5. Re-check on every osmpbf release. |
| RUSTSEC-2024-0436 | paste 1.0.15 | `console -> eframe -> accesskit_windows` | Unmaintained proc-macro inside the egui stack. |
| RUSTSEC-2026-0192 | ttf-parser 0.25.1 | `console -> eframe -> epaint -> ab_glyph` | Unmaintained font parser inside the egui stack. |
| RUSTSEC-2026-0370, RUSTSEC-2026-0371, RUSTSEC-2026-0372 | zvariant 4.2.0, zbus_names 3.0.0, zvariant_derive 4.2.0 | Linux-only zbus path as above | Cleared by the same egui/eframe upgrade. |

## Rules

- Never add a `.cargo/audit.toml` ignore without a dated reason here.
- After bumping egui/eframe or osmpbf, delete the corresponding ignore,
  re-run `cargo audit`, and update this page.
- The audit does not replace the operator checks in
  [TRUCK-CONSTRAINTS.md](TRUCK-CONSTRAINTS.md); it only covers crate
  vulnerabilities.
