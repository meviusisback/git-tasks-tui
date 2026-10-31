//! Herdr backend: names, argv shapes, and the start/attach flow against a
//! stub `herdr` CLI. The stub answers like the real CLI (JSON on stdout,
//! exit 0) so the client logic — binding reuse, unique names, failure
//! surfacing — is pinned without a Herdr daemon.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use gtasks::herdr::{HerdrBinding, HerdrJob, agent_name, attach_argv, run_job};

fn stub_dir(script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr");
    std::fs::write(&path, script).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    dir
}

fn job(program: &Path, tree: &Path) -> HerdrJob {
    HerdrJob {
        program: program.to_path_buf(),
        cli_cwd: tree.to_path_buf(),
        name: agent_name("o/r", 7, "hermes", "coder"),
        kind: "hermes".to_string(),
        agent_args: vec!["-p".to_string(), "coder".to_string()],
        repo: "o/r".to_string(),
        tree: tree.to_path_buf(),
        label: "o/r#7 hermes".to_string(),
        binding: None,
    }
}

/// A stub `herdr` CLI: `space` is the `workspace list` answer — `none` for
/// no Space yet, `ours` for the repo's Space already there.
fn space_stub(log: &std::path::Path, space: &str) -> String {
    let spaces = match space {
        "ours" => r#"[{"label":"r","workspace_id":"w3"}]"#.to_string(),
        "legacy" => r#"[{"label":"o/r","workspace_id":"w4"}]"#.to_string(),
        _ => "[]".to_string(),
    };
    format!(
        r#"#!/bin/sh
echo "$@" >> "{log}"
if [ "$1" = "agent" ] && [ "$2" = "get" ]; then exit 1; fi
if [ "$1" = "workspace" ] && [ "$2" = "list" ]; then echo '{{"result":{{"workspaces":{spaces}}}}}'; exit 0; fi
if [ "$1" = "workspace" ] && [ "$2" = "create" ]; then echo '{{"result":{{"tab":{{"tab_id":"w9:t1"}},"root_pane":{{"pane_id":"w9:p1"}}}}}}'; exit 0; fi
if [ "$1" = "tab" ] && [ "$2" = "create" ]; then echo '{{"result":{{"tab":{{"tab_id":"w3:t9"}},"root_pane":{{"pane_id":"w3:p9"}}}}}}'; exit 0; fi
if [ "$1" = "tab" ] && [ "$2" = "rename" ]; then exit 0; fi
if [ "$1" = "agent" ] && [ "$2" = "start" ]; then echo '{{"result":{{"type":"agent_started"}}}}'; exit 0; fi
exit 1
"#,
        log = log.display()
    )
}

#[test]
fn names_fit_the_herdr_charset_and_length() {
    assert_eq!(agent_name("o/r", 7, "hermes", "coder"), "g-r-7-hermes");
    assert_eq!(
        agent_name("My.Owner/Weird.Repo_Name", 123, "opencode", "coder"),
        "g-weird-repo_name-123-opencode"
    );
    // A long repo plus a long profile still fits: the profile part drops
    // first, then the repo truncates.
    let name = agent_name(
        "o/abcdefghijklmnopqrstuvwxyz0123456789",
        1,
        "pi",
        "averylongprofile",
    );
    assert!(name.len() <= 31, "fits Herdr: {name}");
    assert!(name.starts_with("g-"), "still a name: {name}");
    assert!(
        name.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'),
        "charset: {name}"
    );
}

#[test]
fn attach_argv_is_the_attach_client() {
    let program = PathBuf::from("/usr/sbin/herdr");
    let tree = Path::new("/tmp/tree");
    let argv = attach_argv(&program, tree, "g-r-7-hermes");
    let mut flat = vec![argv.program.to_string_lossy().into_owned()];
    flat.extend(argv.args.iter().map(|a| a.to_string_lossy().into_owned()));
    assert_eq!(
        flat,
        vec!["/usr/sbin/herdr", "agent", "attach", "g-r-7-hermes"]
    );
    assert_eq!(argv.cwd, tree);
}

