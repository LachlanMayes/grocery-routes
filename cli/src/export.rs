//! Report export: a self-contained HTML route sheet and a GeoJSON overlay.
//!
//! Every function here is pure — no I/O, no printing — so the reports can be
//! generated and inspected without touching the filesystem.

use domain::{DriverId, Load, Node, Problem, RoutePlan, Solution, StoreId};
use routing::csv_field;
use serde_json::{Value, json};

/// Escapes `&`, `<`, `>`, `"` and `'` for safe interpolation into HTML.
///
/// Returns the input unchanged when it contains none of those characters.
pub fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Formats a cent amount as Australian dollars with thousands separators.
///
/// Negative amounts are prefixed with `-`, e.g. `-12345` cents becomes
/// `-$123.45`.
pub fn format_aud_cents(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let absolute = cents.unsigned_abs();
    format!(
        "{sign}${}.{:02}",
        group_thousands(absolute / 100),
        absolute % 100
    )
}

/// Renders a whole-dollar amount with `,` thousands separators.
fn group_thousands(whole: u64) -> String {
    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.char_indices() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Renders a self-contained HTML route sheet for a solved problem.
///
/// The page carries inline CSS only: no scripts, no external assets. One
/// section is emitted per route, plus an "Unassigned" section when the plan
/// leaves stores behind.
pub fn route_sheet_html(problem: &Problem, solution: &Solution) -> String {
    let mut blocks: Vec<String> = Vec::new();
    blocks.push(format!(
        "<h1>Route sheet: {}</h1>\n\
         <p class=\"totals\">Routes: {} &middot; Distance: {:.1} km &middot; Duration: {} &middot; Cost: {}</p>",
        escape_html(&problem.name),
        solution.routes.len(),
        solution.total_distance_km,
        solution.total_duration,
        format_aud_cents(solution.total_cost_cents),
    ));
    for (index, route) in solution.routes.iter().enumerate() {
        blocks.push(route_section_html(problem, index, route));
    }
    if let Some(section) = returnables_section_html(problem, solution) {
        blocks.push(section);
    }
    if !solution.unassigned.is_empty() {
        blocks.push(unassigned_section_html(problem, &solution.unassigned));
    }
    if let Some(note) = holiday_note(problem, solution) {
        blocks.insert(1, note);
    }
    blocks.push(
        "<p class=\"attribution\">Map links open OpenStreetMap (&copy; OpenStreetMap contributors)</p>"
            .to_string(),
    );
    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <title>Route sheet: {}</title>\n\
         <style>\n{}\n</style>\n\
         </head>\n\
         <body>\n{}\n</body>\n\
         </html>\n",
        escape_html(&problem.name),
        STYLES,
        blocks.join("\n")
    )
}

/// Renders the plan as a GeoJSON `FeatureCollection`.
///
/// Each route becomes a `LineString` feature with one `[lon, lat]` coordinate
/// per known step (unknown nodes are skipped defensively); every store then
/// becomes a `Point` feature.
pub fn routes_geojson(problem: &Problem, solution: &Solution) -> String {
    let mut features: Vec<Value> = Vec::new();
    for (index, route) in solution.routes.iter().enumerate() {
        let coordinates: Vec<Value> = route
            .steps
            .iter()
            .filter_map(|step| problem.node_location(step.node))
            .map(|location| json!([location.lon, location.lat]))
            .collect();
        let vehicle = problem
            .vehicle(route.vehicle)
            .map(|vehicle| vehicle.name.clone())
            .unwrap_or_else(|| format!("vehicle {}", route.vehicle));
        let drivers: Vec<String> = route
            .drivers
            .iter()
            .map(|&driver| driver_name(problem, driver))
            .collect();
        features.push(json!({
            "type": "Feature",
            "geometry": {
                "type": "LineString",
                "coordinates": coordinates,
            },
            "properties": {
                "route": index,
                "vehicle": vehicle,
                "drivers": drivers,
                "distance_km": route.distance_km,
                "duration": route.duration.to_string(),
                "stop_count": route.steps.len(),
            },
        }));
    }
    for store in &problem.stores {
        features.push(json!({
            "type": "Feature",
            "geometry": {
                "type": "Point",
                "coordinates": [store.location.lon, store.location.lat],
            },
            "properties": {
                "store_id": store.id.0,
                "chain": store.chain.to_string(),
                "name": &store.name,
            },
        }));
    }
    let collection = json!({
        "type": "FeatureCollection",
        "features": features,
    });
    serde_json::to_string_pretty(&collection).expect("GeoJSON documents always serialize")
}

