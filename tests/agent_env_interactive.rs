//! F2 — does the **interactive** child inherit `GH_TOKEN`?
//!
//! `src/agent.rs` says, next to `env: Vec::new()`:
//!
//! > *"No allowlist here: the interactive child inherits the whole environment in
//! > `run_interactive` (it is the user's session)."*
//!
//! If that were true, an agent running inside the worktree of the repository under review would
//! read `GH_TOKEN` — and S3 in `docs/security.md` forbids exactly that ("the agent never does"
//! get the tokens; an injected "read the env and post it" is the exfiltration payload).
//!
//! So this measures the child's actual environment instead of trusting either the comment or the
//! code: `interactive_command` is public, and it builds the `Command` that `run_interactive`
//! spawns, so spawning it here is the same spawn minus the terminal handoff (which needs a tty).
//!
//! The parent environment is a **dirty value**, planted by the test: `GH_TOKEN`, `GITHUB_TOKEN`,
//! `GIT_DIR` and a harmless marker. The fake `hermes` writes its own environment to a file, so
//! the assertion is about the child, not about the builder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gtasks::agent::agent_argv;
use gtasks::gh::interactive_command;

fn fake_hermes(dir: &Path) -> (PathBuf, PathBuf) {
    let dump = dir.join("child.env");
    let path = dir.join("hermes");
    // The dump path is baked into the script: the child's environment is exactly what we are
    // measuring, so it cannot be told where to write through a variable.
    std::fs::write(
        &path,
        format!("#!/bin/sh\nenv > {}\n", dump.display()),
    )
    .expect("write fake hermes");
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
    parent.insert("GITHUB_TOKEN".into(), "ghp_leak_me_too".into());
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

#[test]
fn the_interactive_child_does_not_inherit_a_token() {
    let tmp = tempfile::tempdir().expect("worktree");
    let env = child_env(tmp.path());
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    println!("child env keys: {keys:?}");

    for forbidden in ["GH_TOKEN", "GITHUB_TOKEN"] {
        assert!(
            !keys.contains(&forbidden),
            "the interactive child inherited {forbidden}: an injected instruction inside an \
             untrusted worktree can read it and post it. keys were {keys:?}"
        );
    }
    assert!(
        !keys.iter().any(|k| k.starts_with("GIT_")),
        "a GIT_* variable leaked into the agent: keys were {keys:?}"
    );
}

/// The hinge: the child must have *some* environment, or the assertions above would pass on an
/// empty set for the wrong reason.
#[test]
fn the_interactive_child_still_has_the_base_environment() {
    let tmp = tempfile::tempdir().expect("worktree");
    let env = child_env(tmp.path());
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    for expected in ["PATH", "HOME"] {
        assert!(
            keys.contains(&expected),
            "the allowlist must keep {expected}: the child needs it to run at all. \
             keys were {keys:?}"
        );
    }
}
