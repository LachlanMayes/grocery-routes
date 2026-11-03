//! Vendored Solomon / VRPLib instance parser (M1).
//!
//! Parses the classic Solomon 1987 format (also used by CVRPLIB):
//!
//! ```text
//! C101
//!
//! VEHICLE
//! NUMBER     CAPACITY
//! 25         200
//!
//! CUSTOMER
//! CUST NO.  XCOORD.   YCOORD.  DEMAND  READY TIME  DUE DATE  SERVICE TIME
//! 0         40        50       0       0           1236      0
//! 1         22        22       18      128         250       10
//! ...
//! ```
//!
//! The parser is intentionally strict and minimal — it handles the subset
//! needed for C101 / R101 / RC101 style instances.

use anyhow::Context;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use domain::{
    Chain, CostModel, Depot, DepotId, Driver, DriverId, LatLon, Load, Minutes, Problem, Shift,
    Store, StoreId, TimeWindow, Vehicle, VehicleId,
};

/// A parsed Solomon instance.
#[derive(Debug, Clone)]
pub struct SolomonInstance {
    pub name: String,
    pub vehicle_count: usize,
    pub vehicle_capacity: u32,
    pub depot: LatLon,
    pub customers: Vec<SolomonCustomer>,
}

/// A customer (store or depot) from a Solomon file.
#[derive(Debug, Clone)]
pub struct SolomonCustomer {
    pub id: usize,
    pub x: f64,
    pub y: f64,
    pub demand: u32,
    pub ready: i64,
    pub due: i64,
    pub service: i64,
}

/// Parses a Solomon/VRPLib instance file.
pub fn parse_solomon_file(path: &Path) -> anyhow::Result<SolomonInstance> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);

    let lines = reader
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>();

    if lines.is_empty() {
        anyhow::bail!("empty file");
    }

    let name = lines[0].trim().to_string();
    let mut idx = 1;

    // Skip to VEHICLE section
    while idx < lines.len() && !lines[idx].trim().eq_ignore_ascii_case("VEHICLE") {
        idx += 1;
    }
    if idx >= lines.len() {
        anyhow::bail!("missing VEHICLE section");
    }
    idx += 1; // skip "VEHICLE"
    while idx < lines.len() && !lines[idx].trim().starts_with("NUMBER") {
        idx += 1;
    }
    if idx >= lines.len() {
        anyhow::bail!("missing NUMBER/CAPACITY line");
    }
    // Parse NUMBER and CAPACITY (they're on the next line or same line)
    idx += 1;
    if idx >= lines.len() {
        anyhow::bail!("missing vehicle data line");
    }
    let parts: Vec<&str> = lines[idx].split_whitespace().collect();
    if parts.len() < 2 {
        anyhow::bail!("expected NUMBER CAPACITY, got: {}", lines[idx]);
    }
    let vehicle_count: usize = parts[0].parse().with_context(|| "parsing vehicle count")?;
    let vehicle_capacity: u32 = parts[1]
        .parse()
        .with_context(|| "parsing vehicle capacity")?;

    // Skip to CUSTOMER section
    while idx < lines.len() && !lines[idx].trim().eq_ignore_ascii_case("CUSTOMER") {
        idx += 1;
    }
    if idx >= lines.len() {
        anyhow::bail!("missing CUSTOMER section");
    }
    idx += 1; // skip "CUSTOMER"
    // Skip header line
    while idx < lines.len() && lines[idx].trim().starts_with("CUST") {
        idx += 1;
    }

    let mut customers = Vec::new();
    while idx < lines.len() {
        let line = lines[idx].trim();
        if line.is_empty() {
            idx += 1;
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 7 {
            anyhow::bail!("customer line has <7 fields: {}", line);
        }
        customers.push(SolomonCustomer {
            id: parts[0]
                .parse()
                .with_context(|| format!("parsing id: {}", parts[0]))?,
            x: parts[1]
                .parse()
                .with_context(|| format!("parsing x: {}", parts[1]))?,
            y: parts[2]
                .parse()
                .with_context(|| format!("parsing y: {}", parts[2]))?,
            demand: parts[3]
                .parse()
                .with_context(|| format!("parsing demand: {}", parts[3]))?,
            ready: parts[4]
                .parse()
                .with_context(|| format!("parsing ready: {}", parts[4]))?,
            due: parts[5]
                .parse()
                .with_context(|| format!("parsing due: {}", parts[5]))?,
            service: parts[6]
                .parse()
                .with_context(|| format!("parsing service: {}", parts[6]))?,
        });
        idx += 1;
    }

    if customers.is_empty() {
        anyhow::bail!("no customers parsed");
    }

    let depot = LatLon::new(customers[0].x, customers[0].y);

    Ok(SolomonInstance {
        name,
        vehicle_count,
        vehicle_capacity,
        depot,
        customers,
    })
}