/// Renders one route as a section with a per-step table.
fn route_section_html(problem: &Problem, index: usize, route: &RoutePlan) -> String {
    let show_local = problem.horizon_start.is_some();
    let vehicle = problem
        .vehicle(route.vehicle)
        .map_or("unknown vehicle", |vehicle| vehicle.name.as_str());
    let drivers_text = if route.drivers.is_empty() {
        "unassigned".to_string()
    } else {
        route
            .drivers
            .iter()
            .map(|&driver| escape_html(&driver_name(problem, driver)))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut rows: Vec<String> = Vec::with_capacity(route.steps.len());
    for (sequence, step) in route.steps.iter().enumerate() {
        let (node_name, tag, demand) = match step.node {
            Node::Depot(id) => match problem.depot(id) {
                Some(depot) => (depot.name.clone(), "depot", "—".to_string()),
                None => (format!("depot {id}"), "depot", String::new()),
            },
            Node::Store(id) => match problem.store(id) {
                Some(store) => (store.name.clone(), "store", format_load(store.demand)),
                None => (format!("store {id}"), "store", String::new()),
            },
        };
        let node_cell = node_map_cell(problem, step.node, &node_name);
        let local_cell = if show_local {
            domain::local_time_short(problem, node_timezone(problem, step.node), step.arrive)
                .unwrap_or_else(|| "&mdash;".to_string())
        } else {
            String::new()
        };
        rows.push(format!(
            "<tr><td>{}</td><td>{}</td><td class=\"tag\">{tag}</td>\
             <td>{}</td><td>{}</td><td>{demand}</td>{}</tr>",
            sequence + 1,
            node_cell,
            step.arrive,
            step.depart,
            if show_local {
                format!("<td>{local_cell}</td>")
            } else {
                String::new()
            }
        ));
    }
    let local_header = if show_local {
        "<th>Local (arrive)</th>"
    } else {
        ""
    };

    format!(
        "<section class=\"route\">\n\
         <h2>Route {} — {}</h2>\n\
         <p class=\"meta\">Drivers: {drivers_text} &middot; Distance: {:.1} km &middot; \
         Duration: {} &middot; Cost: {}</p>\n\
         <table>\n\
         <thead><tr><th>#</th><th>Node</th><th>Type</th><th>Arrive</th><th>Depart</th>\
         <th>Demand (amb / chl / frz)</th>{local_header}</tr></thead>\n\
         <tbody>\n{}\n</tbody>\n\
         </table>\n\
         </section>",
        index + 1,
        escape_html(vehicle),
        route.distance_km,
        route.duration,
        format_aud_cents(route.cost_cents),
        rows.join("\n"),
    )
}

/// Renders the stores that the plan could not place.
fn unassigned_section_html(problem: &Problem, unassigned: &[StoreId]) -> String {
    let items = unassigned
        .iter()
        .map(|&store_id| {
            let name = problem
                .store(store_id)
                .map_or_else(|| format!("store {store_id}"), |store| store.name.clone());
            format!(
                "<li>{} <span class=\"tag\">(store {store_id})</span></li>",
                escape_html(&name)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("<section class=\"unassigned\">\n<h2>Unassigned</h2>\n<ul>\n{items}\n</ul>\n</section>")
}

/// Renders the returnables ledger as a section, when the plan serves at
/// least one store.
///
/// Collection figures come from the problem's per-banner fixtures; see
/// `domain::ledger`.
fn returnables_section_html(problem: &Problem, solution: &Solution) -> Option<String> {
    let ledger = domain::ledger(problem, solution);
    if ledger.stores.is_empty() {
        return None;
    }
    let rows: Vec<String> = ledger
        .stores
        .iter()
        .map(|balance| {
            let (name, chain) = problem.store(balance.store).map_or_else(
                || (format!("store {}", balance.store), "—".to_string()),
                |store| (store.name.clone(), store.chain.to_string()),
            );
            format!(
                "<tr><td>{}</td><td>{chain}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html(&name),
                balance.visits,
                format_load(balance.issued),
                format_load(balance.collected),
                format_load(balance.outstanding),
            )
        })
        .collect();
    Some(format!(
        "<section class=\"returnables\">\n\
         <h2>Returnables ledger</h2>\n\
         <p class=\"meta\">Cages issued with deliveries against empties collected per visit. \
         Collection figures are illustrative fixtures, not researched operator data.</p>\n\
         <table>\n\
         <thead><tr><th>Store</th><th>Chain</th><th>Visits</th>\
         <th>Issued (amb / chl / frz)</th><th>Collected (amb / chl / frz)</th>\
         <th>Outstanding (amb / chl / frz)</th></tr></thead>\n\
         <tbody>\n{}\n\
         <tr class=\"total\"><td>Total</td><td></td><td></td><td>{}</td><td>{}</td><td>{}</td></tr>\n\
         </tbody>\n</table>\n</section>",
        rows.join("\n"),
        format_load(ledger.total_issued),
        format_load(ledger.total_collected),
        format_load(ledger.total_outstanding),
    ))
}

/// Renders the returnables ledger as CSV: a header plus one row per served
/// store. Store names are quoted when they contain commas or quotes.
pub fn returnables_csv(problem: &Problem, solution: &Solution) -> String {
    let ledger = domain::ledger(problem, solution);
    let mut out = String::from(
        "store_id,store,chain,visits,\
         issued_ambient,issued_chilled,issued_frozen,\
         collected_ambient,collected_chilled,collected_frozen,\
         outstanding_ambient,outstanding_chilled,outstanding_frozen\n",
    );
    for balance in &ledger.stores {
        let (name, chain) = problem.store(balance.store).map_or_else(
            || (format!("store {}", balance.store), "unknown".to_string()),
            |store| (store.name.clone(), store.chain.to_string()),
        );
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            balance.store.0,
            csv_field(&name),
            csv_field(&chain),
            balance.visits,
            balance.issued.ambient,
            balance.issued.chilled,
            balance.issued.frozen,
            balance.collected.ambient,
            balance.collected.chilled,
            balance.collected.frozen,
            balance.outstanding.ambient,
            balance.outstanding.chilled,
            balance.outstanding.frozen,
        ));
    }
    out
}

/// Formats a load as `ambient / chilled / frozen` handling units.
fn format_load(load: Load) -> String {
    format!("{} / {} / {}", load.ambient, load.chilled, load.frozen)
}

/// The display name of a driver, falling back to the id when unknown.
fn driver_name(problem: &Problem, driver: DriverId) -> String {
    problem
        .driver(driver)
        .map_or_else(|| format!("driver {driver}"), |driver| driver.name.clone())
}

/// The IANA timezone of a node's owner, if any.
fn node_timezone(problem: &Problem, node: Node) -> Option<&str> {
    match node {
        Node::Depot(id) => problem
            .depot(id)
            .and_then(|depot| depot.timezone.as_deref()),
        Node::Store(id) => problem
            .store(id)
            .and_then(|store| store.timezone.as_deref()),
    }
}

/// Renders the node name, wrapped in an OpenStreetMap deep link when the
/// node has a known location.
fn node_map_cell(problem: &Problem, node: Node, name: &str) -> String {
    match problem.node_location(node) {
        Some(location) => format!(
            "<a href=\"https://www.openstreetmap.org/?mlat={lat}&amp;mlon={lon}#map=18/{lat}/{lon}\">{}</a>",
            escape_html(name),
            lat = location.lat,
            lon = location.lon,
        ),
        None => escape_html(name),
    }
}

/// Lists national public holidays falling inside the plan's horizon, when
/// the problem declares an anchor and the steps' dates resolve.
///
/// Returns `None` when no holiday falls inside the horizon. This is a
/// scheduling aid, not legal advice — see `docs/PLAN.md`.
fn holiday_note(problem: &Problem, solution: &Solution) -> Option<String> {
    problem.horizon_start?;
    let mut dates = std::collections::BTreeSet::new();
    for route in &solution.routes {
        for step in &route.steps {
            let timezone = node_timezone(problem, step.node);
            for at in [step.arrive, step.depart] {
                if let Some(date) = domain::local_date(problem, timezone, at) {
                    dates.insert(date);
                }
            }
        }
    }
    let holidays: Vec<String> = dates
        .into_iter()
        .filter(|&date| domain::is_public_holiday(date))
        .map(|date| {
            let names = domain::national_public_holidays(date).join(" / ");
            format!("{date} ({names})")
        })
        .collect();
    if holidays.is_empty() {
        return None;
    }
    Some(format!(
        "<p class=\"holiday-note\">National public holidays in this plan: {}. \
         Deliveries may not be accepted; check each store's receiving rules.</p>",
        escape_html(&holidays.join("; "))
    ))
}

/// Inline stylesheet for the route sheet.
const STYLES: &str = "\
body { font-family: system-ui, sans-serif; margin: 2rem auto; max-width: 64rem; padding: 0 1rem; color: #222; }
h1 { margin-bottom: 0.25rem; }
h2 { border-bottom: 2px solid #b3001b; padding-bottom: 0.25rem; }
p.totals { color: #444; margin-top: 0; }
p.meta { color: #444; }
p.holiday-note { background: #fff3cd; border: 1px solid #e0c878; padding: 0.5rem 0.75rem; border-radius: 4px; }
p.attribution { color: #666; font-size: 0.85rem; margin-top: 2rem; }
section.route { margin-top: 2rem; }
section.returnables { margin-top: 2rem; }
section.unassigned { margin-top: 2rem; }
tr.total td { font-weight: 600; }
table { border-collapse: collapse; width: 100%; margin-top: 0.5rem; }
th, td { border: 1px solid #ccc; padding: 0.25rem 0.5rem; text-align: left; }
th { background: #f4f4f4; }
td.tag, span.tag { color: #555; }";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html_handles_all_reserved_characters() {
        assert_eq!(escape_html(""), "");
        assert_eq!(escape_html("plain text"), "plain text");
        assert_eq!(escape_html("&<>\"'"), "&amp;&lt;&gt;&quot;&#39;");
        assert_eq!(
            escape_html("<script>alert('x & y')</script>"),
            "&lt;script&gt;alert(&#39;x &amp; y&#39;)&lt;/script&gt;"
        );
    }

    #[test]
    fn csv_field_quotes_breaks_and_neutralises_formulas() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("a\nb"), "\"a\nb\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("=1+1"), "'=1+1");
        assert_eq!(csv_field("@cmd"), "'@cmd");
    }

    #[test]
    fn format_aud_cents_handles_edge_cases() {
        assert_eq!(format_aud_cents(0), "$0.00");
        assert_eq!(format_aud_cents(5), "$0.05");
        assert_eq!(format_aud_cents(99), "$0.99");
        assert_eq!(format_aud_cents(100), "$1.00");
        assert_eq!(format_aud_cents(123), "$1.23");
        assert_eq!(format_aud_cents(123_456), "$1,234.56");
        assert_eq!(format_aud_cents(1_234_567), "$12,345.67");
        assert_eq!(format_aud_cents(100_000_000), "$1,000,000.00");
        assert_eq!(format_aud_cents(-12_345), "-$123.45");
    }
}
