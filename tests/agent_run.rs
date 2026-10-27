//! Task 16: the agent handoff. `g` validates, the loop hands the terminal
//! over, the column shows the record — never a live stream.

use std::path::PathBuf;

use gtasks::agent::{
    AgentChoice, AgentId, AgentNext, AgentOutcome, AgentRecord, AgentStatus, PlanInputs, SeedFile,
    agent_argv, agent_seed_body, next_step, plan_agent, run_agent_inner,
};
use gtasks::app::{App, Key, Modal, Msg, Pending, Tab, TabAgent};
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
fn seed_body_without_a_pr_names_only_the_repo() {
    // A repo-level tree works on no PR: no invented #0 for the agent to fetch.
    let body = agent_seed_body("o/r", 0, "https://github.com/o/r", None);
    assert!(
        body.contains(
            "repo: o/r
"
        ),
        "got: {body}"
    );
    assert!(!body.contains('#'), "no PR number at all: {body}");
    assert!(body.contains("url: https://github.com/o/r"), "got: {body}");
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
fn plan_gives_issues_a_fresh_branch() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("r").join(".git")).unwrap();
    let roots = vec![tmp.path().to_path_buf()];
    let plan = plan_agent(
        "o/r",
        9,
        "https://github.com/o/r/issues/9",
        ItemKind::Issue,
        None,
        test_inputs("default", &roots),
    )
    .expect("issues work with a fresh branch");
    assert_eq!(plan.branch, "issue-9");
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
    // Canonicalise: macOS $TMPDIR lives under /var, which is a symlink to
    // /private/var, and the state dir refuses symlinked ancestors. Production
    // seed dirs live under the data dir, never TMPDIR — this is test-only.
    let dir = tmp.path().canonicalize().unwrap();
    let seed = SeedFile::create_in(&dir, "prompt body").unwrap();
    let path = seed.path().to_path_buf();
    let argv = agent_argv(
        PathBuf::from("/nonexistent/hermes").as_path(),
        "default",
        &path,
        &dir,
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
        "no local clone and no clone_roots configured — set one first"
    );
}

#[test]
fn g_on_an_issue_opens_the_agent_menu() {
    let (mut app, _tmp) = pr_app();
    app.apply_outcome(FetchOutcome {
        items: vec![item("o/r", 9, ItemKind::Issue, None)],
        failures: Vec::new(),
    });
    app.update(Msg::Key(Key::Char('g')));
    let Some(Modal::AgentMenu(menu)) = app.modal.as_ref() else {
        panic!("g works on issues too, got {:?}", app.modal);
    };
    assert_eq!(
        menu.plan.branch, "issue-9",
        "a fresh branch, editable in the name prompt"
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

/// A finished run filed under its tab: records live per tab now, so the
/// tests plant the tab and the entry together.
fn seed_tab_record(app: &mut App, record: AgentRecord) {
    app.tabs.push(Tab {
        repo: "o/r".to_string(),
        number: 7,
    });
    app.active_tab = Some(0);
    app.tab_agents.insert(
        ("o/r".to_string(), 7),
        TabAgent {
            record: Some(record),
            ..Default::default()
        },
    );
}

fn tab_record(app: &App) -> Option<&AgentRecord> {
    app.tab_agents
        .get(&("o/r".to_string(), 7))
        .and_then(|agent| agent.record.as_ref())
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
        seed_tab_record(&mut app, record());
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
        assert!(tab_record(&app).is_some(), "the record stays");
    }
    // Reselecting the item shows the card again: leaving clears the dismissal.
    let (mut app, _tmp) = pr_app();
    seed_tab_record(&mut app, record());
    assert!(
        app.visible_record().is_some(),
        "reselecting shows the card again"
    );
}

#[test]
fn moving_after_a_finished_run_returns_the_detail() {
    // After the close the card must not glue itself to the item: arrows,
    // Enter and tabs show the detail again. A still-running session keeps
    // its card — it is the news, not leftovers.
    let (mut app, _tmp) = pr_app();
    seed_tab_record(&mut app, record());
    assert!(app.visible_record().is_some());
    app.update(Msg::Key(Key::Down));
    assert!(
        app.visible_record().is_none(),
        "arrows dismiss a finished card"
    );
    assert!(tab_record(&app).is_some(), "the record itself stays");

    let (mut app, _tmp) = pr_app();
    seed_tab_record(&mut app, record());
    app.update(Msg::Key(Key::Enter));
    assert!(
        app.visible_record().is_none(),
        "opening the tab dismisses a finished card"
    );

    let (mut app, _tmp) = pr_app();
    let mut running = record();
    running.status = AgentStatus::Running;
    seed_tab_record(&mut app, running);
    app.update(Msg::Key(Key::Down));
    app.update(Msg::Key(Key::Enter));
    assert!(
        app.visible_record().is_some(),
        "a running session keeps its card while moving"
    );
    assert!(tab_record(&app).is_some(), "the record itself stays");
}

#[test]
fn x_forgets_the_record_and_says_so() {
    // `x` closes the tab first — the record goes with it. A second `x`
    // finds no tab and no record.
    let (mut app, _tmp) = pr_app();
    seed_tab_record(&mut app, record());
    app.update(Msg::Key(Key::Char('x')));
    assert!(app.tabs.is_empty(), "x closes the tab first");
    assert!(tab_record(&app).is_none(), "the record goes with the tab");
    app.update(Msg::Key(Key::Char('x')));
    assert_eq!(app.status, "no agent record to close");
}

#[test]
fn close_agent_record_forgets_the_card() {
    // The record path itself: called with a tab open, it forgets the card
    // and says what it closed.
    let (mut app, _tmp) = pr_app();
    seed_tab_record(&mut app, record());
    app.close_agent_record();
    assert!(tab_record(&app).is_none(), "x deletes the record");
    assert_eq!(app.status, "closed the agent record for o/r#7");
    app.close_agent_record();
    assert_eq!(app.status, "no agent record to close");
}

#[test]
fn another_item_shows_detail_not_the_record() {
    let (mut app, _tmp) = pr_app();
    seed_tab_record(&mut app, record());
    app.apply_outcome(FetchOutcome {
        items: vec![item("o/r", 8, ItemKind::Pr, Some("other"))],
        failures: Vec::new(),
    });
    assert!(
        app.visible_record().is_none(),
        "a record for #7 is not shown on #8"
    );
}
