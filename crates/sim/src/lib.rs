//! Synthetic scenario generator for Australian grocery distribution networks.
//!
//! Generates reproducible (seeded) [`domain::Problem`] instances that mimic a
//! retail distribution network around one of the five mainland state capitals:
//! stores clustered by suburb, distribution centres on the urban fringe, and a
//! mixed-temperature fleet with one driver per vehicle.
//!
//! All generated places, names and parameters are synthetic fixtures, not real
//! addresses or researched operating data.

use domain::{Chain, Problem};

/// One of the five mainland state capitals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum City {
    Sydney,
    Melbourne,
    Brisbane,
    Perth,
    Adelaide,
}

impl City {
    /// Every city, in a stable order.
    pub const ALL: [Self; 5] = [
        Self::Sydney,
        Self::Melbourne,
        Self::Brisbane,
        Self::Perth,
        Self::Adelaide,
    ];

    /// The city name as used on the command line and in generated names.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sydney => "sydney",
            Self::Melbourne => "melbourne",
            Self::Brisbane => "brisbane",
            Self::Perth => "perth",
            Self::Adelaide => "adelaide",
        }
    }
}

/// Knobs for [`generate`].
#[derive(Clone, Copy, Debug)]
pub struct SimOptions {
    /// The banner whose stores are generated.
    pub chain: Chain,
    /// The city the network is generated around.
    pub city: City,
    /// Number of stores.
    pub stores: usize,
    /// Number of distribution centres.
    pub depots: usize,
    /// Number of vehicles (one driver is generated per vehicle).
    pub vehicles: usize,
    /// Random seed; the same seed always produces the same problem.
    pub seed: u64,
}

/// Generates a deterministic synthetic problem from `options`.
pub fn generate(_options: &SimOptions) -> Problem {
    unimplemented!("sim subagent fills this in")
}
