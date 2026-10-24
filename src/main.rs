//! gtasks — multi-repo GitHub issues & PRs in the terminal.
//!
//! Task 1 scaffold: CLI parsing (`--version` via clap) and file logging.
//! The interactive TUI arrives in Task 5. No external commands are spawned
//! here, so S1–S4 hold trivially.

use std::fs::OpenOptions;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use tracing_subscriber::EnvFilter;

/// gtasks: GitHub issues & PRs across repos, in the terminal.
#[derive(Debug, Parser)]
#[command(name = "gtasks", version, about)]
struct Args {}

/// Resolve the log file path: `$XDG_STATE_HOME/gtasks/gtasks.log`
/// (falls back to `~/.local/state/gtasks/gtasks.log`).
fn log_file_path() -> anyhow::Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "gtasks")
        .context("cannot locate state directory")?;
    let state_dir = dirs.state_dir();
    debug_assert!(
        state_dir.is_some(),
        "ProjectDirs::state_dir is always Some on Linux"
    );
    let state_dir = state_dir.context("no state directory on this platform")?;
    Ok(state_dir.join("gtasks.log"))
}

/// Ensure `dir` exists with mode 0700 (S14), creating parents as needed.
fn ensure_state_dir(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot create state dir {}", dir.display()))?;
    // Harden pre-existing dirs too (create() is a no-op for those).
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("cannot chmod 0700 {}", dir.display()))?;
    Ok(())
}

/// Open (or create) the log file with mode 0600 (S14).
fn open_log_file(path: &std::path::Path) -> anyhow::Result<std::fs::File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot open log file {}", path.display()))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot chmod 0600 {}", path.display()))?;
    Ok(file)
}

/// Log level from `GTASKS_LOG` (a `tracing_subscriber::EnvFilter` directive),
/// defaulting to `info`.
fn log_filter() -> EnvFilter {
    EnvFilter::try_from_env("GTASKS_LOG").unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Initialise `tracing` to append to the state-dir log file (no ANSI codes).
/// Returns the log path so `main` can report it.
fn init_file_logging() -> anyhow::Result<PathBuf> {
    let path = log_file_path()?;
    if let Some(parent) = path.parent() {
        ensure_state_dir(parent)?;
    }
    let file = open_log_file(&path)?;
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .init();
    Ok(path)
}

/// Fallback when the log file cannot be opened: log to stderr instead of
/// failing to start.
fn init_stderr_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init()
        .ok();
}

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

    println!(
        "gtasks {} — interactive TUI lands in Task 5.",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
