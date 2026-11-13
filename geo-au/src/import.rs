//! Builds a SQLite geocoding database from an unzipped G-NAF flat-file tree.
//!
//! Point [`import_directory`] at a directory containing (anywhere below it)
//! the standard per-state files:
//!
//! - `*_STATE_psv.psv`
//! - `*_LOCALITY_psv.psv`
//! - `*_STREET_LOCALITY_psv.psv`
//! - `*_ADDRESS_DETAIL_psv.psv`
//! - `*_ADDRESS_DEFAULT_GEOCODE_psv.psv`
//!
//! Files may be split per state; all matches are imported. Zip archives are
//! not read: unzip the release first (see `docs/GNAF.md`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::flatfile::{PsvReader, parse_f64, parse_text, parse_u32};

/// What an import produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportStats {
    pub addresses: u64,
    pub geocodes: u64,
    pub street_localities: u64,
    pub localities: u64,
    pub states: u64,
    /// Address rows skipped because a supporting row was missing.
    pub skipped_incomplete: u64,
    /// Address rows skipped because no geocode row existed.
    pub skipped_no_geocode: u64,
}

/// Classification of the PSV files found under a directory.
#[derive(Debug, Default)]
struct FileSet {
    states: Vec<PathBuf>,
    localities: Vec<PathBuf>,
    street_localities: Vec<PathBuf>,
    addresses: Vec<PathBuf>,
    geocodes: Vec<PathBuf>,
}

