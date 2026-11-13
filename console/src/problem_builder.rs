//! Interactive problem configuration and scenario builder for the Import tab.
//!
//! Defaults to a clean general custom-route setup (user depot address/location +
//! arbitrary places/stops to visit) while retaining synthetic scenario generation
//! presets (`sim`) in an optional secondary section.

use anyhow::{Context, Result};
use domain::{
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
};

use crate::geocode::{GeocodeMatch, GeocodeTarget, parse_lat_lon_pair};

/// Which sub-table is shown inside the custom problem editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EditorSection {
    Depots,
    #[default]
    Stores,
    Vehicles,
    Costs,
}

/// Interactive click behaviour for the Import-tab map picker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MapPickMode {
    PanZoom,
    SetDepot,
    #[default]
    AddStop,
}

/// UI state for configuring or editing a [`domain::Problem`] in the Import tab.
#[derive(Clone, Debug)]
pub(crate) struct ProblemBuilderState {
    // Optional synthetic generator settings
    pub chain: Chain,
    pub city: sim::City,
    pub stores: usize,
    pub depots: usize,
    pub vehicles: usize,
    pub seed: u64,
    pub christmas_peak: bool,
    pub use_city_timezones: bool,
    pub horizon_start: String,
    pub custom_name: String,

    // Depot quick-setup / address search
    pub depot_query: String,
    pub depot_matches: Vec<GeocodeMatch>,

    // Place / stop quick-add inputs
    pub stop_name_input: String,
    pub stop_address_input: String,
    pub stop_resolved_query: String,
    pub stop_lat: f64,
    pub stop_lon: f64,
    pub stop_chain: Chain,
    pub stop_demand_ambient: u32,
    pub stop_demand_chilled: u32,
    pub stop_demand_frozen: u32,
    pub stop_service_minutes: i64,
    pub stop_window_start_minutes: i64,
    pub stop_window_end_minutes: i64,
    pub stop_matches: Vec<GeocodeMatch>,

    // Geocoding & map picker state
    pub gnaf_db_path: String,
    pub lookup_in_flight: Option<GeocodeTarget>,
    pub lookup_pending_add_stop: bool,
    pub lookup_status: Option<String>,
    pub show_map_picker: bool,
    pub map_pick_mode: MapPickMode,

    // Draft document & active editor table
    pub section: EditorSection,
    pub draft: Problem,
    pub editor_error: Option<String>,
}

impl Default for ProblemBuilderState {
    fn default() -> Self {
        let chain = Chain::General;
        let city = sim::City::Sydney;
        let (center_lat, center_lon) = city_center(city);
        let draft = starter_problem(chain, city, Some("Custom Route".to_string()), false, "");
        Self {
            chain,
            city,
            stores: 12,
            depots: 2,
            vehicles: 4,
            seed: 42,
            christmas_peak: false,
            use_city_timezones: false,
            horizon_start: String::new(),
            custom_name: "Custom Route".to_string(),
            depot_query: String::new(),
            depot_matches: Vec::new(),
            stop_name_input: String::new(),
            stop_address_input: String::new(),
            stop_resolved_query: String::new(),
            stop_lat: center_lat - 0.012,
            stop_lon: center_lon - 0.005,
            stop_chain: Chain::General,
            stop_demand_ambient: 1,
            stop_demand_chilled: 0,
            stop_demand_frozen: 0,
            stop_service_minutes: 15,
            stop_window_start_minutes: 360,
            stop_window_end_minutes: 1080,
            stop_matches: Vec::new(),
            gnaf_db_path: String::new(),
            lookup_in_flight: None,
            lookup_pending_add_stop: false,
            lookup_status: None,
            show_map_picker: false,
            map_pick_mode: MapPickMode::AddStop,
            section: EditorSection::Stores,
            draft,
            editor_error: None,
        }
    }
}

pub(crate) const ALL_CHAINS: [Chain; 5] = [
    Chain::General,
    Chain::Coles,
    Chain::Woolworths,
    Chain::Aldi,
    Chain::Iga,
];

