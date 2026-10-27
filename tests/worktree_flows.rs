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

fn render_screen(app: &gtasks::app::App, view: &gtasks::ui::ViewModel) -> String {
    use gtasks::ui::draw;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let backend = TestBackend::new(200, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app, view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..40 {
        for x in 0..200 {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_branch_question_is_a_box_in_the_middle() {
    use gtasks::ui::view_for;
    let (mut app, _guard) = one_repo_app();
    app.pane = gtasks::app::Pane::Repos;
    app.update(Msg::Key(Key::Enter));

    let view = view_for(&app);
    let screen = render_screen(&app, &view);
    let rows: Vec<&str> = screen.lines().collect();
    assert!(rows.len() >= 40);
    // The question is asked in the middle of the screen, not in the
    // bottom legend: the last three rows carry only the keys.
    let body = rows[..rows.len() - 3].join("\n");
    assert!(
        body.contains("start a new branch?"),
        "question must sit in the box: {screen}"
    );
    assert!(body.contains("y confirm"), "keys sit under the words");
    let tail = rows[rows.len() - 3..].join("\n");
    assert!(
        !tail.contains("start a new branch?"),
        "question must not leak into the legend: {screen}"
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
        Some(Pending::Clone { repo, path, .. }) => {
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
fn g_without_a_clone_offers_to_clone_then_continues() {
    use gtasks::app::{Modal, Pending};
    use gtasks::model::{Item, ItemKind};
    // usage-stats has no local clone here: `g` asks to clone first,
    // then continues into the agent menu instead of erroring.
    let tmp = tempfile::tempdir().unwrap();
    let mut app = App {
        clone_roots: vec![tmp.path().to_path_buf()],
        ..App::default()
    };
    app.set_repos(vec![slug("me/usage")]);
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![Item {
            number: 4,
            title: "t".to_string(),
            state: "OPEN".into(),
            is_draft: None,
            author: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            updated_at: "2026-10-20T10:00:00Z".into(),
            created_at: String::new(),
            url: "https://github.com/me/usage/issues/4".to_string(),
            head_ref_name: None,
            base_ref_name: None,
            review_decision: None,
            mergeable: None,
            status_check_rollup: Vec::new(),
            closing_issues_references: Vec::new(),
            repo: "me/usage".to_string(),
            kind: ItemKind::Issue,
        }],
        failures: Vec::new(),
    });
    app.update(Msg::Key(Key::Char('g')));
    let Some(Modal::Confirm(question)) = app.modal.as_ref() else {
        panic!("g offers the clone, got {:?}", app.modal);
    };
    assert!(
        question.contains("clone it into"),
        "not an error: {question}"
    );
    assert!(
        matches!(
            app.pending,
            Some(Pending::Clone {
                then: gtasks::app::CloneThen::AgentMenu,
                ..
            })
        ),
        "the loop continues into the agent menu: {:?}",
        app.pending
    );
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
