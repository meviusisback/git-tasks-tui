//! GitHub item models (`gh … --json`) and the pure list operations.
//!
//! Parsing is `serde` only: no I/O and no panics — a malformed payload becomes
//! an `Err`. The fetcher fills `repo` and `kind` after parsing, because a `gh`
//! payload is per-repo and per-kind and does not carry either field.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Whether an item came from `gh pr list` or `gh issue list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemKind {
    Pr,
    #[default]
    Issue,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Author {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
}

/// One entry of `statusCheckRollup`; `gh` varies the shape by provider, so
/// every field is optional and unknown keys are ignored.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub conclusion: Option<String>,
}

/// A pull request or an issue, as returned by the pinned `--json` field list.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub is_draft: Option<bool>,
    #[serde(default)]
    pub author: Option<Author>,
    #[serde(default)]
    pub labels: Vec<Label>,
    /// RFC3339 timestamps from `gh`; compared lexicographically (same `Z` form).
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub head_ref_name: Option<String>,
    #[serde(default)]
    pub review_decision: Option<String>,
    #[serde(default)]
    pub mergeable: Option<String>,
    #[serde(default)]
    pub status_check_rollup: Vec<Check>,
    /// Set by the fetcher: the payload does not say which repo it came from.
    #[serde(skip)]
    pub repo: String,
    /// Set by the fetcher: the payload does not say PR vs issue.
    #[serde(skip)]
    pub kind: ItemKind,
}

/// Parse a `gh … list --json` array. Errors carry the serde message; nothing
/// panics, however malformed the payload.
pub fn parse_list(json: &str) -> anyhow::Result<Vec<Item>> {
    serde_json::from_str(json).map_err(|e| anyhow::anyhow!("cannot parse gh JSON: {e}"))
}

/// Case-insensitive substring match over number, title, author and labels.
pub fn matches(item: &Item, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    if item.number.to_string().contains(&query) {
        return true;
    }
    if item.title.to_lowercase().contains(&query) {
        return true;
    }
    if let Some(author) = &item.author
        && author.login.to_lowercase().contains(&query)
    {
        return true;
    }
    item.labels
        .iter()
        .any(|label| label.name.to_lowercase().contains(&query))
}

/// Filter preserving the input order.
pub fn filter_items<'a>(items: &'a [Item], query: &str) -> Vec<&'a Item> {
    items.iter().filter(|item| matches(item, query)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Updated,
    Created,
    Number,
}

/// Stable sort by `key`, descending by default (`ascending = false`).
pub fn sort_items(items: &mut [Item], key: SortKey, ascending: bool) {
    items.sort_by(|a, b| {
        let order = match key {
            SortKey::Updated => a.updated_at.cmp(&b.updated_at),
            SortKey::Created => a.created_at.cmp(&b.created_at),
            SortKey::Number => a.number.cmp(&b.number),
        };
        if ascending { order } else { order.reverse() }
    });
}

/// Group by `repo`, preserving first-seen repo order and within-group order.
pub fn group_by_repo(items: &[Item]) -> Vec<(String, Vec<&Item>)> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
    for item in items {
        if !groups.contains_key(&item.repo) {
            order.push(item.repo.clone());
        }
        groups.entry(item.repo.clone()).or_default().push(item);
    }
    order
        .into_iter()
        .map(|repo| {
            let group = groups.remove(&repo).unwrap_or_default();
            (repo, group)
        })
        .collect()
}