/// Reference center `(lat, lon)` used for initial starter placements.
pub(crate) fn city_center(city: sim::City) -> (f64, f64) {
    match city {
        sim::City::Sydney => (-33.8688, 151.2093),
        sim::City::Melbourne => (-37.8136, 144.9631),
        sim::City::Brisbane => (-27.4698, 153.0251),
        sim::City::Perth => (-31.9505, 115.8605),
        sim::City::Adelaide => (-34.9285, 138.6007),
    }
}

/// Human-friendly city label for dropdowns.
pub(crate) fn city_label(city: sim::City) -> &'static str {
    match city {
        sim::City::Sydney => "Sydney",
        sim::City::Melbourne => "Melbourne",
        sim::City::Brisbane => "Brisbane",
        sim::City::Perth => "Perth",
        sim::City::Adelaide => "Adelaide",
    }
}

fn starter_problem(
    chain: Chain,
    city: sim::City,
    custom_name: Option<String>,
    use_city_timezones: bool,
    horizon_start: &str,
) -> Problem {
    let (lat, lon) = city_center(city);
    let tz = use_city_timezones.then(|| city.iana_timezone().to_string());
    let first_stop_name = match chain {
        Chain::General => "Stop 1".to_string(),
        _ => format!("{chain} Stop 1"),
    };
    let name = custom_name.unwrap_or_else(|| "Custom Route".to_string());
    let mut problem = Problem {
        name,
        depots: vec![Depot {
            id: DepotId(0),
            name: "Main Depot".to_string(),
            location: LatLon::new(lat, lon),
            address: None,
            timezone: tz.clone(),
        }],
        stores: vec![Store {
            id: StoreId(0),
            chain,
            name: first_stop_name,
            location: LatLon::new(lat - 0.012, lon - 0.005),
            address: None,
            demand: Load::new(1, 0, 0),
            service: Minutes::from_minutes(15),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(360),
                Minutes::from_minutes(1080),
            )],
            timezone: tz,
        }],
        vehicles: vec![Vehicle {
            id: VehicleId(0),
            name: "Vehicle 1".to_string(),
            depot: DepotId(0),
            capacity: Load::new(24, 16, 12),
            max_duration: Minutes::from_minutes(600),
            avg_speed_kmh: 50.0,
        }],
        drivers: vec![Driver {
            id: DriverId(0),
            name: "Driver 1".to_string(),
            shifts: vec![Shift::new(
                Minutes::from_minutes(330),
                Minutes::from_minutes(1080),
            )],
            fatigue_rules: FatigueRules::default(),
        }],
        cost: CostModel {
            fixed_per_route_cents: 15_000,
            per_km_cents: 90,
            per_hour_cents: 4_500,
            late_per_min_cents: 0,
        },
        chain_rules: ChainRules::default(),
        curfews: CurfewRules::default(),
        horizon_start: None,
    };
    let _ = apply_optional_horizon(horizon_start, &mut problem);
    problem
}

impl ProblemBuilderState {
    /// Generates a synthetic problem from the scenario parameters and stores it as `self.draft`.
    pub(crate) fn generate_scenario(&mut self) -> Result<&Problem> {
        let mut problem = sim::generate(&sim::SimOptions {
            chain: self.chain,
            city: self.city,
            stores: self.stores.clamp(1, 4_900),
            depots: self.depots.clamp(1, 50),
            vehicles: self.vehicles.clamp(1, 500),
            seed: self.seed,
            christmas_peak: self.christmas_peak,
        });
        let name =
            (!self.custom_name.trim().is_empty()).then(|| self.custom_name.trim().to_string());
        self.finalize_draft(&mut problem, name)?;
        self.draft = problem;
        self.sync_inputs_from_draft();
        self.editor_error = None;
        Ok(&self.draft)
    }

