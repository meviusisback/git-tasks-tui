//! Task 16: the agent handoff. `g` validates, the loop hands the terminal
//! over, the column shows the record — never a live stream.

use std::path::PathBuf;

use gtasks::agent::{
    AgentChoice, AgentId, AgentNext, AgentOutcome, AgentRecord, AgentStatus, PlanInputs, SeedFile,
    agent_argv, agent_seed_body, next_step, plan_agent, run_agent_inner,
};
use gtasks::app::{App, Key, Modal, Msg, Pending};
use gtasks::fetch::FetchOutcome;
use gtasks::model::{Item, ItemKind};

/// The default agent in tests: hermes at a fake path (never spawned here,
/// only carried through the plan).
fn hermes_choice() -> AgentChoice {
    AgentChoice {
        id: AgentId::Hermes,
        program: PathBuf::from("/bin/hermes"),
    }
}

fn test_inputs<'a>(profile: &'a str, roots: &'a [PathBuf]) -> PlanInputs<'a> {
    PlanInputs {
        profile,
        roots,
        default_agent: hermes_choice(),
    }
}

fn item(repo: &str, number: u64, kind: ItemKind, head: Option<&str>) -> Item {
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
        head_ref_name: head.map(str::to_string),
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        closing_issues_references: Vec::new(),
        repo: repo.to_string(),
        kind,
    }
}

fn pr_app() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().join("r");
    std::fs::create_dir(&clone).unwrap();
    std::fs::create_dir(clone.join(".git")).unwrap();
    let root = tmp.path().to_path_buf();

    let mut app = App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("o/r").expect("valid slug"),
    ]);
    app.apply_outcome(FetchOutcome {
        items: vec![item("o/r", 7, ItemKind::Pr, Some("fix-foo"))],
        failures: Vec::new(),
    });
    app.agent_profile = "default".to_string();
    app.clone_roots = vec![root];
    let mut binaries =
        gtasks::binaries::Binaries::new(PathBuf::from("/bin/gh"), PathBuf::from("/usr/bin/git"));
    binaries.hermes = Some(PathBuf::from("/bin/hermes"));
    app.binaries = Some(binaries);
    (app, tmp)
}

#[test]
fn seed_body_carries_repo_number_and_url_and_nothing_else() {
    let body = agent_seed_body("o/r", 7, "https://github.com/o/r/pull/7", None);
    assert!(body.contains("repo: o/r#7"), "got: {body}");
    assert!(
        body.contains("url: https://github.com/o/r/pull/7"),
        "got: {body}"
    );
    assert!(
        body.contains("UNTRUSTED"),
        "the fence names the data: {body}"
    );
    assert!(
        !body.contains("user note"),
        "no note was given, none is shown"
    );
    assert_eq!(
        body.lines().count(),
        4,
        "fence + repo + url + fence: {body}"
    );
    // The builder takes no title and no body: the agent reads those itself.
}

#[test]
fn seed_body_shows_the_note_only_when_given() {
    let body = agent_seed_body(
        "o/r",
        7,
        "https://github.com/o/r/pull/7",
        Some("focus on auth"),
    );
    assert!(body.contains("user note: focus on auth"), "got: {body}");
}

#[test]
fn plan_refuses_issues_because_they_have_no_branch() {
    let err = plan_agent(
        "o/r",
        9,
        "https://github.com/o/r/issues/9",
        ItemKind::Issue,
        None,
        test_inputs("default", &[PathBuf::from("/x")]),
    )
    .expect_err("issues must be refused");
    assert_eq!(err, "agents start on pull requests — issues have no branch");
}

#[test]
fn plan_refuses_a_pr_without_a_head_branch() {
    let err = plan_agent(
        "o/r",
        7,
        "https://github.com/o/r/pull/7",
        ItemKind::Pr,
        None,
        test_inputs("default", &[PathBuf::from("/x")]),
    )
    .expect_err("a PR without a branch cannot be placed");
    assert_eq!(err, "no head branch for o/r#7 — refresh and try again");
}

