//! G-NAF address ingest and geocoding for Australian addresses.
//!
//! [G-NAF](https://data.gov.au) (Geoscape's Geocoded National Address File)
//! ships as per-state pipe-separated flat files. [`import_directory`] builds a
//! small SQLite database from an unzipped directory tree of those files, and
//! [`geocode`] / [`reverse`] query it.
//!
//! # What this crate does and does not do
//!
//! - Ingest is **header-driven**: the standard columns are looked up by name
//!   (`ADDRESS_DETAIL_PID`, `NUMBER_FIRST`, `LATITUDE`, ...), so a schema
//!   change fails loudly with the headers it saw.
//! - The query parser is a **simple heuristic**, not a full address grammar:
//!   `"12 Smith St, Sydney NSW 2000"` works; unusual orderings may not.
//! - Import has been exercised against synthetic fixtures in this
//!   repository's own release format, **not** against a full G-NAF release;
//!   see `docs/GNAF.md`.
//!
//! The data itself is licensed under the Open G-NAF EULA (CC BY 4.0-based);
//! attribution is required when redistributing.

pub mod flatfile;
pub mod import;
pub mod query;

pub use import::{ImportStats, import_directory};
pub use query::{AddressMatch, GeocodeQuery, geocode, parse_query, reverse};

/// Opens an imported G-NAF database, with a message that names the path.
pub fn open_database(path: &std::path::Path) -> anyhow::Result<rusqlite::Connection> {
    rusqlite::Connection::open(path).map_err(|error| {
        anyhow::anyhow!(
            "opening G-NAF database {} (run `import-gnaf` first): {error}",
            path.display()
        )
    })
}

/// One address row from the database.
#[derive(Clone, Debug, PartialEq)]
pub struct Address {
    /// G-NAF `ADDRESS_DETAIL_PID`.
    pub pid: String,
    pub number_first: Option<u32>,
    pub number_last: Option<u32>,
    pub street: String,
    pub street_type: Option<String>,
    pub locality: String,
    pub state: String,
    pub postcode: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

impl Address {
    /// True when `number` falls in this address's number range (either
    /// bound may be absent).
    #[must_use]
    pub fn number_matches(&self, number: u32) -> bool {
        match (self.number_first, self.number_last) {
            (Some(first), Some(last)) => (first.min(last)..=first.max(last)).contains(&number),
            (Some(first), None) => first == number,
            (None, Some(last)) => last == number,
            (None, None) => false,
        }
    }

    /// A human-readable single line, e.g. `12-14 Smith Street, Sydney NSW 2000`.
    #[must_use]
    pub fn display(&self) -> String {
        let number = match (self.number_first, self.number_last) {
            (Some(first), Some(last)) if first != last => format!("{first}-{last} "),
            (Some(first), _) | (None, Some(first)) => format!("{first} "),
            (None, None) => String::new(),
        };
        let street_type = self
            .street_type
            .as_deref()
            .map_or(String::new(), |value| format!(" {value}"));
        let postcode = self
            .postcode
            .as_deref()
            .map_or(String::new(), |value| format!(" {value}"));
        format!(
            "{number}{}{street_type}, {} {}{postcode}",
            self.street, self.locality, self.state
        )
    }
}

/// Great-circle distance in kilometres (local copy keeps this crate
/// independent of the routing model).
pub(crate) fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_KM: f64 = 6371.0088;
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let delta_phi = (lat2 - lat1).to_radians();
    let delta_lambda = (lon2 - lon1).to_radians();
    let h = (delta_phi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (delta_lambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().atan2((1.0 - h).sqrt())
}
