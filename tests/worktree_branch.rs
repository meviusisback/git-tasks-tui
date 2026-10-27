//! `current_branch`: the branch a tree is actually on. Launches refuse a
//! known mismatch instead of working on the wrong code.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} in {}: {e}", dir.display()));
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn has_branch_sees_only_existing_branches() {
    use std::time::Duration;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main", "."]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git(dir, &["checkout", "-q", "-b", "perf-cells-in-place"]);
    let binaries = gtasks::binaries::Binaries::detect().expect("git+gh on PATH");
    let timeout = Duration::from_secs(10);
    let here = gtasks::worktree::BranchName::parse("perf-cells-in-place").unwrap();
    let gone = gtasks::worktree::BranchName::parse("no-such-branch").unwrap();
    assert!(gtasks::worktree::has_branch(&binaries, dir, &here, timeout));
    assert!(!gtasks::worktree::has_branch(
        &binaries, dir, &gone, timeout
    ));
}

#[test]
fn current_branch_names_the_checked_out_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main", "."]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let binaries = gtasks::binaries::Binaries::detect().expect("git+gh on PATH");
    let timeout = Duration::from_secs(10);
    assert_eq!(
        gtasks::worktree::current_branch(&binaries, dir, timeout),
        Some("main".to_string()),
        "fresh repo names its branch"
    );
    git(dir, &["checkout", "-q", "-b", "perf-cells-in-place"]);
    assert_eq!(
        gtasks::worktree::current_branch(&binaries, dir, timeout),
        Some("perf-cells-in-place".to_string()),
        "after a checkout the new branch is named"
    );
}

#[test]
fn current_branch_is_none_outside_a_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let binaries = gtasks::binaries::Binaries::detect().expect("git+gh on PATH");
    assert_eq!(
        gtasks::worktree::current_branch(&binaries, tmp.path(), Duration::from_secs(10)),
        None,
        "a plain directory names no branch"
    );
}