    /// Resets `self.draft` to a minimal 1-depot, 1-stop, 1-vehicle custom starter
    /// ready for inputting user addresses and locations.
    pub(crate) fn reset_custom_template(&mut self) -> Result<&Problem> {
        let name = if !self.custom_name.trim().is_empty() {
            self.custom_name.trim().to_string()
        } else {
            "Custom Route".to_string()
        };
        let mut problem = starter_problem(
            self.stop_chain,
            self.city,
            Some(name.clone()),
            self.use_city_timezones,
            "",
        );
        self.finalize_draft(&mut problem, Some(name))?;
        self.draft = problem;
        self.depot_query.clear();
        self.depot_matches.clear();
        self.stop_matches.clear();
        self.stop_resolved_query.clear();
        self.lookup_status = None;
        self.sync_inputs_from_draft();
        self.editor_error = None;
        Ok(&self.draft)
    }

    fn finalize_draft(&self, problem: &mut Problem, custom_name: Option<String>) -> Result<()> {
        apply_optional_horizon(&self.horizon_start, problem)?;
        if let Some(name) = custom_name {
            problem.name = name;
        }
        if self.use_city_timezones {
            let tz = self.city.iana_timezone().to_string();
            for depot in &mut problem.depots {
                depot.timezone = Some(tz.clone());
            }
            for store in &mut problem.stores {
                store.timezone = Some(tz.clone());
            }
        }
        Ok(())
    }

    fn sync_inputs_from_draft(&mut self) {
        if let Some(depot) = self.draft.depots.first() {
            self.stop_lat = depot.location.lat - 0.012;
            self.stop_lon = depot.location.lon - 0.005;
        }
    }

    /// Parses a JSON problem document into the interactive editor.
    pub(crate) fn load_from_json(&mut self, json: &str) -> Result<()> {
        let mut parsed: Problem =
            serde_json::from_str(json).context("invalid Problem JSON for editor")?;
        reindex_problem(&mut parsed);
        self.custom_name = parsed.name.clone();
        self.depot_query = parsed
            .depots
            .first()
            .and_then(|d| d.address.clone())
            .unwrap_or_default();
        self.depot_matches.clear();
        self.draft = parsed;
        self.sync_inputs_from_draft();
        self.editor_error = None;
        Ok(())
    }

    /// Serializes `self.draft` after normalising dense `0..len` ids and validating.
    pub(crate) fn build_validated_json(&mut self) -> Result<String> {
        reindex_problem(&mut self.draft);
        validate_draft(&self.draft)?;
        serde_json::to_string_pretty(&self.draft).context("serializing configured problem")
    }

    /// Updates Main Depot (index 0, or creates one if missing) with coordinates and optional address.
    pub(crate) fn set_main_depot_location(&mut self, lat: f64, lon: f64, address: Option<String>) {
        if let Some(depot) = self.draft.depots.first_mut() {
            depot.location = LatLon::new(lat, lon);
            depot.address = address;
        } else {
            self.draft.depots.push(Depot {
                id: DepotId(0),
                name: "Main Depot".to_string(),
                location: LatLon::new(lat, lon),
                address,
                timezone: self
                    .use_city_timezones
                    .then(|| self.city.iana_timezone().to_string()),
            });
        }
        reindex_problem(&mut self.draft);
    }

    /// Applies one geocoded/parsed match to either Main Depot or the quick-add Place fields.
    pub(crate) fn apply_geocode_match(&mut self, target: GeocodeTarget, chosen: &GeocodeMatch) {
        let address = (chosen.source != "Coordinates").then(|| chosen.label.clone());
        match target {
            GeocodeTarget::Depot => {
                if let Some(label) = &address {
                    self.depot_query = label.clone();
                }
                self.set_main_depot_location(chosen.lat, chosen.lon, address);
                self.lookup_status = Some(format!(
                    "Depot set to {} ({:.5}, {:.5}) via {}",
                    chosen.label, chosen.lat, chosen.lon, chosen.source
                ));
            }
            GeocodeTarget::Stop => {
                self.stop_lat = chosen.lat;
                self.stop_lon = chosen.lon;
                if let Some(label) = &address {
                    self.stop_address_input = label.clone();
                    self.stop_resolved_query = label.clone();
                } else {
                    self.stop_resolved_query = self.stop_address_input.trim().to_string();
                }
                self.lookup_status = Some(format!(
                    "Place coordinates set to {:.5}, {:.5} via {}",
                    chosen.lat, chosen.lon, chosen.source
                ));
            }
        }
    }

