//! Deselecting a repo in the R menu must hide its rows at once — not when the
//! next fetch happens to arrive — and the next fetch must read the new
//! selection, not the startup repo list.

use gtasks::actions::RepoSlug;
use gtasks::app::App;
use gtasks::fetch::FetchOutcome;
use gtasks::model::{Item, ItemKind};

fn slug(s: &str) -> RepoSlug {
    RepoSlug::parse(s).expect("test slug is valid")
}

fn item(number: u64, repo: &str) -> Item {
    Item {
        number,
        title: format!("thing {number}"),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        updated_at: "2026-10-20T10:00:00Z".into(),
        created_at: String::new(),
        url: format!("https://github.com/{repo}/pull/{number}"),
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

fn app_with_items() -> App {
    let mut app = App {
        all_repos: vec![slug("acme/one"), slug("acme/two")],
        ..App::default()
    };
    app.apply_outcome(FetchOutcome {
        items: vec![item(1, "acme/one"), item(2, "acme/two")],
        failures: Vec::new(),
    });
    app
}

#[test]
fn fetch_scope_empty_means_everything() {
    let all = vec![slug("acme/one"), slug("acme/two")];
    let scope = App::fetch_scope(&all, &[]);
    assert_eq!(scope.len(), 2, "empty selection fetches all");
}

#[test]
fn fetch_scope_follows_the_saved_selection() {
    let all = vec![slug("acme/one"), slug("acme/two")];
    let scope = App::fetch_scope(&all, &["acme/two".to_string()]);
    assert_eq!(
        scope.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec!["acme/two"]
    );
}

#[test]
fn fetch_scope_skips_invalid_slugs() {
    let all = vec![slug("acme/one")];
    let scope = App::fetch_scope(&all, &["nope".to_string()]);
    assert!(scope.is_empty(), "a typo must not break the fetch");
}

#[test]
fn deselecting_a_repo_hides_its_rows_at_once() {
    let mut app = app_with_items();
    assert_eq!(app.visible_items().len(), 2);

    app.apply_repo_selection(vec![slug("acme/one")]);

    let shown: Vec<u64> = app.visible_items().iter().map(|i| i.number).collect();
    assert_eq!(
        shown,
        vec![1],
        "modelforge-style rows are gone immediately: {shown:?}"
    );
    assert!(
        app.refresh_requested,
        "the server is re-read with the new set"
    );
    assert!(app.loading, "a second R while loading stays ignored");
}
