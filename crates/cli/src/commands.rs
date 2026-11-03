//! Subcommand implementations for the `grocery-routes` binary.
//!
//! `main.rs` stays a thin wrapper; every command lives here so it can be
//! exercised from unit and integration tests. Command functions may print
//! summaries to stdout — nothing else in the library prints.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use clap::{Args, Parser, Subcommand, ValueEnum};
use domain::{Chain, Load, Node, Problem, Solution};

use crate::export::{format_aud_cents, route_sheet_html, routes_geojson};

/// The `grocery-routes` command line.
#[derive(Debug, Parser)]
#[command(
    name = "grocery-routes",
    version,
    about = "Route and delivery planning for Australian supermarket distribution"
)]
pub struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// The M0 subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Generate a seeded synthetic problem for one chain and city.
    GenSim(GenSimArgs),
    /// Solve a problem and write the validated plan as JSON.
    Solve(SolveArgs),
    /// Write an HTML route sheet and a GeoJSON report for a solved plan.
    Export(ExportArgs),
}

/// Arguments of `gen-sim`.
#[derive(Debug, Args)]
pub struct GenSimArgs {
    /// Grocery banner whose store network is generated.
    #[arg(long, value_enum, default_value_t = ChainArg::Coles)]
    pub chain: ChainArg,
    /// City the network is generated around.
    #[arg(long, value_enum, default_value_t = CityArg::Sydney)]
    pub city: CityArg,
    /// Number of stores (the M0 solver accepts at most 5,000 nodes).
    #[arg(long, default_value_t = 40)]
    pub stores: usize,
    /// Number of distribution centres.
    #[arg(long, default_value_t = 3)]
    pub depots: usize,
    /// Number of vehicles (one driver is generated per vehicle).
    #[arg(long, default_value_t = 12)]
    pub vehicles: usize,
    /// Random seed: the same seed always yields the same problem.
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    /// File the problem JSON is written to.
    #[arg(long)]
    pub out: PathBuf,
}

/// Arguments of `solve`.
#[derive(Debug, Args)]
pub struct SolveArgs {
    /// Problem JSON to read.
    #[arg(long)]
    pub problem: PathBuf,
    /// File the solution JSON is written to.
    #[arg(long)]
    pub out: PathBuf,
}

/// Arguments of `export`.
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Problem JSON to read.
    #[arg(long)]
    pub problem: PathBuf,
    /// Solution JSON to read.
    #[arg(long)]
    pub solution: PathBuf,
    /// Directory `route-sheet.html` and `routes.geojson` are written to.
    #[arg(long)]
    pub out_dir: PathBuf,
}

/// Runs the selected subcommand.
pub fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::GenSim(args) => run_gen_sim(&args),
        Command::Solve(args) => run_solve(&args),
        Command::Export(args) => run_export(&args),
    }
}

/// `gen-sim`: generates a synthetic problem, writes it as pretty JSON and
/// summarises demand and fleet capacity per compartment.
pub fn run_gen_sim(args: &GenSimArgs) -> anyhow::Result<()> {
    let options = sim::SimOptions {
        chain: args.chain.into(),
        city: args.city.into(),
        stores: args.stores,
        depots: args.depots,
        vehicles: args.vehicles,
        seed: args.seed,
    };
    let problem = sim::generate(&options);
    write_problem_json(&args.out, &problem)?;

    let demand = sum_load(problem.stores.iter().map(|store| store.demand));
    let capacity = sum_load(problem.vehicles.iter().map(|vehicle| vehicle.capacity));
    println!("generated '{}' (seed {})", problem.name, args.seed);
    println!(
        "  stores: {}, depots: {}, vehicles: {}",
        problem.stores.len(),
        problem.depots.len(),
        problem.vehicles.len()
    );
    println!(
        "  total demand: ambient {}, chilled {}, frozen {} ({} units)",
        demand.ambient,
        demand.chilled,
        demand.frozen,
        demand.total()
    );
    println!(
        "  fleet capacity: ambient {}, chilled {}, frozen {} ({} units)",
        capacity.ambient,
        capacity.chilled,
        capacity.frozen,
        capacity.total()
    );
    println!("wrote {}", args.out.display());
    Ok(())
}

/// The M0 solver builds an O(n²) distance matrix (≈200 MB at the ceiling);
/// refuse problems that would blow its memory budget instead of aborting
/// mid-build.
const MAX_SOLVE_NODES: usize = 5_000;

