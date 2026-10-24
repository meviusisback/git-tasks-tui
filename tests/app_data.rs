//! Task 7b wiring: fetched data into the app state, and `r` single-flight.

use gtasks::app::{App, Key, Msg};
use gtasks::fetch::FetchOutcome;
use gtasks::model::{Item, ItemKind};
use gtasks::ui::view_for;

fn item(number: u64, title: &str, repo: &str) -> Item {
    Item {
        number,
        title: title.to_string(),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: Vec::new(),
        updated_at: "2026-10-20T10:00:00Z".into(),
        created_at: String::new(),
        url: format!("https://github.com/{repo}/pull/{number}"),
        head_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        repo: repo.to_string(),
        kind: ItemKind::Pr,
    }
}

fn outcome() -> FetchOutcome {
    FetchOutcome {
        items: vec![
            item(2, "Second thing", "acme/one"),
            item(1, "First thing", "acme/one"),
            item(9, "Other repo item", "acme/two"),
        ],
        failures: Vec::new(),
    }
}

#[test]
fn a_finished_fetch_fills_the_state_and_keeps_repo_order() {
    let mut app = App {
        loading: true,
        ..App::default()
    };

    app.apply_outcome(outcome());

    assert!(!app.loading, "the spinner stops when data arrives");
    assert_eq!(app.items.len(), 3);
    assert_eq!(
        app.repos,
        vec!["acme/one", "acme/two"],
        "unique, first-seen order"
    );
    assert_eq!(app.repos_len, 2);
    assert_eq!(app.list_len, 3);
    assert_eq!(app.failed, 0);
    assert!(app.status.contains("3 items"), "status: {}", app.status);
}

#[test]
fn a_partial_fetch_says_how_many_failed() {
    let mut app = App::default();
    let mut outcome = outcome();
    outcome.failures = vec![("acme/three".to_string(), "timed out".to_string())];

    app.apply_outcome(outcome);

    assert_eq!(app.failed, 1);
    assert!(app.status.contains("1 failed"), "status: {}", app.status);
}

#[test]
fn the_search_box_filters_by_title_and_number() {
    let mut app = App::default();
    app.apply_outcome(outcome());

    app.query = "OTHER".to_string();
    let found: Vec<u64> = app.visible_items().iter().map(|i| i.number).collect();
    assert_eq!(found, vec![9], "case-insensitive on the title");

    app.query = "1".to_string();
    let found: Vec<u64> = app.visible_items().iter().map(|i| i.number).collect();
    assert!(found.contains(&1), "the number is searchable");

    app.query = "   ".to_string();
    assert_eq!(app.visible_items().len(), 3, "blank query shows everything");
}

#[test]
fn r_asks_for_one_refresh_at_a_time() {
    let mut app = App::default();

    app.update(Msg::Key(Key::Char('r')));
    assert!(app.refresh_requested, "first r asks for a refresh");
    assert!(app.loading);
    assert!(app.status.contains("refreshing"), "status: {}", app.status);

    // The consumer clears the flag and the fetch is still in flight.
    app.refresh_requested = false;
    app.update(Msg::Key(Key::Char('r')));
    assert!(
        !app.refresh_requested,
        "a second r while loading is ignored"
    );
}

#[test]
fn the_view_shows_the_selected_item_detail() {
    let mut app = App::default();
    app.apply_outcome(outcome());
    app.list_cursor = 2;

    let view = view_for(&app);
    assert_eq!(view.repos, vec!["acme/one", "acme/two"]);
    assert_eq!(view.items.len(), 3);
    assert!(view.items[0].starts_with("#2 Second thing"));
    assert!(
        view.detail.contains("#9 Other repo item"),
        "detail: {}",
        view.detail
    );
    assert!(view.detail.contains("acme/two"));
}