#[test]
fn a_fresh_tree_gets_a_space_and_a_start() {
    let tree = tempfile::tempdir().unwrap();
    let log = tree.path().join("calls.log");
    let dir = stub_dir(&space_stub(&log, "none"));
    let out = run_job(job(&dir.path().join("herdr"), tree.path()));
    let (name, tab_id, pane_id) = match out {
        gtasks::herdr::HerdrOutcome::Attached {
            name,
            tab_id,
            pane_id,
        } => (name, tab_id, pane_id),
        gtasks::herdr::HerdrOutcome::Failed(reason) => panic!("start failed: {reason}"),
    };
    assert_eq!(name, "g-r-7-hermes");
    assert_eq!(tab_id, "w9:t1", "the fresh Space's own root tab");
    assert_eq!(pane_id, "w9:p1", "no extra tab grown");
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains("workspace create --label r"),
        "the Space is created named after the repo: {calls}"
    );
    assert!(
        !calls.contains("tab create"),
        "a fresh Space needs no extra tab: {calls}"
    );
    assert!(
        calls.contains("agent start g-r-7-hermes --kind hermes --pane w9:p1 -- -p coder"),
        "kind, pane and our args ride along: {calls}"
    );
}

#[test]
fn an_existing_space_grows_a_tab() {
    let tree = tempfile::tempdir().unwrap();
    let log = tree.path().join("calls.log");
    let dir = stub_dir(&space_stub(&log, "ours"));
    let out = run_job(job(&dir.path().join("herdr"), tree.path()));
    let (tab_id, pane_id) = match out {
        gtasks::herdr::HerdrOutcome::Attached {
            tab_id, pane_id, ..
        } => (tab_id, pane_id),
        gtasks::herdr::HerdrOutcome::Failed(reason) => panic!("start failed: {reason}"),
    };
    assert_eq!(tab_id, "w3:t9");
    assert_eq!(pane_id, "w3:p9");
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        !calls.contains("workspace create"),
        "the repo's Space is reused: {calls}"
    );
    assert!(
        calls.contains("tab create --workspace w3"),
        "the tab grows inside it: {calls}"
    );
}

#[test]
fn a_live_binding_reattaches_without_starting() {
    let tree = tempfile::tempdir().unwrap();
    let log = tree.path().join("calls.log");
    let dir = stub_dir(&format!(
        r#"#!/bin/sh
echo "$@" >> "{}"
if [ "$1" = "agent" ] && [ "$2" = "get" ]; then echo '{{"result":{{"agent":{{"name":"g-r-7-hermes","pane_id":"w1:p2","cwd":"{}"}}}}}}'; exit 0; fi
exit 1
"#,
        log.display(),
        tree.path().display()
    ));
    let mut j = job(&dir.path().join("herdr"), tree.path());
    j.binding = Some(HerdrBinding {
        name: "g-r-7-hermes".to_string(),
        tab_id: "w1:t1".to_string(),
        pane_id: "w1:p2".to_string(),
    });
    let out = run_job(j);
    match out {
        gtasks::herdr::HerdrOutcome::Attached { name, tab_id, .. } => {
            assert_eq!(name, "g-r-7-hermes");
            assert_eq!(tab_id, "w1:t1", "the recorded tab is reused");
        }
        gtasks::herdr::HerdrOutcome::Failed(reason) => panic!("reattach failed: {reason}"),
    }
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(!calls.contains("agent start"), "no second agent: {calls}");
}

#[test]
fn a_dead_daemon_is_a_clean_failure_not_a_panic() {
    let tree = tempfile::tempdir().unwrap();
    let dir = stub_dir("#!/bin/sh\nexit 1\n");
    let out = run_job(job(&dir.path().join("herdr"), tree.path()));
    match out {
        gtasks::herdr::HerdrOutcome::Failed(_) => {}
        gtasks::herdr::HerdrOutcome::Attached { name, .. } => {
            panic!("a dead daemon must not attach: {name}")
        }
    }
}

