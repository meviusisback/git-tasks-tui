//! gtasks library root.
//!
//! All application logic lives here so integration tests under `tests/`
//! can import it (`use gtasks::...`). `src/main.rs` stays a thin wrapper:
//! CLI parsing, logging init, process lifetime.

pub mod agent;
pub mod binaries;
pub mod config;
pub mod env;
pub mod gh;
pub mod logging;
pub mod model;
pub mod sanitize;
pub mod state;
pub mod worktree;
