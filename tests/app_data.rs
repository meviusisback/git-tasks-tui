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
        app.repos.iter().map(|r| r.text()).collect::<Vec<_>>(),
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

#[test]
fn enter_requests_the_selected_body_and_does_not_repeat_it() {
    let mut app = App::default();
    app.apply_outcome(outcome());

    app.update(Msg::Key(Key::Enter));
    assert_eq!(
        app.detail_requested,
        Some(("acme/one".to_string(), 2)),
        "the item under the cursor is requested"
    );
    assert!(app.status.contains("loading #2"), "status: {}", app.status);

    // A second Enter while the body is on its way changes nothing.
    app.update(Msg::Key(Key::Enter));
    assert_eq!(app.detail_requested, Some(("acme/one".to_string(), 2)));

    app.apply_detail("acme/one", 2, "## Body".to_string());
    assert!(app.detail_requested.is_none(), "request settled");
    assert_eq!(app.detail, "## Body");
    assert_eq!(app.detail_for, Some(("acme/one".to_string(), 2)));
    assert!(
        app.status.is_empty(),
        "the loading line referred to this body: {status}",
        status = app.status
    );

    // Already loaded: no new request.
    app.update(Msg::Key(Key::Enter));
    assert!(app.detail_requested.is_none(), "cached body is reused");
}

#[test]
fn a_late_body_for_a_previous_selection_is_ignored() {
    let mut app = App::default();
    app.apply_outcome(outcome());
    app.list_cursor = 2;
    app.update(Msg::Key(Key::Enter)); // asks for #9
    assert_eq!(app.detail_requested, Some(("acme/two".to_string(), 9)));

    // The answer for the earlier item arrives late.
    app.apply_detail("acme/one", 2, "stale body".to_string());
    assert_eq!(app.detail, "", "stale body must not be shown");
    assert_eq!(app.detail_for, None);

    app.apply_detail("acme/two", 9, "fresh body".to_string());
    assert_eq!(app.detail, "fresh body");
}

#[test]
fn a_body_for_another_item_leaves_its_loading_line_alone() {
    // apply_detail only clears the status that named its own number: anything
    // else belongs to someone else (a refresh, a save, another item).
    let mut app = App::default();
    app.apply_outcome(outcome());
    app.list_cursor = 2;
    app.update(Msg::Key(Key::Enter)); // asks for #9
    assert!(app.status.contains("loading #9"));

    app.status = "saved".to_string();
    app.apply_detail("acme/two", 9, "fresh body".to_string());
    assert_eq!(app.detail, "fresh body");
    assert_eq!(app.status, "saved", "foreign status survives");
}

#[test]
fn o_asks_to_open_the_selected_item() {
    let mut app = App::default();
    app.apply_outcome(outcome());

    app.update(Msg::Key(Key::Char('o')));

    let url = app.open_requested.clone().expect("a safe url is requested");
    assert!(url.starts_with("https://github.com/"), "got {url}");
    assert!(app.status.contains("opening #2"), "status: {}", app.status);
}

#[test]
fn o_refuses_a_url_that_is_not_github() {
    let mut app = App::default();
    let mut outcome = outcome();
    outcome.items[0].url = "javascript:alert(1)".to_string();
    app.apply_outcome(outcome);

    app.update(Msg::Key(Key::Char('o')));

    assert!(app.open_requested.is_none(), "nothing must be launched");
    assert!(
        app.status.starts_with("cannot open"),
        "status: {}",
        app.status
    );
}
