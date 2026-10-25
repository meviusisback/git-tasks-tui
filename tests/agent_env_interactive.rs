//! The **hinge** for F2, and nothing else.
//!
//! `tests/env.rs` — added by the fix itself — proves the interactive child does not inherit
//! `GH_TOKEN`/`GITHUB_TOKEN`/`GIT_*`. What it does not prove is the other direction: that the
//! allowlist still carries what the child needs to run at all. A fix that emptied the
//! environment would make that test green and break every agent launch.
//!
//! One test on purpose: the leak case already has its site, and two tests for one site is the
//! shape this project keeps removing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gtasks::agent::agent_argv;
use gtasks::gh::interactive_command;

fn fake_hermes(dir: &Path) -> (PathBuf, PathBuf) {
    let dump = dir.join("child.env");
    let path = dir.join("hermes");
    // The dump path is baked into the script: the thing being measured is the child's
    // environment, so it cannot be told where to write through a variable.
    std::fs::write(&path, format!("#!/bin/sh\nenv > {}\n", dump.display())).expect("write fake");
    let mut perms = std::fs::metadata(&path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    std::fs::set_permissions(&path, perms).expect("chmod");
    (path, dump)
}

fn dirty_parent() -> HashMap<String, String> {
    let mut parent = HashMap::new();
    parent.insert("PATH".into(), "/usr/bin:/bin".into());
    parent.insert("HOME".into(), "/home/user".into());
    parent.insert("GH_TOKEN".into(), "ghp_leak_me".into());
    parent.insert("GIT_DIR".into(), "/somewhere/else".into());
    parent
}

fn child_env(worktree: &Path) -> Vec<(String, String)> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (program, dump) = fake_hermes(tmp.path());
    let seed = tmp.path().join("seed-1-0");
    std::fs::write(&seed, "seed\n").expect("seed");

    let argv = agent_argv(&program, "coder", &seed, worktree);
    let mut cmd = interactive_command(&program, &argv, &dirty_parent());
    let status = cmd.status().expect("spawn the fake hermes");
    assert!(status.success(), "the fake must run");

    std::fs::read_to_string(&dump)
        .expect("the child must have written its environment")
        .lines()
        .filter_map(|line| {
            let (k, v) = line.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// The hinge: the child must keep the variables it needs, or a fix that emptied the environment
/// would pass the leak test and break every launch.
#[test]
fn the_interactive_child_still_has_the_base_environment() {
    let tmp = tempfile::tempdir().expect("worktree");
    let env = child_env(tmp.path());
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    println!("child env keys: {keys:?}");
    for expected in ["PATH", "HOME"] {
        assert!(
            keys.contains(&expected),
            "the allowlist must keep {expected}: the child needs it to run at all. \
             keys were {keys:?}"
        );
    }
}
