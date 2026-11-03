//! Coordinates and great-circle distance.

use serde::{Deserialize, Serialize};

/// A WGS84 latitude/longitude pair, in decimal degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl LatLon {
    /// Builds a coordinate from decimal degrees.
    pub const fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }
}

/// Great-circle distance in kilometres (haversine, mean Earth radius).
pub fn haversine_km(a: LatLon, b: LatLon) -> f64 {
    const EARTH_RADIUS_KM: f64 = 6371.0088;
    let phi1 = a.lat.to_radians();
    let phi2 = b.lat.to_radians();
    let delta_phi = (b.lat - a.lat).to_radians();
    let delta_lambda = (b.lon - a.lon).to_radians();
    let h = (delta_phi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (delta_lambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().atan2((1.0 - h).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_distance_for_identical_points() {
        let point = LatLon::new(-31.95, 115.86);
        assert!(haversine_km(point, point) < 1e-9);
    }

    #[test]
    fn sydney_to_melbourne_is_about_714_km() {
        let sydney = LatLon::new(-33.8688, 151.2093);
        let melbourne = LatLon::new(-37.8136, 144.9631);
        let km = haversine_km(sydney, melbourne);
        assert!((km - 714.0).abs() < 5.0, "got {km} km");
    }

    #[test]
    fn one_degree_of_latitude_is_about_111_km() {
        let a = LatLon::new(-35.0, 149.0);
        let b = LatLon::new(-36.0, 149.0);
        let km = haversine_km(a, b);
        assert!((km - 111.2).abs() < 1.0, "got {km} km");
    }
}
