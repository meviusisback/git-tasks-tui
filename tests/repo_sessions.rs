//! `g` in the repos pane: the repo's sessions menu — every agent and past
//! run on its worktrees, open or closed, with `Enter` reopening.

use std::path::PathBuf;

use gtasks::app::{App, Key, Modal, Msg, Pane};
use gtasks::model::ItemKind;

fn repo_app() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().join("r");
    std::fs::create_dir(&clone).unwrap();
    std::fs::create_dir(clone.join(".git")).unwrap();

    let mut app = App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("o/r").expect("valid slug"),
    ]);
    app.agent_profile = "default".to_string();
    app.clone_roots = vec![tmp.path().to_path_buf()];
    app.worktree_root = Some(tmp.path().join("wt"));
    let mut binaries =
        gtasks::binaries::Binaries::new(PathBuf::from("/bin/gh"), PathBuf::from("/usr/bin/git"));
    binaries.hermes = Some(PathBuf::from("/bin/hermes"));
    app.binaries = Some(binaries);
    app.pane = Pane::Repos;
    app.repos_cursor = 1; // the o/r row, not All
    (app, tmp)
}

fn press(app: &mut App, key: Key) {
    app.update(Msg::Key(key));
}

fn plant(root: &std::path::Path, branch: &str) -> PathBuf {
    use gtasks::worktree::BranchName;
    let slug = gtasks::actions::RepoSlug::parse("o/r").unwrap();
    let branch = BranchName::parse(branch).unwrap();
    let path =
        gtasks::worktree::resolve_in(root, std::path::Path::new(""), &slug, &branch).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    let gitdir = root.join("gitdir-").join(branch.as_str());
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
    std::fs::write(path.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    path
}

fn plan(agent: &str) -> gtasks::agent::AgentPlan {
    use gtasks::agent::{AgentChoice, AgentId};
    gtasks::agent::AgentPlan {
        repo: "o/r".to_string(),
        number: 0,
        url: "https://github.com/o/r".to_string(),
        branch: String::new(),
        profile: "default".to_string(),
        agent: AgentChoice {
            id: if agent == "hermes" {
                AgentId::Hermes
            } else {
                AgentId::Omp
            },
            program: PathBuf::from("/bin/hermes"),
        },
        clone: PathBuf::from("/tmp/r"),
    }
}

#[test]
fn g_on_a_repo_lists_its_sessions() {
    let (mut app, tmp) = repo_app();
    plant(&tmp.path().join("wt"), "fix-foo");
    plant(&tmp.path().join("wt"), "issue-9");
    press(&mut app, Key::Char('g'));
    let Some(Modal::Worktrees(menu)) = app.modal.as_ref() else {
        panic!("g on a repo opens its sessions, got {:?}", app.modal);
    };
    assert_eq!(menu.plan.repo, "o/r");
    assert_eq!(menu.rows.len(), 2, "both trees, open or closed");
}

#[test]
fn g_on_all_is_not_a_session() {
    let (mut app, _tmp) = repo_app();
    app.repos_cursor = 0; // All
    press(&mut app, Key::Char('g'));
    assert!(
        !matches!(app.modal, Some(Modal::Worktrees(_))),
        "All is not a place"
    );
    assert!(
        app.status.contains("pick one repo"),
        "says so: {}",
        app.status
    );
}

#[test]
fn g_without_a_clone_offers_to_clone_first() {
    let (mut app, tmp) = repo_app();
    plant(&tmp.path().join("wt"), "fix-foo");
    std::fs::remove_dir_all(tmp.path().join("r")).unwrap();
    press(&mut app, Key::Char('g'));
    assert!(
        matches!(app.modal, Some(Modal::Confirm(_))),
        "offers the clone, got {:?}",
        app.modal
    );
}

#[test]
fn no_trees_names_a_fresh_one_like_w() {
    let (mut app, _tmp) = repo_app();
    press(&mut app, Key::Char('g'));
    let Some(Modal::NameInput(input)) = app.modal.as_ref() else {
        panic!("nothing to reopen: name a tree, got {:?}", app.modal);
    };
    assert_eq!(input.value, "r-dev", "the same suggestion W grows");
}

#[test]
fn enter_reopens_the_recorded_session_with_nothing_sent() {
    use gtasks::app::Pending;
    let (mut app, tmp) = repo_app();
    let path = plant(&tmp.path().join("wt"), "fix-foo");
    app.remember_worktree_agent(&plan("hermes"), &path);
    press(&mut app, Key::Char('g'));
    press(&mut app, Key::Enter);
    let Some(Modal::Confirm(question)) = app.modal.as_ref() else {
        panic!("recorded: offers the resume, got {:?}", app.modal);
    };
    assert!(question.contains("hermes"), "names who ran: {question}");
    assert!(
        matches!(app.pending, Some(Pending::Resume { .. })),
        "y reopens, n chooses"
    );
}

#[test]
fn the_menu_names_agent_live_and_issue() {
    use gtasks::ui::{draw, view_for};
    use ratatui::{Terminal, backend::TestBackend};
    let (mut app, tmp) = repo_app();
    let live = plant(&tmp.path().join("wt"), "fix-foo");
    let dead = plant(&tmp.path().join("wt"), "issue-9");
    // The open issue behind the live branch.
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![gtasks::model::Item {
            number: 7,
            title: "t".to_string(),
            state: "OPEN".into(),
            is_draft: None,
            author: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            updated_at: "2026-10-20T10:00:00Z".into(),
            created_at: String::new(),
            url: "https://github.com/o/r/pull/7".to_string(),
            head_ref_name: Some("fix-foo".to_string()),
            base_ref_name: None,
            review_decision: None,
            mergeable: None,
            status_check_rollup: Vec::new(),
            closing_issues_references: Vec::new(),
            repo: "o/r".to_string(),
            kind: ItemKind::Pr,
        }],
        failures: Vec::new(),
    });
    app.remember_worktree_agent(&plan("hermes"), &live);
    app.remember_worktree_agent(&plan("omp"), &dead);
    // A running session in the live tree.
    app.tab_agents.insert(
        ("o/r".to_string(), 7),
        gtasks::app::TabAgent {
            running: true,
            record: Some(gtasks::agent::AgentRecord {
                repo: "o/r".to_string(),
                number: 7,
                profile: "default".to_string(),
                agent: "hermes".to_string(),
                worktree: live.clone(),
                seed_lines: 0,
                status: gtasks::agent::AgentStatus::Running,
            }),
            ..Default::default()
        },
    );
    press(&mut app, Key::Char('g'));
    let view = view_for(&app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut screen = String::new();
    for y in 0..30 {
        for x in 0..200 {
            screen.push_str(buffer[(x, y)].symbol());
        }
        screen.push('\n');
    }
    assert!(screen.contains("hermes · running"), "live named: {screen}");
    assert!(screen.contains("omp"), "past run named: {screen}");
    assert!(screen.contains("#7"), "open issue linked: {screen}");
    assert!(screen.contains("#9"), "closed issue linked: {screen}");
    assert!(
        !screen.contains("this PR"),
        "a repo menu owns no PR: {screen}"
    );
}