    /// Adds a place/store at explicit coordinates using the quick-add inputs and clears name/address fields.
    pub(crate) fn add_custom_stop_at(
        &mut self,
        lat: f64,
        lon: f64,
        address_override: Option<String>,
    ) -> Result<()> {
        anyhow::ensure!(
            lat.is_finite() && (-90.0..=90.0).contains(&lat),
            "latitude {lat} must be within -90..=90"
        );
        anyhow::ensure!(
            lon.is_finite() && (-180.0..=180.0).contains(&lon),
            "longitude {lon} must be within -180..=180"
        );
        let start = self.stop_window_start_minutes;
        let end = self.stop_window_end_minutes.max(start + 1);

        let idx = self.draft.stores.len();
        let raw_name = self.stop_name_input.trim();
        let name = if !raw_name.is_empty() {
            raw_name.to_string()
        } else {
            match self.stop_chain {
                Chain::General => format!("Place {}", idx + 1),
                other => format!("{other} #{}", idx + 1),
            }
        };
        let address = address_override.or_else(|| {
            let trimmed = self.stop_address_input.trim();
            (!trimmed.is_empty() && parse_lat_lon_pair(trimmed).is_none())
                .then(|| trimmed.to_string())
        });
        let tz = self
            .use_city_timezones
            .then(|| self.city.iana_timezone().to_string());

        self.draft.stores.push(Store {
            id: StoreId(idx as u32),
            chain: self.stop_chain,
            name,
            location: LatLon::new(lat, lon),
            address,
            demand: Load::new(
                self.stop_demand_ambient,
                self.stop_demand_chilled,
                self.stop_demand_frozen,
            ),
            service: Minutes::from_minutes(self.stop_service_minutes.max(0)),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(start),
                Minutes::from_minutes(end),
            )],
            timezone: tz,
        });
        reindex_problem(&mut self.draft);
        self.stop_name_input.clear();
        self.stop_address_input.clear();
        self.stop_resolved_query.clear();
        self.stop_matches.clear();
        self.editor_error = None;
        Ok(())
    }

    /// Attempts to add a stop from `stop_*` fields directly. Returns `Ok(false)`
    /// when `stop_address_input` contains an un-resolved street address so the
    /// caller can trigger an async address lookup first.
    pub(crate) fn try_add_stop_from_inputs(&mut self) -> Result<bool> {
        let addr_trimmed = self.stop_address_input.trim().to_string();
        if let Some((lat, lon)) = parse_lat_lon_pair(&addr_trimmed) {
            self.stop_lat = lat;
            self.stop_lon = lon;
            self.add_custom_stop_at(lat, lon, None)?;
            return Ok(true);
        }
        if !addr_trimmed.is_empty() && self.stop_resolved_query != addr_trimmed {
            return Ok(false);
        }
        let addr = (!addr_trimmed.is_empty()).then_some(addr_trimmed);
        self.add_custom_stop_at(self.stop_lat, self.stop_lon, addr)?;
        Ok(true)
    }

    /// Adds a new depot near the existing network centroid.
    pub(crate) fn add_depot(&mut self) {
        let loc = self
            .draft
            .depots
            .last()
            .map(|d| LatLon::new(d.location.lat + 0.01, d.location.lon + 0.01))
            .unwrap_or_else(|| {
                let (lat, lon) = city_center(self.city);
                LatLon::new(lat, lon)
            });
        let idx = self.draft.depots.len();
        let tz = self
            .use_city_timezones
            .then(|| self.city.iana_timezone().to_string());
        self.draft.depots.push(Depot {
            id: DepotId(idx as u32),
            name: format!("Depot {}", idx + 1),
            location: loc,
            address: None,
            timezone: tz,
        });
        reindex_problem(&mut self.draft);
    }

    /// Removes the depot at `index` (keeping at least one depot) and remaps references.
    pub(crate) fn remove_depot(&mut self, index: usize) {
        if self.draft.depots.len() <= 1 || index >= self.draft.depots.len() {
            return;
        }
        self.draft.depots.remove(index);
        for vehicle in &mut self.draft.vehicles {
            let dep = vehicle.depot.0 as usize;
            if dep == index {
                vehicle.depot = DepotId(0);
            } else if dep > index {
                vehicle.depot = DepotId((dep - 1) as u32);
            }
        }
        reindex_problem(&mut self.draft);
    }

    /// Adds a new store/place near the last stop or depot.
    pub(crate) fn add_store(&mut self) {
        let base = self
            .draft
            .stores
            .last()
            .map(|s| s.location)
            .or_else(|| self.draft.depots.first().map(|d| d.location))
            .unwrap_or_else(|| {
                let (lat, lon) = city_center(self.city);
                LatLon::new(lat, lon)
            });
        let idx = self.draft.stores.len();
        let tz = self
            .use_city_timezones
            .then(|| self.city.iana_timezone().to_string());
        let name = match self.stop_chain {
            Chain::General => format!("Place {}", idx + 1),
            other => format!("{other} #{}", idx + 1),
        };
        self.draft.stores.push(Store {
            id: StoreId(idx as u32),
            chain: self.stop_chain,
            name,
            location: LatLon::new(base.lat + 0.008, base.lon + 0.008),
            address: None,
            demand: Load::new(
                self.stop_demand_ambient,
                self.stop_demand_chilled,
                self.stop_demand_frozen,
            ),
            service: Minutes::from_minutes(self.stop_service_minutes.max(0)),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(self.stop_window_start_minutes),
                Minutes::from_minutes(
                    self.stop_window_end_minutes
                        .max(self.stop_window_start_minutes + 1),
                ),
            )],
            timezone: tz,
        });
        reindex_problem(&mut self.draft);
    }

    /// Removes the store/place at `index` (keeping at least one stop).
    pub(crate) fn remove_store(&mut self, index: usize) {
        if self.draft.stores.len() <= 1 || index >= self.draft.stores.len() {
            return;
        }
        self.draft.stores.remove(index);
        reindex_problem(&mut self.draft);
    }

    /// Adds a vehicle and paired default driver.
    pub(crate) fn add_vehicle(&mut self) {
        let idx = self.draft.vehicles.len();
        self.draft.vehicles.push(Vehicle {
            id: VehicleId(idx as u32),
            name: format!("Vehicle {}", idx + 1),
            depot: DepotId(0),
            capacity: Load::new(24, 16, 12),
            max_duration: Minutes::from_minutes(600),
            avg_speed_kmh: 50.0,
        });
        if self.draft.drivers.len() < self.draft.vehicles.len() {
            let drv_idx = self.draft.drivers.len();
            self.draft.drivers.push(Driver {
                id: DriverId(drv_idx as u32),
                name: format!("Driver {}", drv_idx + 1),
                shifts: vec![Shift::new(
                    Minutes::from_minutes(330),
                    Minutes::from_minutes(1080),
                )],
                fatigue_rules: FatigueRules::default(),
            });
        }
        reindex_problem(&mut self.draft);
    }

    /// Removes the vehicle at `index` (keeping at least one vehicle and one driver).
    pub(crate) fn remove_vehicle(&mut self, index: usize) {
        if self.draft.vehicles.len() <= 1 || index >= self.draft.vehicles.len() {
            return;
        }
        let had_matching_drivers = self.draft.drivers.len() == self.draft.vehicles.len();
        self.draft.vehicles.remove(index);
        if had_matching_drivers && self.draft.drivers.len() > 1 && index < self.draft.drivers.len()
        {
            self.draft.drivers.remove(index);
        }
        reindex_problem(&mut self.draft);
    }
}