#[test]
fn plan_refuses_without_clone_root() {
    let err = plan_agent(
        "o/r",
        7,
        "https://github.com/o/r/pull/7",
        ItemKind::Pr,
        Some("fix-foo"),
        test_inputs("default", &[]),
    )
    .expect_err("no clone root must be refused");
    assert_eq!(err, "no clone_root configured — agents need local clones");
}

#[test]
fn plan_refuses_a_missing_clone_with_its_place_named() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("no-such-root");
    let err = plan_agent(
        "meviusisback/git-tasks-tui",
        3,
        "https://github.com/meviusisback/git-tasks-tui/pull/3",
        ItemKind::Pr,
        Some("x"),
        test_inputs("default", std::slice::from_ref(&missing)),
    )
    .expect_err("a missing clone must be refused");
    assert_eq!(
        err,
        format!(
            "no local clone for meviusisback/git-tasks-tui under any of: {}",
            missing.display()
        )
    );
}

#[test]
fn existing_tree_launches_missing_tree_asks() {
    let tmp = tempfile::tempdir().unwrap();
    let present = tmp.path().join("present");
    std::fs::create_dir(&present).unwrap();
    std::fs::create_dir(present.join(".git")).unwrap();
    assert_eq!(
        next_step(present),
        AgentNext::Launch(tmp.path().join("present"))
    );
    assert_eq!(
        next_step(tmp.path().join("absent")),
        AgentNext::AskToCreate(tmp.path().join("absent"))
    );
}

#[test]
fn the_prompt_travels_as_a_path_never_as_argv() {
    // A body starting with `-` would become a flag if it ever touched argv.
    let argv = agent_argv(
        PathBuf::from("/bin/hermes").as_path(),
        "default",
        PathBuf::from("/tmp/seed-1").as_path(),
        PathBuf::from("/tmp/wt").as_path(),
    );
    let args: Vec<String> = argv
        .as_argv()
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.contains(&"/tmp/seed-1".to_string()),
        "the path travels: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a.contains("dash")),
        "the body never travels: {args:?}"
    );
    let pos = args
        .iter()
        .position(|a| a == "--query-file")
        .expect("the flag exists");
    assert_eq!(args[pos + 1], "/tmp/seed-1", "the value follows its flag");
    // S18: the agent never inherits the seed repo's rules (attacker-influenced
    // via PRs); the static check pins every builder, this pins the runtime argv.
    assert!(
        args.contains(&"--ignore-rules".to_string()),
        "rules off: {args:?}"
    );
    assert!(!args.iter().any(|a| a == "--yolo" || a == "--accept-hooks"));
}

#[test]
fn a_launch_that_never_starts_still_unlinks_the_seed() {
    let tmp = tempfile::tempdir().unwrap();
    let seed = SeedFile::create_in(tmp.path(), "prompt body").unwrap();
    let path = seed.path().to_path_buf();
    let argv = agent_argv(
        PathBuf::from("/nonexistent/hermes").as_path(),
        "default",
        &path,
        tmp.path(),
    );
    let (status, lines) = run_agent_inner(&argv, seed, std::time::Instant::now(), 1);
    assert_eq!(lines, 1);
    assert!(
        matches!(
            status,
            AgentStatus::Finished {
                outcome: AgentOutcome::Failed,
                ..
            }
        ),
        "a binary that cannot spawn is Failed, deterministically: {status:?}"
    );
    assert!(!path.exists(), "the seed is gone even though nothing ran");
}

#[test]
fn g_without_clone_root_names_the_missing_config() {
    let (mut app, _tmp) = pr_app();
    app.clone_roots = Vec::new();
    app.update(Msg::Key(Key::Char('g')));
    assert!(
        app.agent_requested.is_none(),
        "nothing validated, nothing asked"
    );
    assert_eq!(
        app.status,
        "no clone_root configured — agents need local clones"
    );
}

#[test]
fn g_on_an_issue_says_why_not() {
    let (mut app, _tmp) = pr_app();
    app.apply_outcome(FetchOutcome {
        items: vec![item("o/r", 9, ItemKind::Issue, None)],
        failures: Vec::new(),
    });
    app.update(Msg::Key(Key::Char('g')));
    assert!(app.agent_requested.is_none());
    assert_eq!(
        app.status,
        "agents start on pull requests — issues have no branch"
    );
}

