# `sim` — seeded synthetic network generator

Generates realistic-but-synthetic Australian supermarket networks for
development and benchmarking. Byte-identical output per seed.

- Five capitals, four banners (Coles/Woolworths/ALDI/IGA), suburb-scale
  store clusters, fringe depots.
- `--christmas-peak` profile (higher demand, wider receiving windows).
- Counts clamped to ≥ 1; one driver per vehicle.

Used by `gen-sim`. No real store addresses are shipped.