/// Imports every G-NAF flat file under `gnaf_dir` into the SQLite database at
/// `db_path` (replacing any previous contents).
///
/// The import runs with `journal_mode=OFF` and `synchronous=OFF` for bulk
/// speed; the database file must not be shared while it is being written.
pub fn import_directory(gnaf_dir: &Path, db_path: &Path) -> anyhow::Result<ImportStats> {
    let files = classify_files(gnaf_dir)?;
    if files.addresses.is_empty() {
        bail!(
            "no ADDRESS_DETAIL_psv file found under {} (unzip the G-NAF release first)",
            gnaf_dir.display()
        );
    }
    if files.states.is_empty() || files.localities.is_empty() || files.street_localities.is_empty()
    {
        bail!(
            "the G-NAF directory {} is missing support files (need STATE, LOCALITY and STREET_LOCALITY PSVs)",
            gnaf_dir.display()
        );
    }
    if files.geocodes.is_empty() {
        bail!(
            "the G-NAF directory {} has no ADDRESS_DEFAULT_GEOCODE files, so addresses cannot be positioned",
            gnaf_dir.display()
        );
    }

    let mut conn = Connection::open(db_path)
        .with_context(|| format!("opening database {}", db_path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         DROP TABLE IF EXISTS address;
         DROP TABLE IF EXISTS geocode;
         CREATE TABLE address(
             pid TEXT PRIMARY KEY,
             number_first INTEGER,
             number_last INTEGER,
             street TEXT NOT NULL,
             street_type TEXT,
             locality TEXT NOT NULL,
             state TEXT NOT NULL,
             postcode TEXT,
             lat REAL NOT NULL,
             lon REAL NOT NULL
         );
         CREATE INDEX address_street_upper_idx ON address(UPPER(street));
         CREATE INDEX address_locality_upper_idx ON address(UPPER(locality));
         CREATE INDEX address_state_upper_idx ON address(UPPER(state));
         CREATE INDEX address_postcode_idx ON address(postcode);
         CREATE INDEX address_lat_lon_idx ON address(lat, lon);
         CREATE TABLE geocode(pid TEXT PRIMARY KEY, lat REAL NOT NULL, lon REAL NOT NULL);",
    )
    .context("creating the schema")?;

    let mut stats = ImportStats::default();
    let states = import_states(&files.states, &mut stats)?;
    let localities = import_localities(&files.localities, &states, &mut stats)?;
    let street_localities =
        import_street_localities(&files.street_localities, &localities, &mut stats)?;
    import_geocodes(&mut conn, &files.geocodes, &mut stats)?;
    import_addresses(
        &mut conn,
        &files.addresses,
        &street_localities,
        &localities,
        &mut stats,
    )?;
    conn.execute_batch(
        "DROP TABLE IF EXISTS geocode;
         PRAGMA journal_mode = DELETE;
         PRAGMA synchronous = NORMAL;
         PRAGMA optimize;",
    )
    .context("dropping temporary geocode table and optimising")?;
    Ok(stats)
}

/// Walks `dir` recursively, classifying files by name.
fn classify_files(dir: &Path) -> anyhow::Result<FileSet> {
    let mut all = Vec::new();
    collect_files(dir, &mut all)?;
    let mut files = FileSet::default();
    for path in all {
        let name = path.file_name().map_or_else(String::new, |name| {
            name.to_string_lossy().to_ascii_uppercase()
        });
        if !(name.ends_with(".PSV") || name.ends_with(".CSV")) {
            continue;
        }
        if name.contains("STREET_LOCALITY_PSV") {
            files.street_localities.push(path);
        } else if name.contains("ADDRESS_DETAIL_PSV") {
            files.addresses.push(path);
        } else if name.contains("ADDRESS_DEFAULT_GEOCODE") {
            files.geocodes.push(path);
        } else if name.contains("LOCALITY_PSV") {
            files.localities.push(path);
        } else if name.contains("STATE_PSV") {
            files.states.push(path);
        }
    }
    Ok(files)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    out.sort();
    Ok(())
}

/// STATE_PID -> STATE_ABBREVIATION.
fn import_states(
    paths: &[PathBuf],
    stats: &mut ImportStats,
) -> anyhow::Result<HashMap<String, String>> {
    let mut states = HashMap::new();
    for path in paths {
        let mut reader = PsvReader::open(path)?;
        let (pid, abbreviation) = (
            reader.required("STATE_PID")?,
            reader.required("STATE_ABBREVIATION")?,
        );
        while let Some(row) = reader.next_row()? {
            if let (Some(pid), Some(abbreviation)) =
                (parse_text(row.get(pid)), parse_text(row.get(abbreviation)))
            {
                states.insert(pid, abbreviation);
                stats.states += 1;
            }
        }
    }
    Ok(states)
}

/// LOCALITY_PID -> (LOCALITY_NAME, state abbreviation).
fn import_localities(
    paths: &[PathBuf],
    states: &HashMap<String, String>,
    stats: &mut ImportStats,
) -> anyhow::Result<HashMap<String, (String, String)>> {
    let mut localities = HashMap::new();
    for path in paths {
        let mut reader = PsvReader::open(path)?;
        let (pid, name, state_pid) = (
            reader.required("LOCALITY_PID")?,
            reader.required("LOCALITY_NAME")?,
            reader.required("STATE_PID")?,
        );
        while let Some(row) = reader.next_row()? {
            let (Some(pid), Some(name), Some(state_pid)) = (
                parse_text(row.get(pid)),
                parse_text(row.get(name)),
                parse_text(row.get(state_pid)),
            ) else {
                continue;
            };
            if let Some(state) = states.get(&state_pid) {
                localities.insert(pid, (name, state.clone()));
                stats.localities += 1;
            }
        }
    }
    Ok(localities)
}

/// STREET_LOCALITY_PID -> (street name, street type code, LOCALITY_PID).
type StreetLocalities = HashMap<String, (String, Option<String>, String)>;

/// Imports STREET_LOCALITY rows for every locality already known.
fn import_street_localities(
    paths: &[PathBuf],
    localities: &HashMap<String, (String, String)>,
    stats: &mut ImportStats,
) -> anyhow::Result<StreetLocalities> {
    let mut streets = HashMap::new();
    for path in paths {
        let mut reader = PsvReader::open(path)?;
        let (pid, name, street_type, locality_pid) = (
            reader.required("STREET_LOCALITY_PID")?,
            reader.required("STREET_NAME")?,
            reader.index_of("STREET_TYPE_CODE"),
            reader.required("LOCALITY_PID")?,
        );
        while let Some(row) = reader.next_row()? {
            let (Some(pid), Some(name), Some(locality_pid)) = (
                parse_text(row.get(pid)),
                parse_text(row.get(name)),
                parse_text(row.get(locality_pid)),
            ) else {
                continue;
            };
            if localities.contains_key(&locality_pid) {
                let street_type = street_type.and_then(|index| parse_text(row.get(index)));
                streets.insert(pid, (name, street_type, locality_pid));
                stats.street_localities += 1;
            }
        }
    }
    Ok(streets)
}

/// ADDRESS_DETAIL_PID -> (lat, lon), into the `geocode` table.
fn import_geocodes(
    conn: &mut Connection,
    paths: &[PathBuf],
    stats: &mut ImportStats,
) -> anyhow::Result<()> {
    let tx = conn.transaction().context("starting geocode import")?;
    {
        let mut insert = tx
            .prepare("INSERT OR REPLACE INTO geocode(pid, lat, lon) VALUES (?1, ?2, ?3)")
            .context("preparing geocode insert")?;
        for path in paths {
            let mut reader = PsvReader::open(path)?;
            let (pid, latitude, longitude) = (
                reader.required("ADDRESS_DETAIL_PID")?,
                reader.required("LATITUDE")?,
                reader.required("LONGITUDE")?,
            );
            while let Some(row) = reader.next_row()? {
                let (Some(pid), Some(lat), Some(lon)) = (
                    parse_text(row.get(pid)),
                    parse_f64(row.get(latitude)),
                    parse_f64(row.get(longitude)),
                ) else {
                    continue;
                };
                insert.execute(params![pid, lat, lon])?;
                stats.geocodes += 1;
            }
        }
    }
    tx.commit().context("committing geocodes")?;
    Ok(())
}

/// Streams the address files, joining support rows and the geocode table.
fn import_addresses(
    conn: &mut Connection,
    paths: &[PathBuf],
    streets: &HashMap<String, (String, Option<String>, String)>,
    localities: &HashMap<String, (String, String)>,
    stats: &mut ImportStats,
) -> anyhow::Result<()> {
    let tx = conn.transaction().context("starting address import")?;
    {
        let mut insert = tx
            .prepare(
                "INSERT OR REPLACE INTO address(
                     pid, number_first, number_last, street, street_type,
                     locality, state, postcode, lat, lon
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .context("preparing address insert")?;
        let mut geocode = tx
            .prepare("SELECT lat, lon FROM geocode WHERE pid = ?1")
            .context("preparing geocode lookup")?;

        for path in paths {
            let mut reader = PsvReader::open(path)?;
            let (pid, number_first, number_last, street_locality_pid) = (
                reader.required("ADDRESS_DETAIL_PID")?,
                reader.index_of("NUMBER_FIRST"),
                reader.index_of("NUMBER_LAST"),
                reader.required("STREET_LOCALITY_PID")?,
            );
            let postcode = reader.index_of("POSTCODE");
            while let Some(row) = reader.next_row()? {
                let Some(pid) = parse_text(row.get(pid)) else {
                    stats.skipped_incomplete += 1;
                    continue;
                };
                let Some(street_locality_pid) = parse_text(row.get(street_locality_pid)) else {
                    stats.skipped_incomplete += 1;
                    continue;
                };
                let Some((street, street_type, locality_pid)) = streets.get(&street_locality_pid)
                else {
                    stats.skipped_incomplete += 1;
                    continue;
                };
                let Some((locality, state)) = localities.get(locality_pid) else {
                    stats.skipped_incomplete += 1;
                    continue;
                };
                let geocoded: Option<(f64, f64)> = geocode
                    .query_row([&pid], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?;
                let Some((lat, lon)) = geocoded else {
                    stats.skipped_no_geocode += 1;
                    continue;
                };
                insert.execute(params![
                    pid,
                    number_first.and_then(|index| parse_u32(row.get(index))),
                    number_last.and_then(|index| parse_u32(row.get(index))),
                    street,
                    street_type,
                    locality,
                    state,
                    postcode.and_then(|index| parse_text(row.get(index))),
                    lat,
                    lon,
                ])?;
                stats.addresses += 1;
            }
        }
    }
    tx.commit().context("committing addresses")?;
    Ok(())
}
