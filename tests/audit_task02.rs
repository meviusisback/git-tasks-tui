//! Vendored auditor probe for the Task 2 integration, adapted to the current
//! API (`git(&binaries, cwd, args)`, slug paths with real components).
//!
//! Why these three together: T2-1 and T2-2 each passed on their own while the
//! product could not run a single command — the path fix and the absolute-program
//! check cancelled each other out because the builders emitted `"git"`/`"gh"`.
//! Only an assertion that the produced argv is actually spawnable closes that gap.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use gtasks::actions::RepoSlug;
use gtasks::binaries::Binaries;
use gtasks::gh::{Argv, gh, git, run_pipe};
use gtasks::worktree::{BranchName, resolve_in};

/// T2-1: the worktree path is keyed on the slug, so two repositories that clone
/// into a directory with the same name cannot share a worktree.
#[test]
fn slug_keys_the_worktree_path() {
    let base = Path::new("/data/worktrees");
    let clone = Path::new("/repos/widget");

    let a = resolve_in(
        base,
        clone,
        &RepoSlug::parse("acme/widget").unwrap(),
        &BranchName::parse("main").unwrap(),
    )
    .unwrap();
    let b = resolve_in(
        base,
        clone,
        &RepoSlug::parse("other-org/widget").unwrap(),
        &BranchName::parse("main").unwrap(),
    )
    .unwrap();

    assert_ne!(a, b, "different slugs must give different paths");
    assert!(a.ends_with("acme/widget/main"), "got {}", a.display());
    assert!(b.ends_with("other-org/widget/main"), "got {}", b.display());
}

/// T2-2: a bare program name is refused before any spawn.
#[test]
fn a_relative_program_is_refused() {
    let argv = Argv {
        program: "true".into(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: std::env::temp_dir(),
    };
    let err = run_pipe(&argv, Duration::from_secs(5)).unwrap_err();
    assert!(
        err.to_string().contains("non-absolute program"),
        "got {err}"
    );
}

/// B-1: the argv the builders produce must be spawnable. This is the assertion
/// whose absence let the merge through.
#[test]
fn builders_produce_a_runnable_argv() {
    let binaries = Binaries::detect().expect("git and gh are on PATH");

    let git_argv = git(
        &binaries,
        Path::new("/tmp"),
        vec![OsString::from("--version")],
    );
    let out = run_pipe(&git_argv, Duration::from_secs(10)).expect("git argv runs");
    assert!(out.status.success(), "git --version failed");

    let gh_argv = gh(
        &binaries,
        Path::new("/tmp"),
        vec![OsString::from("--version")],
    );
    let out = run_pipe(&gh_argv, Duration::from_secs(10)).expect("gh argv runs");
    assert!(out.status.success(), "gh --version failed");
}
