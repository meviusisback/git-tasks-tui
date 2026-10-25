//! Items cache: instant paint on startup, then a freshness check.
//!
//! The cache is only speed — the fetch stays the source of truth — so a
//! broken file is `None` (ignored), never an error the user sees.

use gtasks::model::{Item, ItemKind};

fn item(repo: &str, number: u64) -> Item {
    Item {
        number,
        title: format!("t{number}"),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        created_at: String::new(),
        updated_at: String::new(),
        url: String::new(),
        head_ref_name: None,
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        closing_issues_references: Vec::new(),
        repo: repo.to_string(),
        kind: ItemKind::Pr,
    }
}

#[test]
fn save_and_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("items-cache.json");
    let items = vec![item("me/a", 1), item("me/b", 2)];
    gtasks::cache::save(&path, &items).unwrap();
    assert_eq!(gtasks::cache::load(&path), Some(items));
}

#[test]
fn a_broken_cache_is_ignored_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("items-cache.json");
    std::fs::write(&path, "{not json").unwrap();
    assert_eq!(gtasks::cache::load(&path), None);
    assert_eq!(gtasks::cache::load(&dir.path().join("missing.json")), None);
}

#[test]
fn freshness_is_exact_equality() {
    let items = vec![item("me/a", 1)];
    assert!(gtasks::cache::is_fresh(&items, &items));
    assert!(!gtasks::cache::is_fresh(&items, &[item("me/a", 2)]));
    assert!(!gtasks::cache::is_fresh(&items, &[]));
}