/// `solve`: solves a problem, refuses to write an invalid plan and summarises
/// the result.
pub fn run_solve(args: &SolveArgs) -> anyhow::Result<()> {
    let problem = read_problem_json(&args.problem)?;
    ensure_solvable_size(&problem)?;
    let solution = solver::solve(&problem);
    check_solution(&problem, &solution)?;
    write_solution_json(&args.out, &solution)?;

    let served = solution
        .routes
        .iter()
        .flat_map(|route| &route.steps)
        .filter(|step| matches!(step.node, Node::Store(_)))
        .count();
    println!(
        "solved '{}': {} routes used, {} of {} stores served",
        problem.name,
        solution.routes.len(),
        served,
        problem.stores.len()
    );
    match solution.unassigned.as_slice() {
        [] => println!("  unassigned: none"),
        ids => {
            let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
            println!("  unassigned: {} (ids: {})", ids.len(), ids.join(", "));
        }
    }
    println!("  distance: {:.1} km", solution.total_distance_km);
    println!("  duration: {}", solution.total_duration);
    println!("  cost: {}", format_aud_cents(solution.total_cost_cents));
    println!("wrote {}", args.out.display());
    Ok(())
}

/// Refuses problems whose node count exceeds the M0 solver's memory budget.
fn ensure_solvable_size(problem: &Problem) -> anyhow::Result<()> {
    let nodes = problem.depots.len() + problem.stores.len();
    anyhow::ensure!(
        nodes <= MAX_SOLVE_NODES,
        "problem has {nodes} nodes; the M0 matrix solver supports up to {MAX_SOLVE_NODES} — regenerate with fewer stores/depots"
    );
    Ok(())
}

/// `export`: re-validates a plan and writes the HTML route sheet and GeoJSON
/// report into `--out-dir`, creating the directory when missing.
pub fn run_export(args: &ExportArgs) -> anyhow::Result<()> {
    let problem = read_problem_json(&args.problem)?;
    let solution = read_solution_json(&args.solution)?;
    check_solution(&problem, &solution)?;

    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("creating output directory {}", args.out_dir.display()))?;
    let sheet_path = args.out_dir.join("route-sheet.html");
    let geojson_path = args.out_dir.join("routes.geojson");
    std::fs::write(&sheet_path, route_sheet_html(&problem, &solution))
        .with_context(|| format!("writing {}", sheet_path.display()))?;
    std::fs::write(&geojson_path, routes_geojson(&problem, &solution))
        .with_context(|| format!("writing {}", geojson_path.display()))?;
    println!("route sheet: {}", sheet_path.display());
    println!("geojson: {}", geojson_path.display());
    Ok(())
}

/// Validates `solution` against `problem`, printing every violation to
/// stderr before failing.
fn check_solution(problem: &Problem, solution: &Solution) -> anyhow::Result<()> {
    if let Err(violations) = solution.validate(problem) {
        for violation in &violations {
            eprintln!("violation: {violation}");
        }
        anyhow::bail!(
            "solution failed validation ({} violation(s))",
            violations.len()
        );
    }
    Ok(())
}

/// Sums a sequence of loads compartment by compartment.
fn sum_load(loads: impl Iterator<Item = Load>) -> Load {
    loads.fold(Load::ZERO, |total, load| total + load)
}

/// Writes `problem` as pretty JSON with a trailing newline.
fn write_problem_json(path: &Path, problem: &Problem) -> anyhow::Result<()> {
    let mut json = serde_json::to_string_pretty(problem).context("serializing problem JSON")?;
    json.push('\n');
    write_text(path, &json)
}

/// Writes `solution` as pretty JSON with a trailing newline.
fn write_solution_json(path: &Path, solution: &Solution) -> anyhow::Result<()> {
    let mut json = serde_json::to_string_pretty(solution).context("serializing solution JSON")?;
    json.push('\n');
    write_text(path, &json)
}

/// Maximum size of an input JSON file accepted by the CLI (100 MiB).
/// M0 problem files are ~100 KiB; this cap is far above legitimate use.
const MAX_JSON_BYTES: u64 = 100 * 1024 * 1024;

/// Reads and parses a problem JSON document, enforcing a size cap and
/// using a bounded deserializer to avoid OOM/stack DoS from malicious input.
fn read_problem_json(path: &Path) -> anyhow::Result<Problem> {
    check_json_size(path)?;
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = std::io::BufReader::with_capacity(8 * 1024, file);
    serde_json::from_reader(reader).with_context(|| format!("parsing {}", path.display()))
}

