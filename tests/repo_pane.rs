//! The repos pane lists what the user configured, including repos with no open
//! items, and picking one filters the list. A repo that is configured but empty
//! must be visible: hiding it looks like the config was ignored.

use gtasks::app::{App, Key, Msg};
use gtasks::model::{Item, ItemKind};

fn item(repo: &str, number: u64, title: &str) -> Item {
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

fn app_with_two_repos() -> App {
    let mut app = App::default();
    app.set_repos(vec!["me/empty".to_string(), "me/busy".to_string()]);
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![item("me/busy", 7, "seven")],
        failures: Vec::new(),
    });
    app
}

#[test]
fn a_configured_repo_with_no_items_is_still_listed() {
    let app = app_with_two_repos();
    assert!(
        app.repos.iter().any(|r| r == "me/empty"),
        "a repo with no open items disappeared from the pane: {:?}",
        app.repos
    );
}

#[test]
fn the_first_row_shows_everything() {
    let app = app_with_two_repos();
    assert_eq!(app.repos.first().map(String::as_str), Some("— all repos —"));
    assert_eq!(app.visible_items().len(), 1);
}

#[test]
fn picking_a_repo_filters_the_list_and_an_empty_one_empties_it() {
    let mut app = app_with_two_repos();
    // One keypress from the list: narrowing the list must not need discovery.
    app.update(Msg::Key(Key::Tab));
    assert_eq!(
        app.pane,
        gtasks::app::Pane::Repos,
        "Tab did not reach the repos pane"
    );
    app.update(Msg::Key(Key::Down)); // row 1: me/empty
    assert_eq!(
        app.repos_cursor, 1,
        "the cursor did not move in the repos pane"
    );
    assert!(
        app.visible_items().is_empty(),
        "the list still shows items from another repo: {:?}",
        app.visible_items()
            .iter()
            .map(|i| i.repo.clone())
            .collect::<Vec<_>>()
    );

    app.update(Msg::Key(Key::Down)); // row 2: me/busy
    assert_eq!(app.visible_items().len(), 1);

    app.update(Msg::Key(Key::Up)); // back to me/empty
    app.update(Msg::Key(Key::Up)); // row 0: all
    assert_eq!(app.visible_items().len(), 1);
    assert!(app.repo_filter.is_none(), "row 0 must clear the filter");
}

#[test]
fn acting_on_an_item_uses_that_items_repo_not_the_cursor() {
    // The cursor sits on the "all" row (index 0), whose text is not a repo slug:
    // if the action took its repo from the cursor, this could never work.
    let mut app = app_with_two_repos();
    app.status.clear();
    app.update(Msg::Key(Key::Char('a')));
    assert!(
        !app.status.contains("no repository"),
        "the action looked for a repo in the wrong place: {}",
        app.status
    );
}
