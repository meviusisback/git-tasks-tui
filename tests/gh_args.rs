//! S1/S3/S4/S5: argv construction, per-child environment, worktree path policy.
//!
//! Everything here asserts on the `Argv` values, never by spawning.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use gtasks::env::{Child, env_for, has_no_git_vars};
use gtasks::gh::{gh, git};
use gtasks::worktree::resolve;

fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn text(argv: &gtasks::gh::Argv) -> Vec<String> {
    argv.args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn keys(env: &[(OsString, OsString)]) -> Vec<String> {
    env.iter()
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .collect()
}

/// S1: metacharacters stay inside a single argv element; nothing is re-split.
#[test]
fn argv_never_splits_metacharacters() {
    let nasty = "owner/repo; rm -rf / #$(id)";
    let argv = gh(
        Path::new("/tmp"),
        Path::new("/usr/bin/gh"),
        args(&["repo", "view", nasty]),
        Path::new("/usr/bin"),
    );
    assert_eq!(argv.args.len(), 3);
    assert_eq!(argv.args[2], OsString::from(nasty));
}

/// S4: forbidden flags are never produced by the builders (they are not
/// reachable at all — the callers pass explicit field/value lists).
#[test]
fn argv_forbids_admin_force_auto() {
    let argv = gh(
        Path::new("/tmp"),
        Path::new("/usr/bin/gh"),
        args(&["pr", "merge", "1", "--squash"]),
        Path::new("/usr/bin"),
    );
    let flat = text(&argv);
    for forbidden in ["--admin", "--force", "--auto"] {
        assert!(
            !flat.iter().any(|a| a == forbidden),
            "{forbidden} must never appear"
        );
    }
}

/// S16: every git invocation carries the three overrides, in one place.
#[test]
fn git_always_carries_s16_overrides() {
    let argv = git(
        Path::new("/tmp"),
        Path::new("/usr/bin/git"),
        args(&["status", "--porcelain"]),
        Path::new("/usr/bin"),
    );
    let flat = text(&argv);
    assert_eq!(
        &flat[..6],
        [
            "-c",
            "core.hooksPath=/nonexistent-hooks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.pager=cat"
        ]
    );
    assert_eq!(&flat[6..], ["status", "--porcelain"]);
}

/// S3: `gh` gets its non-interactive knobs and never a git variable.
#[test]
fn gh_env_knobs_present() {
    let env = env_for(Child::Gh, Path::new("/usr/bin"));
    let keys = keys(&env);
    for knob in [
        "GH_PROMPT_DISABLED",
        "GH_NO_UPDATE_NOTIFIER",
        "GH_PAGER",
        "NO_COLOR",
    ] {
        assert!(keys.iter().any(|k| k == knob), "{knob} must be set");
    }
    assert!(has_no_git_vars(&env), "gh must not see GIT_*");
}

/// S3: `git` gets the prompt switch and nothing else from `GIT_*`.
#[test]
fn git_env_only_has_prompt_switch() {
    let env = env_for(Child::Git, Path::new("/usr/bin"));
    let git_keys: Vec<String> = keys(&env)
        .into_iter()
        .filter(|k| k.starts_with("GIT_"))
        .collect();
    assert_eq!(git_keys, ["GIT_TERMINAL_PROMPT"]);
    assert!(!keys(&env).iter().any(|k| k == "GH_TOKEN"));
}

/// S3: the agent never receives a GitHub token nor a git variable.
#[test]
fn agent_env_has_no_gh_token() {
    let env = env_for(Child::Agent, Path::new("/usr/bin"));
    let keys = keys(&env);
    assert!(!keys.iter().any(|k| k == "GH_TOKEN" || k == "GITHUB_TOKEN"));
    assert!(has_no_git_vars(&env));
}

/// S5: the resolver is pure and refuses anything that could escape the root.
#[test]
fn worktree_resolve_is_pure_and_rejects_escapes() {
    let base = Path::new("/data/worktrees");
    let clone = Path::new("/repos/git-tasks-tui");

    let path = resolve(base, clone, "git-tasks-tui", "feat/x").unwrap();
    assert_eq!(
        path,
        PathBuf::from("/data/worktrees/git-tasks-tui/feat-x"),
        "branch slashes are flattened"
    );

    for bad_slug in ["", "..", "a b", "../x", "-repo"] {
        assert!(
            resolve(base, clone, bad_slug, "main").is_err(),
            "slug {bad_slug:?} must be refused"
        );
    }
    for bad_branch in ["", "..", "../etc", "/abs", "-x", "a b", "a..b"] {
        assert!(
            resolve(base, clone, "repo", bad_branch).is_err(),
            "branch {bad_branch:?} must be refused"
        );
    }
}
