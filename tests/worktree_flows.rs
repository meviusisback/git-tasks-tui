//! The three empty-state worktree flows: `W` with nowhere to point, `Enter`
//! on a repo with no issues or PRs, and the missing-clone offer.

use gtasks::actions::RepoSlug;
use gtasks::app::{App, Key, Modal, Msg, NamePurpose, Pane, Pending};

fn slug(s: &str) -> RepoSlug {
    RepoSlug::parse(s).expect("test slug is valid")
}

fn one_repo_app() -> (App, tempfile::TempDir) {
    // A fake local clone, so the flow reaches the branch prompt instead of
    // the clone question (which has its own tests below).
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().join("r");
    std::fs::create_dir_all(clone.join(".git")).unwrap();
    let mut app = App {
        clone_roots: vec![tmp.path().to_path_buf()],
        pane: Pane::Repos,
        ..App::default()
    };
    app.set_repos(vec![slug("o/r")]);
    // Cursor 0 is "all repos"; move onto the repo so the filter is One.
    app.update(Msg::Key(Key::Down));
    (app, tmp)
}

#[test]
fn w_on_an_empty_list_filtered_to_one_repo_grows_a_tree() {
    let (mut app, _guard) = one_repo_app();
    assert!(app.visible_items().is_empty());
    app.update(Msg::Key(Key::Char('W')));
    match &app.modal {
        Some(Modal::NameInput(input)) => match &input.purpose {
            NamePurpose::Create { plan, .. } => assert_eq!(plan.repo, "o/r"),
            other => panic!("expected the create flow, got {other:?}"),
        },
        other => panic!("expected the branch prompt, got {other:?}"),
    }
}

#[test]
fn w_on_an_empty_unfiltered_list_names_the_way_out() {
    let mut app = App::default();
    app.update(Msg::Key(Key::Char('W')));
    assert!(
        app.status.contains("filter to one repo"),
        "hint, not silence: {}",
        app.status
    );
}

#[test]
fn enter_on_an_empty_repo_suggests_a_fresh_branch() {
    let (mut app, _guard) = one_repo_app();
    app.pane = gtasks::app::Pane::Repos;
    app.update(Msg::Key(Key::Enter));
    // Drilled into the empty repo, with the suggestion on screen.
    assert!(matches!(app.modal, Some(Modal::Confirm(_))));
    assert!(matches!(app.pending, Some(Pending::NewBranch { .. })));
    // `y` opens the branch flow instead of an empty list.
    app.update(Msg::Key(Key::Char('y')));
    assert!(
        matches!(app.modal, Some(Modal::NameInput(_))),
        "the branch prompt opens, got {:?}",
        app.modal
    );
}

#[test]
fn missing_clone_offers_to_clone_first() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = App {
        clone_roots: vec![tmp.path().to_path_buf()],
        ..App::default()
    };
    app.open_repo_worktree_for("o/r");
    assert!(
        matches!(app.modal, Some(Modal::Confirm(_))),
        "a question, not an error: {:?}",
        app.modal
    );
    match &app.pending {
        Some(Pending::Clone { repo, path }) => {
            assert_eq!(repo, "o/r");
            assert_eq!(*path, tmp.path().join("r"));
        }
        other => panic!("expected the clone question, got {other:?}"),
    }
    // `y` hands the agreed clone to the loop; nothing runs synchronously.
    app.update(Msg::Key(Key::Char('y')));
    assert!(app.clone_go.is_some(), "the loop owns the clone now");
}

#[test]
fn missing_clone_without_roots_says_what_to_set() {
    let mut app = App::default();
    app.open_repo_worktree_for("o/r");
    assert!(
        app.modal.is_none(),
        "no question without a root to clone into"
    );
    assert!(
        app.status.contains("clone_roots"),
        "names the missing setting: {}",
        app.status
    );
}
