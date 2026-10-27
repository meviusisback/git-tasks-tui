//! UI state (`state.json`, mode 0600): the user's selection and view options.
//!
//! Same write discipline as the config: refuse symlinks, `0600`, atomic rename.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// One open tab: the issue/PR the user opened with `Enter`.
/// Titles are resolved live from the fetched items, so only the key is
/// stored — the session survives a restart before the first fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabState {
    pub repo: String,
    pub number: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    pub selected_repos: Vec<String>,
    pub sort_key: String,
    pub ascending: bool,
    /// Last focused pane (`repos`/`list`/`detail`/`agent`); unknown values
    /// fall back to `list` on restore, never a crash.
    pub pane: String,
    pub repos_cursor: usize,
    pub list_cursor: usize,
    pub query: String,
    /// `None`/empty means "all"; otherwise a validated `owner/name` slug.
    pub repo_filter: Option<String>,
    /// Open tabs, in order, and the active one.
    pub tabs: Vec<TabState>,
    pub active_tab: Option<usize>,
    /// The cross geometry, in percent (left width, top height): dragged
    /// with the mouse, restored on start.
    pub col_pct: u16,
    pub row_pct: u16,
    /// The agent last launched in each worktree, by tree path: reopening
    /// the tree offers that agent's previous session instead of a fresh
    /// start. Unknown agent names are simply never matched, so a stale
    /// entry is inert, never an error.
    pub last_agents: HashMap<String, crate::agent::LastAgent>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selected_repos: Vec::new(),
            sort_key: "updated".into(),
            ascending: false,
            // A fresh start lands on the repos: the left column is repos on
            // top and issues below, so the first keypress picks a repo.
            pane: String::from("repos"),
            repos_cursor: 0,
            list_cursor: 0,
            query: String::new(),
            repo_filter: None,
            tabs: Vec::new(),
            active_tab: None,
            col_pct: 30,
            row_pct: 38,
            last_agents: HashMap::new(),
        }
    }
}

impl State {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = crate::config::read_private(path)?;
        let state: Self = serde_json::from_str(&text)
            .with_context(|| format!("cannot parse {}", path.display()))?;
        Ok(state)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        for repo in &self.selected_repos {
            crate::config::validate_repo_slug(repo)?;
        }
        let text = serde_json::to_string_pretty(self).context("cannot serialize the state")?;
        crate::config::write_private(path, text.as_bytes())
    }
}
