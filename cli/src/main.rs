//! `grocery-routes` binary entry point.
//!
//! Parsing and dispatch only; every command lives in the `cli` library so it
//! can be tested without spawning a subprocess.

use clap::Parser as _;

use cli::commands::{self, Cli};

fn main() -> anyhow::Result<()> {
    commands::run(Cli::parse())
}
