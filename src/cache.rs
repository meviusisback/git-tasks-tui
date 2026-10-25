//! Items cache: instant paint, then a freshness check.
//!
//! The first fetch is the slowest part of startup (69 repos over GraphQL).
//! So the last good list is kept on disk next to `state.json` (same private
//! write discipline: no symlinks, `0600`, atomic rename). Startup paints the
//! cache immediately and refreshes in the background; when the fetch lands,
//! the list is compared — unchanged means "up to date", changed means the
//! screen and the cache are replaced together.

use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::model::Item;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cache {
    saved_at: u64,
    items: Vec<Item>,
}

/// Read the cached list, if any. A broken cache is `None`, never an error:
/// the fetch behind it is the source of truth.
pub fn load(path: &Path) -> Option<Vec<Item>> {
    let text = crate::config::read_private(path).ok()?;
    let cache: Cache = serde_json::from_str(&text).ok()?;
    Some(cache.items)
}

/// Replace the cache with the fresh list.
pub fn save(path: &Path, items: &[Item]) -> anyhow::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let text = serde_json::to_string(&Cache {
        saved_at: now,
        items: items.to_vec(),
    })
    .context("cannot serialize the items cache")?;
    crate::config::write_private(path, text.as_bytes())
}

/// True when the fetch brought nothing new: same items, same order.
pub fn is_fresh(cached: &[Item], fresh: &[Item]) -> bool {
    cached == fresh
}

/// One cached pane: the body (or diff) plus the item's `updated_at` when it
/// was fetched. A later list carrying the same `updated_at` means the cache
/// is current — no fetch at all. A different one means stale: shown at
/// once, revalidated in the background. The text is the full fetch; any
/// display cap (the diff's 200 lines) applies at render, never here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BodyCache {
    pub saved_at: u64,
    pub updated_at: String,
    pub text: String,
}

/// What to do with a cached pane for the item's current `updated_at`.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolve {
    /// Same `updated_at`: show it, fetch nothing.
    Fresh(String),
    /// Changed (or first fetch recorded nothing): show it, refetch behind.
    Stale(String),
    /// Nothing on disk: fetch, show the loading state meanwhile.
    Missing,
}

/// The cached pane for this `updated_at`, if any.
pub fn resolve(cached: Option<BodyCache>, updated_at: &str) -> Resolve {
    match cached {
        Some(entry) if entry.updated_at == updated_at => Resolve::Fresh(entry.text),
        Some(entry) => Resolve::Stale(entry.text),
        None => Resolve::Missing,
    }
}

/// `dir/<owner>/<name>/<number>.<kind>.json`, `kind` being `body` or `diff`.
/// The slash in `owner/name` becomes real subdirectories; a broken file is
/// `None`, never an error.
fn pane_path(dir: &Path, repo: &str, number: u64, kind: &str) -> std::path::PathBuf {
    dir.join(repo).join(format!("{number}.{kind}.json"))
}

fn load_pane(dir: &Path, repo: &str, number: u64, kind: &str) -> Option<BodyCache> {
    let text = crate::config::read_private(&pane_path(dir, repo, number, kind)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_pane(
    dir: &Path,
    repo: &str,
    number: u64,
    kind: &str,
    updated_at: &str,
    text: &str,
) -> anyhow::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let payload = serde_json::to_string(&BodyCache {
        saved_at: now,
        updated_at: updated_at.to_string(),
        text: text.to_string(),
    })
    .context("cannot serialize the body cache")?;
    crate::config::write_private(&pane_path(dir, repo, number, kind), payload.as_bytes())
}

/// Read the cached body, if any.
pub fn load_detail(dir: &Path, repo: &str, number: u64) -> Option<BodyCache> {
    load_pane(dir, repo, number, "body")
}

/// Replace the cached body.
pub fn save_detail(
    dir: &Path,
    repo: &str,
    number: u64,
    updated_at: &str,
    text: &str,
) -> anyhow::Result<()> {
    save_pane(dir, repo, number, "body", updated_at, text)
}

/// Read the cached diff, if any.
pub fn load_diff(dir: &Path, repo: &str, number: u64) -> Option<BodyCache> {
    load_pane(dir, repo, number, "diff")
}

/// Replace the cached diff.
pub fn save_diff(
    dir: &Path,
    repo: &str,
    number: u64,
    updated_at: &str,
    text: &str,
) -> anyhow::Result<()> {
    save_pane(dir, repo, number, "diff", updated_at, text)
}
