//! `sandbox_agents`: the `<agent>-sandboxed` wrappers with non-interactive
//! approvals. Off by default; on means containment plus YOLO where the
//! agent has a flag for it.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use gtasks::agent::{AgentId, agent_argv_for, sandboxed_program, sandboxed_program_in};
use gtasks::app::{App, Key, Modal, Msg};
use gtasks::config::Config;

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
            kind: gtasks::model::ItemKind::Pr,
        }],
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

fn render(app: &App) -> String {
    use gtasks::ui::{draw, view_for};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let view = view_for(app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..30 {
        for x in 0..200 {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_menu_names_the_mode_before_anything_is_confirmed() {
    let (mut app, _tmp) = pr_app();
    app.update(Msg::Key(Key::Char('g')));
    assert!(matches!(app.modal, Some(Modal::AgentMenu(_))));
    let plain = render(&app);
    assert!(!plain.contains("sandboxed"), "off means no suffix: {plain}");

    let (mut app, _tmp) = pr_app();
    app.sandbox_agents = true;
    app.update(Msg::Key(Key::Char('g')));
    let screen = render(&app);
    assert!(
        screen.contains("· sandboxed"),
        "every row says the mode: {screen}"
    );
}

fn args(id: AgentId, program: &str, sandbox: bool) -> Vec<String> {
    let argv = agent_argv_for(
        id,
        PathBuf::from(program).as_path(),
        "default",
        PathBuf::from("/tmp/seed-1").as_path(),
        PathBuf::from("/tmp/wt").as_path(),
        sandbox,
    );
    argv.as_argv()
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn off_by_default_and_settable_from_toml() {
    assert!(!Config::default().sandbox_agents);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "repos = [\"o/r\"]\nsandbox_agents = true\n").unwrap();
    assert!(Config::load(&path).unwrap().sandbox_agents);
}

#[test]
fn the_wrapper_is_the_sibling_that_exists() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("pi");
    std::fs::write(&plain, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
    // No sibling, empty search dirs: no launch target anywhere.
    assert_eq!(
        sandboxed_program_in(&plain, &[]),
        None,
        "no wrapper, no launch target"
    );
    let wrapped = dir.path().join("pi-sandboxed");
    std::fs::write(&wrapped, "#!/bin/sh\n").unwrap();
    assert_eq!(
        sandboxed_program_in(&plain, &[]),
        Some(wrapped.clone()),
        "the sibling wins without any PATH"
    );
    // Sibling missing, wrapper on the search path: the PATH leg finds it.
    std::fs::remove_file(&wrapped).unwrap();
    assert_eq!(
        sandboxed_program_in(&plain, std::slice::from_ref(&dir.path().to_path_buf())),
        None,
        "wrapper gone everywhere"
    );
    std::fs::write(&wrapped, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&wrapped, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        sandboxed_program_in(&plain, std::slice::from_ref(&dir.path().to_path_buf())),
        Some(wrapped),
        "PATH leg covers a binary reached without its sibling"
    );
    // The live PATH still works end to end (sibling next to the binary).
    assert_eq!(
        sandboxed_program(&plain),
        Some(dir.path().join("pi-sandboxed"))
    );
}

#[test]
fn off_means_the_plain_shapes() {
    let omp = args(AgentId::Omp, "/bin/omp", false);
    assert_eq!(omp, vec!["--cwd", "/tmp/wt", "@/tmp/seed-1"]);
}

#[test]
fn only_omp_gains_a_flag_and_it_is_yolo() {
    let omp = args(AgentId::Omp, "/bin/omp-sandboxed", true);
    assert_eq!(
        omp,
        vec![
            "--approval-mode",
            "yolo",
            "--cwd",
            "/tmp/wt",
            "@/tmp/seed-1"
        ]
    );
    // hermes keeps Smart (its default), opencode/agy carry theirs in the
    // wrapper, pi has no flag: same argv, only the program changes.
    let hermes = args(AgentId::Hermes, "/bin/hermes-sandboxed", true);
    assert!(hermes.contains(&"-p".to_string()));
    assert!(!hermes.iter().any(|a| a == "yolo" || a == "--yolo"));
    let pi = args(AgentId::Pi, "/bin/pi-sandboxed", true);
    assert_eq!(pi, vec!["@/tmp/seed-1"]);
    let opencode = args(AgentId::OpenCode, "/bin/opencode-sandboxed", true);
    assert!(!opencode.iter().any(|a| a == "--auto" || a == "yolo"));
    let agy = args(AgentId::Agy, "/bin/agy-sandboxed", true);
    assert!(!agy.iter().any(|a| a.contains("permission") || a == "yolo"));
}
