# `sim` — seeded synthetic network generator

Generates realistic-but-synthetic Australian delivery and supermarket networks for
development and benchmarking. Byte-identical output per seed.

- Five capitals, four supermarket banners (`Coles`/`Woolworths`/`ALDI`/`IGA`) plus
  `General` custom stops, suburb-scale store clusters, and fringe depots. Sydney
  points are kept on land: stores and depots that would land in the Tasman Sea are
  mirrored inland.
- `--christmas-peak` profile (higher demand, wider receiving windows).
- Counts clamped to ≥ 1; one driver per vehicle.

Used by `gen-sim`. No real store addresses are shipped.
