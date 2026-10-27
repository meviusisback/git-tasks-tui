//! Reopening a tree: when the plan picks the same agent that ran there
//! before, the app offers the previous session back (`y` = resume, nothing
//! sent · `n` = fresh start). These pin the argv half (the continue flags,
//! never a prompt) and the offer/routing half (record, match, y/n, the
//! session file round-trip). Spawning itself stays in the loop.

use std::path::PathBuf;

use gtasks::agent::{AgentChoice, AgentId, AgentPlan, LastAgent, agent_argv_for_resume};
use gtasks::app::{App, Key, Modal, Msg, Pending};
use gtasks::state::State;

fn prog() -> PathBuf {
    PathBuf::from("/usr/bin/agent")
}

fn g_app(kind: gtasks::model::ItemKind, head: Option<&str>) -> (App, tempfile::TempDir) {
    use gtasks::actions::RepoSlug;
    use gtasks::fetch::FetchOutcome;
    use gtasks::model::Item;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("r").join(".git")).unwrap();
    let mut app = App {
        clone_roots: vec![tmp.path().to_path_buf()],
        worktree_root: Some(tmp.path().join("wt")),
        ..App::default()
    };
    let mut binaries =
        gtasks::binaries::Binaries::new(PathBuf::from("/bin/gh"), PathBuf::from("/bin/git"));
    binaries.hermes = Some(PathBuf::from("/bin/hermes"));
    app.binaries = Some(binaries);
    app.set_repos(vec![RepoSlug::parse("o/r").expect("valid slug")]);
    app.apply_outcome(FetchOutcome {
        items: vec![Item {
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
            head_ref_name: head.map(str::to_string),
            base_ref_name: None,
            review_decision: None,
            mergeable: None,
            status_check_rollup: Vec::new(),
            closing_issues_references: Vec::new(),
            repo: "o/r".to_string(),
            kind,
        }],
        failures: Vec::new(),
    });
    (app, tmp)
}

