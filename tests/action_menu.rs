//! Task 14: the `m` menu turns method and branch deletion into a choice.
//! Rows are typed picks, so the menu cannot promise what the action cannot
//! build; Enter goes through the same confirmation as `a`.

use gtasks::actions::RepoSlug;
use gtasks::app::{ActionPick, App, Key, Modal, Msg, Pane, Pending};
use gtasks::fetch::FetchOutcome;
use gtasks::model::{Item, ItemKind};

fn item(repo: &str, number: u64, kind: ItemKind) -> Item {
    Item {
        number,
        title: "t".to_string(),
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
        kind,
    }
}

fn app_with(item: Item) -> App {
    let mut app = App::default();
    app.set_repos(vec![RepoSlug::parse(&item.repo).expect("valid slug")]);
    app.apply_outcome(FetchOutcome {
        items: vec![item],
        failures: Vec::new(),
    });
    app.binaries = Some(gtasks::binaries::Binaries::new(
        std::path::PathBuf::from("/bin/true"),
        std::path::PathBuf::from("/bin/true"),
    ));
    app
}

fn pr_app() -> App {
    app_with(item("o/r", 7, ItemKind::Pr))
}

#[test]
fn y_on_the_cleanup_question_hands_trees_and_branch_to_the_loop() {
    use std::path::PathBuf;
    let mut app = pr_app();
    let order = gtasks::worktree::CleanupOrder {
        repo: "o/r".to_string(),
        number: 7,
        branch: gtasks::worktree::BranchName::parse("fix-foo").unwrap(),
        clone: PathBuf::from("/c"),
        trees: vec![PathBuf::from("/t")],
    };
    app.modal = Some(Modal::Confirm("merged #7 — delete tree /t?".into()));
    app.pending = Some(Pending::Cleanup(order));
    app.update(Msg::Key(Key::Char('y')));
    let moved = app.cleanup_go.as_ref().expect("the loop cleans up");
    assert_eq!(moved.branch.as_str(), "fix-foo");
    assert_eq!(moved.trees, vec![PathBuf::from("/t")]);

    let mut app = pr_app();
    let order = gtasks::worktree::CleanupOrder {
        repo: "o/r".to_string(),
        number: 7,
        branch: gtasks::worktree::BranchName::parse("fix-foo").unwrap(),
        clone: PathBuf::from("/c"),
        trees: Vec::new(),
    };
    app.modal = Some(Modal::Confirm("merged #7 — delete local branch?".into()));
    app.pending = Some(Pending::Cleanup(order));
    app.update(Msg::Key(Key::Char('n')));
    assert!(app.modal.is_none(), "the question closes");
    assert!(app.cleanup_go.is_none(), "declining keeps everything");
}

#[test]
fn m_outside_the_list_refuses_instead_of_opening() {
    let mut app = pr_app();
    app.pane = Pane::Detail;
    app.update(Msg::Key(Key::Char('m')));
    assert!(app.modal.is_none(), "the menu belongs to the list box");
    assert_eq!(app.status, "move to the list first");
}

#[test]
fn m_opens_six_typed_rows_on_a_pr() {
    let mut app = pr_app();
    app.update(Msg::Key(Key::Char('m')));
    let Some(Modal::ActionMenu(menu)) = app.modal.as_ref() else {
        panic!("expected the actions menu");
    };
    assert_eq!(menu.rows.len(), 6, "3 methods × keep/delete the branch");
    assert_eq!(
        menu.rows[0],
        ActionPick::Merge(gtasks::actions::MergeMethod::Squash, false)
    );
    assert!(menu.rows.contains(&ActionPick::Merge(
        gtasks::actions::MergeMethod::Rebase,
        true
    )));
    // No strings cross the boundary: every row is already typed.
    assert!(menu.rows.iter().all(|r| !r.text().is_empty()));
}

#[test]
fn m_on_an_issue_offers_only_closing() {
    let mut app = app_with(item("o/r", 9, ItemKind::Issue));
    app.update(Msg::Key(Key::Char('m')));
    let Some(Modal::ActionMenu(menu)) = app.modal.as_ref() else {
        panic!("expected the actions menu");
    };
    assert_eq!(menu.rows, vec![ActionPick::Close]);
}

#[test]
fn m_with_no_items_says_so() {
    let mut app = App::default();
    app.update(Msg::Key(Key::Char('m')));
    assert!(app.modal.is_none(), "no menu without an item");
    assert_eq!(app.status, "nothing selected");
}

#[test]
fn esc_closes_the_menu_without_acting() {
    let mut app = pr_app();
    app.update(Msg::Key(Key::Char('m')));
    assert!(app.modal.is_some());
    app.update(Msg::Key(Key::Esc));
    assert!(app.modal.is_none(), "Esc cancels");
    assert!(app.pending.is_none(), "nothing was confirmed");
}

#[test]
fn enter_confirms_the_picked_row_through_the_same_dialog_as_a() {
    let mut app = pr_app();
    app.update(Msg::Key(Key::Char('m')));
    app.update(Msg::Key(Key::Down));
    app.update(Msg::Key(Key::Down)); // row 2: merge — rebase
    app.update(Msg::Key(Key::Enter));
    let gtasks::app::Pending::Action(pending) = app.pending.as_ref().expect("a pick was confirmed")
    else {
        panic!("the menu confirms actions, not agent launches");
    };
    assert_eq!(
        pending.action,
        gtasks::actions::Action::Merge {
            repo: RepoSlug::parse("o/r").expect("valid slug"),
            number: 7,
            method: gtasks::actions::MergeMethod::Rebase,
            delete_branch: false,
        }
    );
    assert!(
        matches!(app.modal, Some(Modal::Confirm(_))),
        "the shared confirmation dialog opens"
    );
    assert!(
        app.status.contains("--rebase"),
        "the shown argv is the picked one: {}",
        app.status
    );
    assert!(
        !app.status.contains("--delete-branch"),
        "not picked, not shown: {}",
        app.status
    );
}

#[test]
fn picking_delete_branch_shows_the_flag_before_anything_runs() {
    let mut app = pr_app();
    app.update(Msg::Key(Key::Char('m')));
    for _ in 0..5 {
        app.update(Msg::Key(Key::Down));
    } // row 5: merge — rebase + delete branch
    app.update(Msg::Key(Key::Enter));
    let gtasks::app::Pending::Action(pending) = app.pending.as_ref().expect("a pick was confirmed")
    else {
        panic!("the menu confirms actions, not agent launches");
    };
    assert_eq!(
        pending.action,
        gtasks::actions::Action::Merge {
            repo: RepoSlug::parse("o/r").expect("valid slug"),
            number: 7,
            method: gtasks::actions::MergeMethod::Rebase,
            delete_branch: true,
        }
    );
    assert!(
        app.status.contains("--delete-branch"),
        "the flag is shown before it runs: {}",
        app.status
    );
}

#[test]
fn a_still_fast_merges_with_defaults() {
    // `a` is the one-keystroke path with defaults; `m` is the choice.
    // Same constructor underneath, different jobs on top.
    let mut app = pr_app();
    app.update(Msg::Key(Key::Char('a')));
    let gtasks::app::Pending::Action(pending) = app.pending.as_ref().expect("fast merge confirmed")
    else {
        panic!("a confirms an action, not an agent launch");
    };
    assert_eq!(
        pending.action,
        gtasks::actions::Action::Merge {
            repo: RepoSlug::parse("o/r").expect("valid slug"),
            number: 7,
            method: gtasks::actions::MergeMethod::Squash,
            delete_branch: false,
        }
    );
}