/// Converts a `SolomonInstance` into a `domain::Problem`.
///
/// Mapping:
/// - Single depot at the depot coordinates.
/// - One vehicle per vehicle_count, all starting at the depot.
/// - Capacity per vehicle is the Solomon capacity (all ambient).
/// - One driver per vehicle with a shift covering the whole horizon (0..=max_due).
/// - Time windows from READY TIME .. DUE DATE, service time from SERVICE TIME.
/// - Travel times are Euclidean distance / speed (Solomon uses Euclidean coordinates).
pub fn solomon_to_problem(inst: SolomonInstance, speed_kmh: f64) -> Problem {
    let max_due = inst.customers.iter().map(|c| c.due).max().unwrap_or(1440);

    let depot_id = DepotId(0);
    let depot = Depot {
        id: depot_id,
        name: "Depot".into(),
        location: inst.depot,
    };

    let vehicles: Vec<Vehicle> = (0..inst.vehicle_count)
        .map(|i| Vehicle {
            id: VehicleId(i as u32),
            name: format!("Vehicle {:03}", i + 1),
            depot: depot_id,
            capacity: Load::new(inst.vehicle_capacity, 0, 0),
            max_duration: Minutes::from_minutes(max_due + 100),
            avg_speed_kmh: speed_kmh,
        })
        .collect();

    let drivers: Vec<Driver> = (0..inst.vehicle_count)
        .map(|i| Driver {
            id: DriverId(i as u32),
            name: format!("Driver {:03}", i + 1),
            shifts: vec![Shift::new(
                Minutes::ZERO,
                Minutes::from_minutes(max_due + 100),
            )],
        })
        .collect();

    let mut stores = Vec::new();
    // Solomon customers are 0-indexed with 0 = depot; skip the depot
    for cust in inst.customers.iter().skip(1) {
        stores.push(Store {
            id: StoreId(cust.id as u32),
            chain: Chain::Coles, // placeholder
            name: format!("Customer {:03}", cust.id),
            location: LatLon::new(cust.x, cust.y),
            demand: Load::new(cust.demand, 0, 0),
            service: Minutes::from_minutes(cust.service),
            receiving: vec![TimeWindow::new(
                Minutes::from_minutes(cust.ready),
                Minutes::from_minutes(cust.due),
            )],
        });
    }

    Problem {
        name: inst.name,
        depots: vec![depot],
        stores,
        vehicles,
        drivers,
        cost: CostModel {
            fixed_per_route_cents: 0,
            per_km_cents: 100, // 1 per unit distance
            per_hour_cents: 0,
            late_per_min_cents: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    const C101_SAMPLE: &str = r#"C101

VEHICLE
NUMBER     CAPACITY
25         200

CUSTOMER
CUST NO.  XCOORD.   YCOORD.  DEMAND  READY TIME  DUE DATE  SERVICE TIME
0         40        50       0       0           1236      0
1         22        22       18      128         250       10
2         36        26       26      284         386       10
3         22        42       16      246         378       10
4         18        46       20      128         250       10
"#;

    #[test]
    fn parses_c101_sample() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(C101_SAMPLE.as_bytes()).unwrap();
        f.flush().unwrap();

        let inst = parse_solomon_file(f.path()).unwrap();
        assert_eq!(inst.name, "C101");
        assert_eq!(inst.vehicle_count, 25);
        assert_eq!(inst.vehicle_capacity, 200);
        assert_eq!(inst.depot, LatLon::new(40.0, 50.0));
        assert_eq!(inst.customers.len(), 5); // 0=depot + 4 customers
        assert_eq!(inst.customers[0].id, 0);
        assert_eq!(inst.customers[1].id, 1);
        assert_eq!(inst.customers[1].demand, 18);
        assert_eq!(inst.customers[1].ready, 128);
        assert_eq!(inst.customers[1].due, 250);
        assert_eq!(inst.customers[1].service, 10);
    }

    #[test]
    fn converts_to_problem() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(C101_SAMPLE.as_bytes()).unwrap();
        f.flush().unwrap();

        let inst = parse_solomon_file(f.path()).unwrap();
        let problem = solomon_to_problem(inst, 1.0);

        assert_eq!(problem.depots.len(), 1);
        assert_eq!(problem.vehicles.len(), 25);
        assert_eq!(problem.drivers.len(), 25);
        assert_eq!(problem.stores.len(), 4); // 4 customers
        assert_eq!(problem.vehicles[0].capacity, Load::new(200, 0, 0));
        // First customer has window 128..250
        assert_eq!(
            problem.stores[0].receiving[0],
            TimeWindow::new(Minutes::from_minutes(128), Minutes::from_minutes(250))
        );
    }
}