fn plant(root: &std::path::Path, branch: &str) -> PathBuf {
    use gtasks::worktree::BranchName;
    let slug = gtasks::actions::RepoSlug::parse("o/r").unwrap();
    let b = BranchName::parse(branch).unwrap();
    let path = gtasks::worktree::resolve_in(root, std::path::Path::new(""), &slug, &b).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    let gitdir = root.join(format!("gitdir-{branch}"));
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
    std::fs::write(path.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    path
}

#[test]
fn g_offers_the_single_previous_session_before_any_menu() {
    use gtasks::app::Modal;
    use gtasks::model::ItemKind;
    let (mut app, tmp) = g_app(ItemKind::Pr, Some("fix-foo"));
    let path = plant(&tmp.path().join("wt"), "fix-foo");
    app.remember_worktree_agent(&plan(AgentId::Hermes), &path);
    app.update(Msg::Key(Key::Char('g')));
    let Some(Modal::Confirm(question)) = app.modal.as_ref() else {
        panic!("g asks first, got {:?}", app.modal);
    };
    assert!(question.contains("hermes"), "names who ran: {question}");
    assert!(
        matches!(app.pending, Some(Pending::Resume { .. })),
        "y reopens, n chooses"
    );
    // `y` reopens with nothing sent.
    app.update(Msg::Key(Key::Char('y')));
    let (plan, place) = app.agent_resume.as_ref().expect("y reopens");
    assert_eq!(place, &path);
    assert_eq!(plan.agent.id, AgentId::Hermes);
}

#[test]
fn g_without_a_single_previous_session_opens_the_menu() {
    use gtasks::app::Modal;
    use gtasks::model::ItemKind;
    // Two recorded trees: the menu disambiguates, as before.
    let (mut app, tmp) = g_app(ItemKind::Pr, Some("fix-foo"));
    let root = tmp.path().join("wt");
    let first = plant(&root, "fix-foo");
    app.remember_worktree_agent(&plan(AgentId::Hermes), &first);
    // A second tree on another branch is not a candidate for this item,
    // so plant a twin on the same branch instead.
    let twin = root.join("o").join("r").join("fix-foo-twin");
    std::fs::create_dir_all(&twin).unwrap();
    let gitdir = root.join("gitdir-fix-foo-twin");
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/fix-foo\n").unwrap();
    std::fs::write(twin.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    app.remember_worktree_agent(&plan(AgentId::Hermes), &twin);
    app.update(Msg::Key(Key::Char('g')));
    assert!(
        matches!(app.modal, Some(Modal::AgentMenu(_))),
        "several sessions: choose first, got {:?}",
        app.modal
    );
    assert!(app.pending.is_none());

    // A recorded agent with no binary installed is skipped silently.
    let (mut app, tmp) = g_app(ItemKind::Pr, Some("fix-foo"));
    let path = plant(&tmp.path().join("wt"), "fix-foo");
    app.last_agents.insert(
        path.display().to_string(),
        LastAgent {
            agent: "retired".to_string(),
            profile: "default".to_string(),
        },
    );
    app.update(Msg::Key(Key::Char('g')));
    assert!(
        matches!(app.modal, Some(Modal::AgentMenu(_))),
        "unknown binary: choose, got {:?}",
        app.modal
    );
}

fn tree() -> PathBuf {
    PathBuf::from("/home/user/worktrees/acme-widget")
}

fn plan(agent: AgentId) -> AgentPlan {
    AgentPlan {
        repo: "o/r".to_string(),
        number: 7,
        url: "https://github.com/o/r/pull/7".to_string(),
        branch: "fix-foo".to_string(),
        profile: "coder".to_string(),
        agent: AgentChoice {
            id: agent,
            program: prog(),
        },
        clone: PathBuf::from("/tmp/clones"),
    }
}

fn args(agent: AgentId, sandbox: bool) -> Vec<String> {
    agent_argv_for_resume(agent, &prog(), "coder", &tree(), sandbox)
        .as_argv()
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn flat_contains(args: &[String], needle: &str) -> bool {
    args.iter().any(|a| a == needle)
}

#[test]
fn resume_argv_sends_nothing_for_any_agent() {
    for agent in [
        AgentId::Hermes,
        AgentId::Pi,
        AgentId::Omp,
        AgentId::OpenCode,
        AgentId::Agy,
    ] {
        let args = args(agent, false);
        assert!(
            !args
                .iter()
                .any(|a| a.contains("seed") || a.contains("query")),
            "{agent:?} resume must not name a seed or query: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.starts_with('@')),
            "{agent:?} resume must not use the @file form: {args:?}"
        );
        for flag in ["--file", "--query-file", "--prompt-interactive"] {
            assert!(
                !flat_contains(&args, flag),
                "{agent:?} resume carries no prompt flag {flag}: {args:?}"
            );
        }
    }
}

#[test]
fn hermes_resume_is_the_latest_session_in_that_tree() {
    let args = args(AgentId::Hermes, false);
    for want in [
        "-p",
        "coder",
        "chat",
        "--resume",
        "latest",
        "--in",
        "--ignore-rules",
    ] {
        assert!(
            flat_contains(&args, want),
            "hermes resume keeps {want}: {args:?}"
        );
    }
}

#[test]
fn pi_agy_and_opencode_resume_with_continue() {
    for agent in [AgentId::Pi, AgentId::Agy, AgentId::OpenCode] {
        assert_eq!(
            args(agent, false),
            vec!["--continue".to_string()],
            "{agent:?} resume is just the continue flag"
        );
    }
}

#[test]
fn omp_resume_keeps_cwd_and_continue() {
    let plain = args(AgentId::Omp, false);
    assert!(flat_contains(&plain, "--cwd"), "still starts in the tree");
    assert!(flat_contains(&plain, "--continue"), "reopens it");
    assert!(
        !flat_contains(&plain, "--approval-mode"),
        "no yolo unless sandbox is on: {plain:?}"
    );
    let sandboxed = args(AgentId::Omp, true);
    assert!(
        flat_contains(&sandboxed, "--approval-mode") && flat_contains(&sandboxed, "yolo"),
        "sandboxed omp resumes yolo, like every launch: {sandboxed:?}"
    );
}

fn recorded(agent: &str) -> App {
    let mut app = App::default();
    app.last_agents.insert(
        tree().display().to_string(),
        LastAgent {
            agent: agent.to_string(),
            profile: "coder".to_string(),
        },
    );
    app
}

#[test]
fn the_record_is_per_tree_and_follows_it() {
    let mut app = App::default();
    app.remember_worktree_agent(&plan(AgentId::Hermes), &tree());
    assert!(
        app.last_agents.contains_key(&tree().display().to_string()),
        "a launch records its agent"
    );
    let moved = PathBuf::from("/home/user/worktrees/acme-widget-2");
    app.move_worktree(&tree(), &moved);
    assert!(
        !app.last_agents.contains_key(&tree().display().to_string()),
        "the entry follows the rename"
    );
    assert!(
        app.last_agents.contains_key(&moved.display().to_string()),
        "and lands on the new path"
    );
    app.forget_worktree(&moved);
    assert!(
        !app.last_agents.contains_key(&moved.display().to_string()),
        "deleting the tree drops the entry"
    );
}

#[test]
fn y_reopens_while_n_goes_back_to_choosing() {
    use gtasks::app::Modal;
    let preset = AgentChoice {
        id: AgentId::Omp,
        program: prog(),
    };
    // The `g` flow already picked omp: `n` launches there, `y` resumes.
    let mut app = recorded("hermes");
    let question = "reopen?".to_string();
    app.modal = Some(Modal::Confirm(question));
    app.pending = Some(Pending::Resume {
        plan: plan(AgentId::Hermes),
        path: tree(),
        preset: Some(preset.clone()),
    });
    app.update(Msg::Key(Key::Char('y')));
    assert!(app.modal.is_none(), "the question closes");
    assert!(app.agent_resume.is_some(), "y reopens the recorded session");
    assert!(app.agent_place.is_none() && app.agent_go.is_none());

    let mut app = recorded("hermes");
    app.modal = Some(Modal::Confirm("reopen?".to_string()));
    app.pending = Some(Pending::Resume {
        plan: plan(AgentId::Hermes),
        path: tree(),
        preset: Some(preset),
    });
    app.update(Msg::Key(Key::Char('n')));
    assert!(app.modal.is_none(), "the question closes");
    let (chosen_plan, chosen_path) = app
        .agent_place
        .as_ref()
        .expect("n launches with the preset");
    assert_eq!(
        chosen_plan.agent.id,
        AgentId::Omp,
        "the picked agent, not the recorded one"
    );
    assert_eq!(*chosen_path, tree());
    assert!(app.agent_resume.is_none() && app.agent_go.is_none());

    // The `W` flow picked nothing yet: `n` opens the agent menu there.
    let mut app = recorded("hermes");
    app.modal = Some(Modal::Confirm("reopen?".to_string()));
    app.pending = Some(Pending::Resume {
        plan: plan(AgentId::Hermes),
        path: tree(),
        preset: None,
    });
    app.update(Msg::Key(Key::Char('n')));
    match &app.modal {
        Some(Modal::AgentMenu(menu)) => {
            assert_eq!(
                menu.place.as_ref(),
                Some(&tree()),
                "the menu already points at the tree"
            );
        }
        other => panic!("n opens the agent menu, got {other:?}"),
    }
    assert!(app.agent_place.is_none() && app.agent_resume.is_none());
}

#[test]
fn n_on_any_other_question_just_closes() {
    let mut app = recorded("omp");
    app.modal = Some(Modal::Confirm("create?".to_string()));
    app.pending = Some(Pending::Agent(plan(AgentId::Omp), tree()));
    app.update(Msg::Key(Key::Char('n')));
    assert!(app.modal.is_none());
    assert!(
        app.agent_place.is_none() && app.agent_resume.is_none() && app.agent_go.is_none(),
        "no launch from a declined creation"
    );
    assert!(
        matches!(app.pending, Some(Pending::Agent(..))),
        "the other question is untouched"
    );
}

#[test]
fn the_record_survives_a_restart_but_old_files_still_parse() {
    // Old session files predate the map: they load with no offer anywhere.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "{\"selected_repos\":[]}").unwrap();
    let old = State::load(&path).expect("old files still parse");
    assert!(old.last_agents.is_empty());

    let app = recorded("opencode");
    let mut state = State::default();
    app.capture_session(&mut state);
    let mut back = App::default();
    back.restore_session(&state);
    assert_eq!(
        back.last_agents
            .get(&tree().display().to_string())
            .map(|l| l.agent.as_str()),
        Some("opencode"),
        "the record survives a restart"
    );
}
