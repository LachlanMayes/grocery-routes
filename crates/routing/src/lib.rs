//! Routing engine for M3: OSM PBF ingest, road-network graph, truck-aware
//! shortest paths, and cached distance/duration matrices.
//!
//! The solver currently plans on haversine distances with a single planning
//! speed (`domain::Vehicle::avg_speed_kmh`). M3 replaces this with real road
//! network data from OpenStreetMap:
//!
//! - Ingest AU OSM extracts (`.osm.pbf`) via [`osmpbf`]
//! - Build a directed, tagged graph of drivable ways
//! - Filter edges by truck constraints (`maxweight`, `maxheight`, `hgv`)
//! - Compute shortest paths (Dijkstra) for distance/duration matrices
//! - Cache matrices keyed by problem + vehicle profile

pub mod cached_matrix;
pub mod curated;
pub mod graph;
pub mod osm_ingest;
pub mod router;
pub mod timed;
pub mod truck_constraints;

pub use cached_matrix::{CachedMatrix, MatrixEntry, MatrixKey};
pub use curated::{CuratedConstraints, CuratedEntry, CuratedError};
pub use graph::{EdgeId, Graph, NodeId};
pub use router::{RouteResult, Router};
pub use timed::{CurfewContext, LegStep, TimedLeg, earliest_leg, plain_leg};
pub use truck_constraints::TruckProfile;