/// Reads and parses a solution JSON document, enforcing a size cap and
/// using a bounded deserializer to avoid OOM/stack DoS from malicious input.
fn read_solution_json(path: &Path) -> anyhow::Result<Solution> {
    check_json_size(path)?;
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = std::io::BufReader::with_capacity(8 * 1024, file);
    serde_json::from_reader(reader).with_context(|| format!("parsing {}", path.display()))
}

/// Validates that `path` is a regular file not larger than `MAX_JSON_BYTES`.
fn check_json_size(path: &Path) -> anyhow::Result<()> {
    let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if !meta.is_file() {
        anyhow::bail!("{} is not a regular file", path.display());
    }
    if meta.len() > MAX_JSON_BYTES {
        anyhow::bail!(
            "{} is {} bytes; maximum accepted size is {} bytes",
            path.display(),
            meta.len(),
            MAX_JSON_BYTES
        );
    }
    Ok(())
}

/// Writes a text file, creating the parent directory when needed.
/// On Unix, created directories are owner-only (0o700) to avoid leaking
/// generated route plans to other local users in shared environments.
fn write_text(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

/// Grocery banner as accepted on the command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ChainArg {
    /// Coles.
    Coles,
    /// Woolworths.
    Woolworths,
    /// ALDI.
    Aldi,
    /// IGA.
    Iga,
}

impl fmt::Display for ChainArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Coles => "coles",
            Self::Woolworths => "woolworths",
            Self::Aldi => "aldi",
            Self::Iga => "iga",
        })
    }
}

impl From<ChainArg> for Chain {
    fn from(value: ChainArg) -> Self {
        match value {
            ChainArg::Coles => Self::Coles,
            ChainArg::Woolworths => Self::Woolworths,
            ChainArg::Aldi => Self::Aldi,
            ChainArg::Iga => Self::Iga,
        }
    }
}

/// City as accepted on the command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum CityArg {
    /// Sydney.
    Sydney,
    /// Melbourne.
    Melbourne,
    /// Brisbane.
    Brisbane,
    /// Perth.
    Perth,
    /// Adelaide.
    Adelaide,
}

impl fmt::Display for CityArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sydney => "sydney",
            Self::Melbourne => "melbourne",
            Self::Brisbane => "brisbane",
            Self::Perth => "perth",
            Self::Adelaide => "adelaide",
        })
    }
}

impl From<CityArg> for sim::City {
    fn from(value: CityArg) -> Self {
        match value {
            CityArg::Sydney => Self::Sydney,
            CityArg::Melbourne => Self::Melbourne,
            CityArg::Brisbane => Self::Brisbane,
            CityArg::Perth => Self::Perth,
            CityArg::Adelaide => Self::Adelaide,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_solvable_size;
    use domain::{
        Chain, CostModel, Depot, DepotId, Driver, DriverId, LatLon, Load, Minutes, Problem, Shift,
        Store, StoreId, TimeWindow, Vehicle, VehicleId,
    };

    fn problem_with_stores(count: usize) -> Problem {
        Problem {
            name: "size test".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "Depot".into(),
                location: LatLon::new(-33.87, 151.21),
            }],
            stores: (0..count)
                .map(|index| Store {
                    id: StoreId(index as u32),
                    chain: Chain::Coles,
                    name: format!("Coles #{index}"),
                    location: LatLon::new(-33.90, 151.20),
                    demand: Load::new(1, 0, 0),
                    service: Minutes::from_minutes(5),
                    receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
                })
                .collect(),
            vehicles: vec![Vehicle {
                id: VehicleId(0),
                name: "Rigid".into(),
                depot: DepotId(0),
                capacity: Load::new(1, 0, 0),
                max_duration: Minutes::from_minutes(600),
                avg_speed_kmh: 60.0,
            }],
            drivers: vec![Driver {
                id: DriverId(0),
                name: "Sam".into(),
                shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(600))],
            }],
            cost: CostModel {
                fixed_per_route_cents: 0,
                per_km_cents: 0,
                per_hour_cents: 0,
                late_per_min_cents: 0,
            },
        }
    }

    #[test]
    fn solve_ceiling_accepts_five_thousand_nodes() {
        assert!(ensure_solvable_size(&problem_with_stores(4_999)).is_ok());
    }

    #[test]
    fn solve_ceiling_rejects_oversized_problems() {
        let error = ensure_solvable_size(&problem_with_stores(5_000)).unwrap_err();
        assert!(error.to_string().contains("5000"), "got: {error}");
    }
}
