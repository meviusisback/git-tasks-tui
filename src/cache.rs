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