#[test]
fn g_opens_the_agent_menu_not_the_launch() {
    let (mut app, _tmp) = pr_app();
    app.update(Msg::Key(Key::Char('g')));
    let Some(Modal::AgentMenu(menu)) = app.modal.as_ref() else {
        panic!("g must open the agent menu, got {:?}", app.modal);
    };
    assert_eq!(menu.plan.branch, "fix-foo");
    assert!(
        app.agent_requested.is_none(),
        "nothing launches before a row is confirmed"
    );
    assert!(!menu.rows.is_empty(), "hermes is configured");
    assert_eq!(menu.rows[0].id, AgentId::Hermes, "hermes first");
}

#[test]
fn y_on_an_agent_question_moves_the_agreed_plan_to_the_loop() {
    let (mut app, _tmp) = pr_app();
    let plan = plan_agent(
        "o/r",
        7,
        "https://github.com/o/r/pull/7",
        ItemKind::Pr,
        Some("fix-foo"),
        test_inputs("default", &app.clone_roots),
    )
    .expect("the fixture validates");
    let path = PathBuf::from("/tmp/wt/o/r/fix-foo");
    app.modal = Some(Modal::Confirm(
        "no worktree — create it and start the agent?".into(),
    ));
    app.pending = Some(Pending::Agent(plan, path.clone()));
    app.update(Msg::Key(Key::Char('y')));
    assert!(app.modal.is_none());
    let (moved, moved_path) = app.agent_go.as_ref().expect("the loop launches");
    assert_eq!(moved.number, 7);
    assert_eq!(moved_path, &path);
}

fn record() -> AgentRecord {
    AgentRecord {
        repo: "o/r".to_string(),
        number: 7,
        profile: "default".to_string(),
        agent: "hermes".to_string(),
        worktree: PathBuf::from("/tmp/wt/o/r/fix-foo"),
        seed_lines: 4,
        status: AgentStatus::Finished {
            outcome: AgentOutcome::Exit(0),
            secs: 42,
        },
    }
}

#[test]
fn esc_hides_the_card_but_keeps_the_record() {
    // The card lives in the right column, which both panes share: Esc closes
    // it from either pane, and the record itself stays.
    for pane in [gtasks::app::Pane::Detail, gtasks::app::Pane::List] {
        let (mut app, _tmp) = pr_app();
        app.pane = pane;
        app.agent_record = Some(record());
        app.update(Msg::Key(Key::Esc));
        if pane == gtasks::app::Pane::Detail {
            assert_eq!(
                app.pane,
                gtasks::app::Pane::Detail,
                "first Esc stays on detail"
            );
        }
        assert!(
            app.visible_record().is_none(),
            "the card hides from {pane:?}"
        );
        assert!(app.agent_record.is_some(), "the record stays");
    }
    // Reselecting the item shows the card again: leaving clears the dismissal.
    let (mut app, _tmp) = pr_app();
    app.agent_record = Some(record());
    app.agent_dismissed_for = None;
    assert!(
        app.visible_record().is_some(),
        "reselecting shows the card again"
    );
}

#[test]
fn x_forgets_the_record_and_says_so() {
    let (mut app, _tmp) = pr_app();
    app.agent_record = Some(record());
    app.update(Msg::Key(Key::Char('x')));
    assert!(app.agent_record.is_none(), "x deletes the record");
    assert_eq!(app.status, "closed the agent record for o/r#7");
    app.update(Msg::Key(Key::Char('x')));
    assert_eq!(app.status, "no agent record to close");
}

#[test]
fn another_item_shows_detail_not_the_record() {
    let (mut app, _tmp) = pr_app();
    app.agent_record = Some(record());
    app.apply_outcome(FetchOutcome {
        items: vec![item("o/r", 8, ItemKind::Pr, Some("other"))],
        failures: Vec::new(),
    });
    assert!(
        app.visible_record().is_none(),
        "a record for #7 is not shown on #8"
    );
}