/// Ensures contiguous `0..len` ids across depots, stores, vehicles, and drivers,
/// clamping out-of-range depot references safely.
pub(crate) fn reindex_problem(problem: &mut Problem) {
    for (idx, depot) in problem.depots.iter_mut().enumerate() {
        depot.id = DepotId(idx as u32);
    }
    for (idx, store) in problem.stores.iter_mut().enumerate() {
        store.id = StoreId(idx as u32);
    }
    let depot_count = problem.depots.len();
    for (idx, vehicle) in problem.vehicles.iter_mut().enumerate() {
        vehicle.id = VehicleId(idx as u32);
        if depot_count > 0 && (vehicle.depot.0 as usize) >= depot_count {
            vehicle.depot = DepotId(0);
        }
    }
    for (idx, driver) in problem.drivers.iter_mut().enumerate() {
        driver.id = DriverId(idx as u32);
    }
}

fn apply_optional_horizon(raw: &str, problem: &mut Problem) -> Result<()> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        problem.horizon_start = None;
        return Ok(());
    }
    let timestamp = domain::parse_horizon_start(trimmed).with_context(|| {
        format!("invalid horizon start '{trimmed}' (expected RFC 3339, e.g. 2026-11-05T00:00:00Z)")
    })?;
    problem.horizon_start = Some(timestamp);
    Ok(())
}

