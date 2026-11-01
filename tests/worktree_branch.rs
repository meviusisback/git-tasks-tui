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

#[test]
fn default_branch_names_origin_head() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main", "."]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git(
        dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    let binaries = gtasks::binaries::Binaries::detect().expect("git+gh on PATH");
    let timeout = std::time::Duration::from_secs(10);
    assert_eq!(
        gtasks::worktree::default_branch(&binaries, dir, timeout),
        Some("main".to_string()),
        "origin/HEAD names the default"
    );
}

#[test]
fn default_branch_is_none_without_a_remote_head() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main", "."]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let binaries = gtasks::binaries::Binaries::detect().expect("git+gh on PATH");
    let timeout = std::time::Duration::from_secs(10);
    assert_eq!(
        gtasks::worktree::default_branch(&binaries, dir, timeout),
        None,
        "no remote, no question"
    );
}

#[test]
fn checkout_new_branch_argv_cuts_here() {
    let binaries = gtasks::binaries::Binaries::detect().expect("git+gh on PATH");
    let branch = gtasks::worktree::BranchName::parse("fix-foo").unwrap();
    let argv = gtasks::worktree::checkout_new_branch_argv(
        &binaries,
        std::path::Path::new("/tmp/tree"),
        &branch,
    );
    let args: Vec<String> = argv
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        &args[args.len() - 3..],
        &["checkout", "-b", "fix-foo"],
        "the cut itself rides last, after git's own -c overrides: {args:?}"
    );
    assert_eq!(
        argv.cwd,
        std::path::PathBuf::from("/tmp/tree"),
        "the cut happens in the tree"
    );
}
