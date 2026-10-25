//! A count of one reads as singular, in every message that counts something.
//!
//! Today the app writes five phrases by hand (`format!("{shown} items in {slug}")` and
//! friends), so a single item is reported as "1 items" and a single repo as "1 repos".
//! The fix this specifies: **one helper, one place**, and the three messages the app can
//! compose itself must use it.
//!
//! These assertions are wording-tolerant on purpose: they demand the singular for one and
//! the plural for many, not a particular sentence. A test that pins the whole sentence
//! would fail on the next rewording and teach people to edit the test instead of the
//! message.

use gtasks::actions::RepoSlug;
use gtasks::app::{App, Key, Msg, RepoFilter};
use gtasks::model::{Item, ItemKind};

fn slug(s: &str) -> RepoSlug {
    RepoSlug::parse(s).expect("slug")
}

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
        assignees: Vec::new(),
        base_ref_name: None,
        closing_issues_references: Vec::new(),
    }
}

/// One repo that answers, one that does not, and a fetch result with `items` items.
fn app_with(repos: &[&str], items: Vec<Item>) -> App {
    let mut app = App::default();
    let slugs: Vec<RepoSlug> = repos.iter().map(|r| slug(r)).collect();
    app.set_repos(slugs.clone());
    app.all_repos = slugs;
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items,
        failures: Vec::new(),
    });
    app
}

#[test]
fn a_fetch_of_one_item_and_one_repo_reads_as_singular() {
    // app.rs: the message composed when a fetch lands.
    let app = app_with(&["me/only"], vec![item("me/only", 7, "seven")]);
    println!("fetch status: {:?}", app.status);
    assert!(
        app.status.contains("1 item"),
        "one item must be singular: {:?}",
        app.status
    );
    assert!(
        !app.status.contains("1 items"),
        "one item is not '1 items': {:?}",
        app.status
    );
    assert!(
        app.status.contains("1 repo"),
        "one repo must be singular: {:?}",
        app.status
    );
    assert!(
        !app.status.contains("1 repos"),
        "one repo is not '1 repos': {:?}",
        app.status
    );
}

#[test]
fn filtering_to_a_repo_with_one_item_reads_as_singular() {
    // app.rs: the message composed when the filter changes.
    let mut app = app_with(
        &["me/one", "me/many"],
        vec![
            item("me/one", 1, "a"),
            item("me/many", 2, "b"),
            item("me/many", 3, "c"),
        ],
    );
    app.update(Msg::Key(Key::Tab));
    app.update(Msg::Key(Key::Down)); // me/one
    assert!(matches!(app.repo_filter, RepoFilter::One(_)));
    println!("filtered status: {:?}", app.status);
    assert!(
        app.status.contains("1 item"),
        "one item must be singular: {:?}",
        app.status
    );
    assert!(
        !app.status.contains("1 items"),
        "one item is not '1 items': {:?}",
        app.status
    );

    // And the plural still works when there are two.
    app.update(Msg::Key(Key::Down)); // me/many
    println!("filtered status (2): {:?}", app.status);
    assert!(
        app.status.contains("2 items"),
        "two items must stay plural: {:?}",
        app.status
    );
}

#[test]
fn the_all_repos_message_uses_the_same_counting() {
    // app.rs: the "— all repos — · N items" message, a third site with its own format.
    let mut app = app_with(&["me/one"], vec![item("me/one", 1, "a")]);
    app.update(Msg::Key(Key::Tab));
    app.update(Msg::Key(Key::Down)); // filter to me/one
    // A sentinel first: after the messages were unified, the "all repos" text is
    // identical to the fetch text, so without this the assertion below cannot tell
    // "the cursor refreshed the status" from "the status was left over".
    app.status = "SENTINEL".to_string();
    app.update(Msg::Key(Key::Up)); // back to "all repos", one item still
    assert!(
        matches!(app.repo_filter, RepoFilter::All),
        "row 0 means everything"
    );
    println!("all-repos status: {:?}", app.status);
    assert_ne!(
        app.status, "SENTINEL",
        "moving the cursor must refresh the status, not leave the previous one"
    );
    assert!(
        app.status.contains("1 item"),
        "one item must be singular here too: {:?}",
        app.status
    );
    assert!(
        !app.status.contains("1 items"),
        "one item is not '1 items': {:?}",
        app.status
    );
}
