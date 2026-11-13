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
    Chain, ChainRules, CostModel, CurfewRules, Depot, DepotId, Driver, DriverId, FatigueRules,
    LatLon, Load, Minutes, Problem, Shift, Store, StoreId, TimeWindow, Vehicle, VehicleId,
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
///
/// The parser is tolerant of extra blank lines, comments (lines starting with `#` or `//`),
/// case-insensitive section headers, and variable whitespace. It validates that
/// all required sections and fields are present.
pub fn parse_solomon_file(path: &Path) -> anyhow::Result<SolomonInstance> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);

    let lines = reader
        .lines()
        .map_while(Result::ok)
        .filter(|l| {
            !l.trim().is_empty() && !l.trim().starts_with('#') && !l.trim().starts_with("//")
        })
        .collect::<Vec<_>>();

    if lines.is_empty() {
        anyhow::bail!("empty file");
    }

    let name = lines[0].trim().to_string();
    let mut idx = 1;

    // Helper: find next section header (case-insensitive)
    fn find_section(lines: &[String], idx: &mut usize, target: &str) -> anyhow::Result<()> {
        while *idx < lines.len() && !lines[*idx].trim().eq_ignore_ascii_case(target) {
            *idx += 1;
        }
        if *idx >= lines.len() {
            anyhow::bail!("missing {} section", target);
        }
        *idx += 1; // skip header
        Ok(())
    }

    // VEHICLE section
    find_section(&lines, &mut idx, "VEHICLE")?;
    // Find NUMBER line
    while idx < lines.len() && !lines[idx].trim().starts_with("NUMBER") {
        idx += 1;
    }
    if idx >= lines.len() {
        anyhow::bail!("missing NUMBER/CAPACITY line in VEHICLE section");
    }
    idx += 1; // data line
    if idx >= lines.len() {
        anyhow::bail!("missing vehicle data line after NUMBER header");
    }
    let parts: Vec<&str> = lines[idx].split_whitespace().collect();
    if parts.len() < 2 {
        anyhow::bail!("expected NUMBER CAPACITY, got: {}", lines[idx]);
    }
    let vehicle_count: usize = parts[0].parse().with_context(|| "parsing vehicle count")?;
    let vehicle_capacity: u32 = parts[1]
        .parse()
        .with_context(|| "parsing vehicle capacity")?;

    // CUSTOMER section
    find_section(&lines, &mut idx, "CUSTOMER")?;
    // Skip header line(s) - any line starting with CUST (case-insensitive)
    while idx < lines.len() && lines[idx].trim().to_uppercase().starts_with("CUST") {
        idx += 1;
    }

    let mut customers = Vec::new();
    while idx < lines.len() {
        let line = lines[idx].trim();
        if line.is_empty() {
            idx += 1;
            continue;
        }
        // Stop if we hit another section header
        if line.eq_ignore_ascii_case("VEHICLE")
            || line.eq_ignore_ascii_case("CUSTOMER")
            || line.eq_ignore_ascii_case("DEPOT")
        {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 7 {
            anyhow::bail!("customer line {} has <7 fields: {}", idx + 1, line);
        }
        customers.push(SolomonCustomer {
            id: parts[0]
                .parse()
                .with_context(|| format!("parsing id at line {}: {}", idx + 1, parts[0]))?,
            x: parts[1]
                .parse()
                .with_context(|| format!("parsing x at line {}: {}", idx + 1, parts[1]))?,
            y: parts[2]
                .parse()
                .with_context(|| format!("parsing y at line {}: {}", idx + 1, parts[2]))?,
            demand: parts[3]
                .parse()
                .with_context(|| format!("parsing demand at line {}: {}", idx + 1, parts[3]))?,
            ready: parts[4]
                .parse()
                .with_context(|| format!("parsing ready at line {}: {}", idx + 1, parts[4]))?,
            due: parts[5]
                .parse()
                .with_context(|| format!("parsing due at line {}: {}", idx + 1, parts[5]))?,
            service: parts[6]
                .parse()
                .with_context(|| format!("parsing service at line {}: {}", idx + 1, parts[6]))?,
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

/// Euclidean distance between two Solomon coordinates.
fn euclidean_distance(a: &SolomonCustomer, b: &SolomonCustomer) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

/// Converts a `SolomonInstance` into a `domain::Problem`.
///
/// Mapping:
/// - Single depot at the depot coordinates.
/// - One vehicle per vehicle_count, all starting at the depot.
/// - Capacity per vehicle is the Solomon capacity (all ambient).
/// - One driver per vehicle with a shift covering the whole horizon plus
///   the worst-case return leg from the farthest customer to depot.
/// - Time windows from READY TIME .. DUE DATE, service time from SERVICE TIME.
/// - Travel times are Euclidean distance / speed (Solomon uses Euclidean coordinates).
pub fn solomon_to_problem(inst: SolomonInstance, speed_kmh: f64) -> Problem {
    // Skip the depot (customer 0) when computing max_due - depot often has
    // a large due date (e.g., 1236 in C101) which inflates the horizon artificially
    let max_due = inst
        .customers
        .iter()
        .skip(1) // skip depot
        .map(|c| c.due)
        .max()
        .unwrap_or(1440);

    // Compute worst-case return leg from any customer to depot (in minutes at given speed).
    let depot_cust = &inst.customers[0];
    let max_return_km = inst
        .customers
        .iter()
        .skip(1) // skip depot itself
        .map(|c| euclidean_distance(depot_cust, c))
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(0.0);
    let max_return_min = (max_return_km / speed_kmh * 60.0).ceil() as i64;

    let horizon_end = max_due + max_return_min + 10; // small buffer for service time

    let depot_id = DepotId(0);
    let depot = Depot {
        id: depot_id,
        name: "Depot".into(),
        location: inst.depot,
        address: None,
        timezone: None,
    };

    let vehicles: Vec<Vehicle> = (0..inst.vehicle_count)
        .map(|i| Vehicle {
            id: VehicleId(i as u32),
            name: format!("Vehicle {:03}", i + 1),
            depot: depot_id,
            capacity: Load::new(inst.vehicle_capacity, 0, 0),
            max_duration: Minutes::from_minutes(horizon_end),
            avg_speed_kmh: speed_kmh,
        })
        .collect();

    let drivers: Vec<Driver> = (0..inst.vehicle_count)
        .map(|i| Driver {
            id: DriverId(i as u32),
            name: format!("Driver {:03}", i + 1),
            shifts: vec![Shift::new(
                Minutes::ZERO,
                Minutes::from_minutes(horizon_end),
            )],
            fatigue_rules: FatigueRules::default(),
        })
        .collect();

    let mut stores = Vec::new();
    // Solomon customers are 0-indexed with 0 = depot; skip the depot
    // Remap customer IDs (1..=n) to contiguous 0-based store indices.
    let customers: Vec<_> = inst.customers.iter().skip(1).collect();
    for (store_index, cust) in customers.iter().enumerate() {
        stores.push(Store {
            id: StoreId(store_index as u32),
            chain: Chain::Coles, // placeholder
            name: format!("Customer {:03}", cust.id),
            location: LatLon::new(cust.x, cust.y),
            address: None,
            timezone: None,
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
            fixed_per_route_cents: 15_000,
            per_km_cents: 100, // 1 per unit distance
            per_hour_cents: 0,
            late_per_min_cents: 0,
        },
        chain_rules: ChainRules::default(),
        curfews: CurfewRules::default(),
        horizon_start: None,
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
