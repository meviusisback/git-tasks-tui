//! UI state (`state.json`, mode 0600): the user's selection and view options.
//!
//! Same write discipline as the config: refuse symlinks, `0600`, atomic rename.

use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    pub selected_repos: Vec<String>,
    pub sort_key: String,
    pub ascending: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selected_repos: Vec::new(),
            sort_key: "updated".into(),
            ascending: false,
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
