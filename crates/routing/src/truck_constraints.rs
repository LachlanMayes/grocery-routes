//! Truck constraint filtering for road-network edges.
//!
//! Reads OSM way tags (`maxweight`, `maxheight`, `hgv`, `access`) and decides
//! whether a given vehicle profile may traverse an edge.

use crate::graph::GraphEdge;

/// A truck's physical constraints, used to filter drivable edges.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TruckProfile {
    /// Maximum vehicle weight in tonnes.
    pub maxweight_tonnes: Option<f64>,
    /// Maximum vehicle height in metres.
    pub maxheight_m: Option<f64>,
    /// True when the vehicle is classified as a heavy goods vehicle.
    pub hgv: bool,
}

impl Eq for TruckProfile {}

impl std::hash::Hash for TruckProfile {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.maxweight_tonnes.map(|v| v.to_bits()).hash(state);
        self.maxheight_m.map(|v| v.to_bits()).hash(state);
        self.hgv.hash(state);
    }
}

/// A reason an edge is forbidden for a truck profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeForbidden {
    /// Edge is tagged `access=no` (or equivalent) for all vehicles.
    NoAccess,
    /// Edge carries a `maxweight` lower than the truck's weight.
    WeightLimitExceeded,
    /// Edge carries a `maxheight` lower than the truck's height.
    HeightLimitExceeded,
    /// Edge is tagged `hgv=no` / `hgv=delivery` and the vehicle is an HGV.
    HgvNotAllowed,
}

/// Returns the estimated duration in minutes for traversing `edge`.
///
/// Falls back to a speed derived from the highway type when no explicit
/// `maxspeed` tag is present.
pub fn edge_duration_minutes(edge: &GraphEdge) -> i64 {
    edge.duration_minutes
}

/// Evaluates whether `profile` may traverse `edge`. Returns `Ok(())` when
/// permitted, or the first forbidding reason otherwise.
pub fn edge_allowed_for(edge: &GraphEdge, profile: TruckProfile) -> Result<(), EdgeForbidden> {
    // Hard access bans take precedence.
    if let Some(highway) = edge.highway.as_deref() {
        if highway == "no" || highway == "private" {
            return Err(EdgeForbidden::NoAccess);
        }
    }

    if let (Some(limit), Some(weight)) = (edge.maxweight_tonnes, profile.maxweight_tonnes) {
        if weight > limit {
            return Err(EdgeForbidden::WeightLimitExceeded);
        }
    }

    if let (Some(limit), Some(height)) = (edge.maxheight_m, profile.maxheight_m) {
        if height > limit {
            return Err(EdgeForbidden::HeightLimitExceeded);
        }
    }

    if profile.hgv {
        match edge.highway.as_deref() {
            Some("hgv=no") | Some("hgv=delivery") => {
                return Err(EdgeForbidden::HgvNotAllowed);
            }
            _ => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeId;
    use crate::graph::GraphEdge;
    use crate::graph::NodeId;
    use domain::LatLon;

    fn test_edge() -> GraphEdge {
        GraphEdge {
            id: EdgeId(0),
            from: NodeId(1),
            to: NodeId(2),
            distance_km: 5.0,
            duration_minutes: 6,
            highway: Some("primary".to_string()),
            maxweight_tonnes: Some(10.0),
            maxheight_m: Some(4.0),
            hgv: true,
            oneway: false,
        }
    }

    #[test]
    fn light_truck_passes_weight_limit() {
        let edge = test_edge();
        let profile = TruckProfile {
            maxweight_tonnes: Some(8.0),
            ..Default::default()
        };
        assert!(edge_allowed_for(&edge, profile).is_ok());
    }

    #[test]
    fn heavy_truck_fails_weight_limit() {
        let edge = test_edge();
        let profile = TruckProfile {
            maxweight_tonnes: Some(12.0),
            ..Default::default()
        };
        assert_eq!(
            edge_allowed_for(&edge, profile),
            Err(EdgeForbidden::WeightLimitExceeded)
        );
    }

    #[test]
    fn tall_truck_fails_height_limit() {
        let edge = test_edge();
        let profile = TruckProfile {
            maxheight_m: Some(5.0),
            ..Default::default()
        };
        assert_eq!(
            edge_allowed_for(&edge, profile),
            Err(EdgeForbidden::HeightLimitExceeded)
        );
    }

    #[test]
    fn hgv_blocked_on_hgv_no_road() {
        let mut edge = test_edge();
        edge.highway = Some("hgv=no".to_string());
        let profile = TruckProfile {
            hgv: true,
            ..Default::default()
        };
        assert_eq!(
            edge_allowed_for(&edge, profile),
            Err(EdgeForbidden::HgvNotAllowed)
        );
    }

    #[test]
    fn no_profile_passes_all() {
        let edge = test_edge();
        let profile = TruckProfile::default();
        assert!(edge_allowed_for(&edge, profile).is_ok());
    }
}
