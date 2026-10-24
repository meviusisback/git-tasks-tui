//! gtasks library root.
//!
//! All application logic lives here so integration tests under `tests/`
//! can import it (`use gtasks::...`). `src/main.rs` stays a thin wrapper:
//! CLI parsing, logging init, process lifetime.

pub mod actions;
pub mod actions_ui;
pub mod agent;
pub mod app;
pub mod binaries;
pub mod config;
pub mod env;
pub mod fetch;
pub mod gh;
pub mod input;
pub mod logging;
pub mod markdown;
pub mod model;
pub mod sanitize;
pub mod state;
pub mod ui;
pub mod worktree;