#[test]
fn tree_live_follows_running_records_only() {
    use gtasks::agent::{AgentRecord, AgentStatus};
    let (mut app, tmp) = repo_app();
    let path = plant(&tmp.path().join("wt"), "fix-foo");
    assert!(!app.tree_live(&path), "nothing runs yet");
    let mut agent = gtasks::app::TabAgent::default();
    agent.running = true;
    agent.record = Some(AgentRecord {
        repo: "o/r".to_string(),
        number: 7,
        profile: "default".to_string(),
        agent: "hermes".to_string(),
        worktree: path.clone(),
        seed_lines: 0,
        status: AgentStatus::Running,
    });
    app.tab_agents.insert(("o/r".to_string(), 7), agent);
    assert!(app.tree_live(&path), "the live tree is marked");
}

#[test]
fn issue_refs_come_from_open_retained_and_branch_names() {
    use gtasks::fetch::FetchOutcome;
    use gtasks::model::Item;
    let (mut app, _tmp) = repo_app();
    let item = |number: u64, head: Option<&str>| Item {
        number,
        title: "t".to_string(),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        updated_at: "2026-10-20T10:00:00Z".into(),
        created_at: String::new(),
        url: format!("https://github.com/o/r/pull/{number}"),
        head_ref_name: head.map(str::to_string),
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        closing_issues_references: Vec::new(),
        repo: "o/r".to_string(),
        kind: ItemKind::Pr,
    };
    app.apply_outcome(FetchOutcome {
        items: vec![item(7, Some("fix-foo"))],
        failures: Vec::new(),
    });
    assert_eq!(app.issue_ref_for_branch("fix-foo"), Some(7));
    assert_eq!(app.issue_ref_for_branch("issue-9"), Some(9));
    assert_eq!(app.issue_ref_for_branch("unknown"), None);
}