/// Validates structural invariants required before importing/solving a configured problem.
pub(crate) fn validate_draft(problem: &Problem) -> Result<()> {
    if problem.depots.is_empty() {
        anyhow::bail!("problem must contain at least one depot");
    }
    if problem.stores.is_empty() {
        anyhow::bail!("problem must contain at least one place or store to visit");
    }
    if problem.vehicles.is_empty() {
        anyhow::bail!("problem must contain at least one vehicle");
    }
    if problem
        .vehicles
        .iter()
        .any(|vehicle| (vehicle.depot.0 as usize) >= problem.depots.len())
    {
        anyhow::bail!("vehicle references unknown depot id");
    }
    if problem.stores.iter().any(|store| {
        store
            .receiving
            .iter()
            .any(|window| window.start > window.end)
    }) {
        anyhow::bail!("stop has a time window with start > end");
    }
    if problem
        .drivers
        .iter()
        .any(|driver| driver.shifts.iter().any(|shift| shift.start >= shift.end))
    {
        anyhow::bail!("driver has a shift with start >= end");
    }
    let bad_tzs = problem.validate_timezones();
    if !bad_tzs.is_empty() {
        let joined = bad_tzs
            .iter()
            .take(3)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("invalid timezone(s): {joined}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_builder_starts_with_general_custom_route() {
        let builder = ProblemBuilderState::default();
        assert_eq!(builder.draft.name, "Custom Route");
        assert_eq!(builder.draft.depots.len(), 1);
        assert_eq!(builder.draft.depots[0].name, "Main Depot");
        assert_eq!(builder.draft.stores.len(), 1);
        assert_eq!(builder.draft.stores[0].chain, Chain::General);
        assert_eq!(builder.draft.stores[0].name, "Stop 1");
        assert!(validate_draft(&builder.draft).is_ok());
    }

    #[test]
    fn geocode_matches_and_custom_stop_addition_preserve_addresses() {
        let mut builder = ProblemBuilderState::default();
        builder.apply_geocode_match(
            GeocodeTarget::Depot,
            &GeocodeMatch {
                label: "100 George St, Sydney NSW 2000".into(),
                lat: -33.864,
                lon: 151.208,
                source: "G-NAF".into(),
            },
        );
        assert_eq!(
            builder.draft.depots[0].address.as_deref(),
            Some("100 George St, Sydney NSW 2000")
        );
        assert!((builder.draft.depots[0].location.lat - -33.864).abs() < 1e-6);

        builder.stop_name_input = "Client Warehouse".into();
        builder.stop_address_input = "-33.9000, 151.1900".into();
        assert!(builder.try_add_stop_from_inputs().unwrap());
        assert_eq!(builder.draft.stores.len(), 2);
        let added = &builder.draft.stores[1];
        assert_eq!(added.name, "Client Warehouse");
        assert_eq!(added.chain, Chain::General);
        assert_eq!(added.address, None);
        assert!((added.location.lat - -33.90).abs() < 1e-6);

        builder.stop_name_input = "Cafe Supply".into();
        builder.stop_address_input = "45 King St, Newtown NSW 2042".into();
        assert!(!builder.try_add_stop_from_inputs().unwrap());
        builder.apply_geocode_match(
            GeocodeTarget::Stop,
            &GeocodeMatch {
                label: "45 King Street, Newtown NSW 2042".into(),
                lat: -33.893,
                lon: 151.182,
                source: "OpenStreetMap".into(),
            },
        );
        assert!(builder.try_add_stop_from_inputs().unwrap());
        assert_eq!(
            builder.draft.stores[2].address.as_deref(),
            Some("45 King Street, Newtown NSW 2042")
        );
        assert!(builder.build_validated_json().is_ok());
    }

    #[test]
    fn generate_scenario_respects_custom_name_timezones_and_horizon() {
        let mut builder = ProblemBuilderState {
            chain: Chain::Woolworths,
            city: sim::City::Melbourne,
            stores: 6,
            depots: 2,
            vehicles: 3,
            seed: 7,
            christmas_peak: true,
            use_city_timezones: true,
            horizon_start: "2026-11-05T00:00:00Z".into(),
            custom_name: "Melbourne Peak".into(),
            ..Default::default()
        };
        builder.generate_scenario().unwrap();
        assert_eq!(builder.draft.name, "Melbourne Peak");
        assert_eq!(builder.draft.stores.len(), 6);
        assert_eq!(builder.draft.depots.len(), 2);
        assert_eq!(builder.draft.vehicles.len(), 3);
        assert!(builder.draft.horizon_start.is_some());
        assert!(
            builder
                .draft
                .stores
                .iter()
                .all(|s| s.timezone.as_deref() == Some("Australia/Melbourne"))
        );
        assert!(builder.build_validated_json().is_ok());
    }

    #[test]
    fn invalid_horizon_start_returns_error() {
        let mut builder = ProblemBuilderState {
            horizon_start: "not-a-timestamp".into(),
            ..Default::default()
        };
        let err = builder.generate_scenario().unwrap_err();
        assert!(err.to_string().contains("RFC 3339"), "{err}");
    }

    #[test]
    fn add_and_remove_nodes_keep_dense_valid_problem() {
        let mut builder = ProblemBuilderState::default();
        builder.reset_custom_template().unwrap();
        assert_eq!(builder.draft.depots.len(), 1);
        assert_eq!(builder.draft.stores.len(), 1);
        assert_eq!(builder.draft.vehicles.len(), 1);

        builder.add_depot();
        builder.add_store();
        builder.add_vehicle();
        // Assign the second vehicle to Depot 1, then delete Depot 1 and verify remap.
        builder.draft.vehicles[1].depot = DepotId(1);
        builder.remove_depot(1);
        builder.remove_store(0);
        builder.remove_vehicle(0);

        assert!(validate_draft(&builder.draft).is_ok());
        assert_eq!(builder.draft.depots[0].id, DepotId(0));
        assert_eq!(builder.draft.stores[0].id, StoreId(0));
        assert_eq!(builder.draft.vehicles[0].id, VehicleId(0));
        assert_eq!(builder.draft.vehicles[0].depot, DepotId(0));

        let serialized = builder.build_validated_json().unwrap();
        let mut reloaded = ProblemBuilderState::default();
        reloaded.load_from_json(&serialized).unwrap();
        assert_eq!(reloaded.draft.name, builder.draft.name);
    }
}
