//! gtasks — multi-repo GitHub issues & PRs in the terminal.
//!
//! Thin CLI wrapper (the TUI arrives in Task 5). No external commands are
//! spawned here, so S1–S4 hold trivially.

#![deny(clippy::print_stdout)]

use clap::Parser;
use gtasks::logging::{init_file_logging, init_stderr_logging};

/// gtasks: GitHub issues & PRs across repos, in the terminal.
#[derive(Debug, Parser)]
#[command(name = "gtasks", version, about)]
struct Args {}

fn main() -> anyhow::Result<()> {
    let _args = Args::parse();

    match init_file_logging() {
        Ok(path) => {
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                log = %path.display(),
                "gtasks starting"
            );
        }
        Err(err) => {
            eprintln!("gtasks: file logging unavailable ({err:#}); logging to stderr");
            init_stderr_logging();
        }
    }

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "interactive TUI lands in Task 5"
    );
    Ok(())
}
