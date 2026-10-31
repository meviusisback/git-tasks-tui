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
        tree: tree.to_path_buf(),
        label: "o/r#7 hermes".to_string(),
        binding: None,
    }
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
fn a_fresh_tree_gets_a_tab_and_a_start() {
    let tree = tempfile::tempdir().unwrap();
    let log = tree.path().join("calls.log");
    let dir = stub_dir(&format!(
        r#"#!/bin/sh
echo "$@" >> "{}"
if [ "$1" = "agent" ] && [ "$2" = "get" ]; then exit 1; fi
if [ "$1" = "tab" ] && [ "$2" = "create" ]; then echo '{{"result":{{"tab":{{"tab_id":"w1:t9"}},"root_pane":{{"pane_id":"w1:p9"}}}}}}'; exit 0; fi
if [ "$1" = "agent" ] && [ "$2" = "start" ]; then echo '{{"result":{{"type":"agent_started"}}}}'; exit 0; fi
exit 1
"#,
        log.display()
    ));
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
    assert_eq!(tab_id, "w1:t9");
    assert_eq!(pane_id, "w1:p9");
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains("agent start g-r-7-hermes --kind hermes --pane w1:p9 -- -p coder"),
        "kind, pane and our args ride along: {calls}"
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