#[test]
fn the_g_menu_lists_every_agent_twice_with_herdr() {
    use gtasks::app::{App, Key, Modal, Msg};
    use gtasks::fetch::FetchOutcome;
    use gtasks::model::{Item, ItemKind};

    let tmp = tempfile::tempdir().unwrap();
    let mut app = App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("o/r").expect("valid slug"),
    ]);
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
    let clone = tmp.path().join("r");
    std::fs::create_dir(&clone).unwrap();
    std::fs::create_dir(clone.join(".git")).unwrap();
    app.clone_roots = vec![tmp.path().to_path_buf()];
    let mut binaries = gtasks::binaries::Binaries::new(
        std::path::PathBuf::from("/bin/gh"),
        std::path::PathBuf::from("/usr/bin/git"),
    );
    binaries.hermes = Some(std::path::PathBuf::from("/bin/hermes"));
    binaries.herdr = Some(std::path::PathBuf::from("/usr/sbin/herdr"));
    app.binaries = Some(binaries);
    app.agent_profile = "default".to_string();
    app.update(Msg::Key(Key::Char('g')));
    let Some(Modal::AgentMenu(menu)) = app.modal.as_ref() else {
        panic!("g must open the agent menu, got {:?}", app.modal);
    };
    // PATH in CI may or may not hold the other agents: hermes always does
    // (configured above), so pin its pair and the shape of the rest.
    let labels: Vec<String> = menu
        .rows
        .iter()
        .map(|row| row.label(&menu.plan.profile))
        .collect();
    assert!(
        labels.contains(&"hermes — profile default".to_string()),
        "the direct row: {labels:?}"
    );
    assert!(
        labels.contains(&"hermes — profile default · Herdr".to_string()),
        "the Herdr twin: {labels:?}"
    );
    let herdr_rows = menu.rows.iter().filter(|row| row.herdr).count();
    let direct_rows = menu.rows.len() - herdr_rows;
    assert_eq!(
        herdr_rows, direct_rows,
        "every agent appears twice: {labels:?}"
    );
    let direct_ids: Vec<_> = menu
        .rows
        .iter()
        .filter(|row| !row.herdr)
        .map(|row| row.id)
        .collect();
    let herdr_ids: Vec<_> = menu
        .rows
        .iter()
        .filter(|row| row.herdr)
        .map(|row| row.id)
        .collect();
    assert_eq!(
        herdr_ids, direct_ids,
        "every installed agent grows exactly one twin"
    );
}

#[test]
fn no_herdr_binary_no_herdr_rows() {
    use gtasks::app::{App, Key, Modal, Msg};
    use gtasks::fetch::FetchOutcome;
    use gtasks::model::{Item, ItemKind};

    let tmp = tempfile::tempdir().unwrap();
    let mut app = App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("o/r").expect("valid slug"),
    ]);
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
    let clone = tmp.path().join("r");
    std::fs::create_dir(&clone).unwrap();
    std::fs::create_dir(clone.join(".git")).unwrap();
    app.clone_roots = vec![tmp.path().to_path_buf()];
    let mut binaries = gtasks::binaries::Binaries::new(
        std::path::PathBuf::from("/bin/gh"),
        std::path::PathBuf::from("/usr/bin/git"),
    );
    binaries.hermes = Some(std::path::PathBuf::from("/bin/hermes"));
    app.binaries = Some(binaries);
    app.agent_profile = "default".to_string();
    app.update(Msg::Key(Key::Char('g')));
    let Some(Modal::AgentMenu(menu)) = app.modal.as_ref() else {
        panic!("g must open the agent menu, got {:?}", app.modal);
    };
    assert!(
        menu.rows.iter().all(|row| !row.herdr),
        "without the CLI there is nothing to attach to"
    );
}

#[test]
fn a_legacy_full_slug_space_is_reused() {
    let tree = tempfile::tempdir().unwrap();
    let log = tree.path().join("calls.log");
    let dir = stub_dir(&space_stub(&log, "legacy"));
    let out = run_job(job(&dir.path().join("herdr"), tree.path()));
    match out {
        gtasks::herdr::HerdrOutcome::Attached { tab_id, .. } => {
            assert_eq!(tab_id, "w3:t9", "the old Space grows the tab");
        }
        gtasks::herdr::HerdrOutcome::Failed(reason) => panic!("start failed: {reason}"),
    }
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        !calls.contains("workspace create"),
        "no duplicate Space next to the legacy one: {calls}"
    );
}
