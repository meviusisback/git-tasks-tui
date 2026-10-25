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

fn app_with_two_repos() -> App {
    let mut app = App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("me/empty").expect("valid slug"),
        gtasks::actions::RepoSlug::parse("me/busy").expect("valid slug"),
    ]);
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
        app.repos.iter().any(|r| r.text() == "me/empty"),
        "a repo with no open items disappeared from the pane: {:?}",
        app.repos
    );
}

#[test]
fn the_first_row_shows_everything() {
    let app = app_with_two_repos();
    assert_eq!(app.repos.first().map(|r| r.text()), Some("— all repos —"));
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
    assert!(
        matches!(app.repo_filter, gtasks::app::RepoFilter::All),
        "row 0 must clear the filter"
    );
}

#[test]
fn enter_in_the_repos_pane_drills_into_the_filtered_list() {
    let mut app = app_with_two_repos();
    app.update(Msg::Key(Key::Tab));
    assert_eq!(app.pane, gtasks::app::Pane::Repos);
    app.update(Msg::Key(Key::Down)); // row 1: me/empty
    app.update(Msg::Key(Key::Down)); // row 2: me/busy
    assert_eq!(app.visible_items().len(), 1);

    app.update(Msg::Key(Key::Enter));
    assert_eq!(
        app.pane,
        gtasks::app::Pane::List,
        "Enter in the repos pane drills in"
    );
    assert_eq!(app.visible_items().len(), 1, "the filter came along");
    assert_eq!(app.visible_items()[0].repo, "me/busy");
}

#[test]
fn esc_in_a_filtered_list_climbs_back_to_the_repos_pane() {
    let mut app = app_with_two_repos();
    app.update(Msg::Key(Key::Tab));
    app.update(Msg::Key(Key::Down));
    app.update(Msg::Key(Key::Down)); // me/busy, filter live
    app.update(Msg::Key(Key::Enter)); // drill in
    assert_eq!(app.pane, gtasks::app::Pane::List);

    app.update(Msg::Key(Key::Esc));
    assert!(!app.quit, "drilling out is not quitting");
    assert_eq!(app.pane, gtasks::app::Pane::Repos, "Esc drills out");
    assert_eq!(
        app.repos_cursor, 2,
        "drilling out keeps the repo position: no re-searching"
    );
    assert_eq!(
        app.visible_items().len(),
        1,
        "the filter survived the round trip"
    );

    // Row 0 clears the filter: Esc at the top level does nothing (the shell
    // rule: Esc climbs and stops, only q quits).
    app.update(Msg::Key(Key::Up)); // me/empty
    app.update(Msg::Key(Key::Up)); // row 0: all
    app.update(Msg::Key(Key::Enter)); // drill in, no filter
    assert!(matches!(app.repo_filter, gtasks::app::RepoFilter::All));
    app.update(Msg::Key(Key::Esc));
    assert!(!app.quit, "nothing to climb back to: Esc stays");
    assert_eq!(app.pane, gtasks::app::Pane::List);
}

#[test]
fn status_says_where_you_are() {
    let mut app = app_with_two_repos();
    app.update(Msg::Key(Key::Tab)); // repos pane
    app.update(Msg::Key(Key::Down)); // row 1: me/empty
    assert_eq!(app.status, "0 items in me/empty");
    app.update(Msg::Key(Key::Up)); // row 0: all
    assert_eq!(app.status, "— all repos — · 1 items");
}

#[test]
fn legend_follows_the_drill_mode() {
    let mut app = app_with_two_repos();
    app.update(Msg::Key(Key::Tab)); // repos pane
    let repos_legend = app.legend();
    assert!(repos_legend.contains("Enter drill in"), "{repos_legend}");
    // S-6a: the note is a *message*, not a key hint. In the legend it was 24
    // characters that pushed every real status off the right edge at 80 columns.
    assert!(!repos_legend.contains("nothing to close"), "{repos_legend}");
    assert!(!repos_legend.contains("Esc back"), "{repos_legend}");
    app.update(Msg::Key(Key::Esc));
    assert_eq!(app.status, "Esc — nothing to close");

    app.update(Msg::Key(Key::Down));
    app.update(Msg::Key(Key::Down)); // me/busy, filter live
    app.update(Msg::Key(Key::Enter)); // drill in
    let drilled_legend = app.legend();
    assert!(drilled_legend.contains("Esc back"), "{drilled_legend}");
    assert!(
        !drilled_legend.contains("nothing to close"),
        "{drilled_legend}"
    );
}

#[test]
fn esc_in_the_repos_pane_with_a_filter_clears_it() {
    let mut app = app_with_two_repos();
    app.update(Msg::Key(Key::Tab));
    app.update(Msg::Key(Key::Down));
    app.update(Msg::Key(Key::Down)); // me/busy, filter live
    app.update(Msg::Key(Key::Enter)); // drill in
    app.update(Msg::Key(Key::Esc)); // drill out: Repos, cursor kept
    assert_eq!(app.pane, gtasks::app::Pane::Repos);
    assert_eq!(app.repos_cursor, 2);
    assert!(matches!(app.repo_filter, gtasks::app::RepoFilter::One(_)));

    app.update(Msg::Key(Key::Esc)); // second Esc: clear
    assert_eq!(app.pane, gtasks::app::Pane::Repos);
    assert_eq!(app.repos_cursor, 0, "Esc goes home to row 0");
    assert!(matches!(app.repo_filter, gtasks::app::RepoFilter::All));
    assert_eq!(app.status, "— all repos — · 1 items");

    app.update(Msg::Key(Key::Esc)); // no filter: no-op, no quit
    assert!(!app.quit);
    assert_eq!(app.repos_cursor, 0);
}

#[test]
fn drill_out_keeps_the_cursor_where_drill_in_left_it() {
    // The cursor never moves while drilled in, so the row it left is still
    // selected by construction: Esc only changes the pane, and the filter
    // (hence the visible list) is untouched.
    let mut app = app_with_two_repos();
    app.update(Msg::Key(Key::Tab));
    app.update(Msg::Key(Key::Down));
    app.update(Msg::Key(Key::Down)); // me/busy, row 2
    app.update(Msg::Key(Key::Enter)); // drill in
    assert_eq!(app.pane, gtasks::app::Pane::List);

    app.update(Msg::Key(Key::Esc)); // drill out
    assert_eq!(app.pane, gtasks::app::Pane::Repos);
    assert_eq!(app.repos_cursor, 2, "the cursor never moved");
    assert!(
        matches!(app.repo_filter, gtasks::app::RepoFilter::One(_)),
        "the filter is untouched"
    );
    assert_eq!(app.visible_items().len(), 1);
}

#[test]
fn set_repos_builds_typed_rows() {
    // No strings cross this boundary: row 0 is All, the rest are Repo.
    // Nothing downstream parses, and there is no sentinel to compare.
    let app = app_with_two_repos();
    let texts: Vec<&str> = app.repos.iter().map(|r| r.text()).collect();
    assert_eq!(texts, vec!["— all repos —", "me/empty", "me/busy"]);
    assert!(matches!(app.repos[0], gtasks::app::RepoRow::All));
    assert!(matches!(app.repos[1], gtasks::app::RepoRow::Repo(_)));
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
