//! Task 15 — creating a worktree must be **impossible** to point outside the
//! configured root, and a failure must leave nothing registered.
//!
//! These tests are red against the stub in `src/worktree.rs`. They are the
//! definition of done for task/15, in the order the risk appears:
//!
//!   1. a symlink where the worktree should go (the last component) — the case a
//!      component-validation check passes and the filesystem does not;
//!   2. a symlink on an **ancestor** — `root/owner` pointing outside, so the walk
//!      to the target crosses the boundary before the last component;
//!   3. the mechanism itself, as a control: `create_dir` refuses a symlink with
//!      EEXIST instead of following it. If this control ever goes green-on-failure,
//!      tests 1 and 2 are passing for the wrong reason;
//!   4. a failed `git worktree add` must not leave a **registered** worktree:
//!      `git worktree list` showing a directory that does not work is worse than
//!      no entry at all, because everything downstream trusts that list;
//!   5. after success, the branch is the one the type names — git derives the
//!      branch from the directory name, so `BranchName` and `HEAD` can disagree.
//!
//! **The trap these tests have to dodge is their own**: a `create` that refuses
//! everything passes every "must be refused" test. Against the stub, tests 1 and 2
//! were green for exactly that reason. So each refusal test ends with a **positive
//! hinge** — a clean create that must succeed — and test 5 is the anchor on the
//! happy path. A refusal only means something next to an acceptance.
//!
//! Where the symlinks go is computed with `resolve_in`, the same function
//! `create` must use: a test that plants a symlink somewhere the implementation
//! never looks proves nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

use gtasks::actions::RepoSlug;
use gtasks::binaries::Binaries;
use gtasks::worktree::{self, BranchName};

struct Fixture {
    _tmp: tempfile::TempDir,
    clone: PathBuf,
    root: PathBuf,
    outside: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path();

    let clone = base.join("clone");
    std::fs::create_dir_all(&clone).expect("clone dir");
    git(&clone, &["init", "-q", "-b", "main"]);
    git(&clone, &["config", "user.email", "audit@example.invalid"]);
    git(&clone, &["config", "user.name", "audit"]);
    std::fs::write(clone.join("README.md"), "fixture\n").expect("write");
    git(&clone, &["add", "README.md"]);
    git(&clone, &["commit", "-qm", "init"]);

    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::create_dir_all(&outside).expect("outside");

    Fixture {
        _tmp: tmp,
        clone,
        root,
        outside,
    }
}

fn slug() -> RepoSlug {
    RepoSlug::parse("cli/cli").expect("slug")
}

fn branch() -> BranchName {
    BranchName::parse("item-1").expect("branch")
}

fn binaries() -> Binaries {
    Binaries::detect().expect("git must be on PATH for these tests")
}

/// What `git worktree list` says about our target, as one string.
fn registered(f: &Fixture, path: &Path) -> bool {
    let listing = git(&f.clone, &["worktree", "list", "--porcelain"]);
    listing.lines().any(|line| {
        line.strip_prefix("worktree ")
            .is_some_and(|p| p.trim() == path.display().to_string())
    })
}

fn assert_outside_is_empty(f: &Fixture) {
    let entries: Vec<_> = std::fs::read_dir(&f.outside)
        .expect("outside dir")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert!(
        entries.is_empty(),
        "something was written outside the root: {entries:?}"
    );
}

// 1 — a symlink where the worktree should go.
#[test]
fn a_symlink_at_the_target_is_refused_and_nothing_lands_outside() {
    let f = fixture();
    let target = worktree::resolve_in(&f.root, &f.clone, &slug(), &branch()).expect("target");
    std::fs::create_dir_all(target.parent().unwrap()).expect("parent");
    std::os::unix::fs::symlink(&f.outside, &target).expect("plant symlink");

    let err = worktree::create(&f.root, &f.clone, &slug(), &branch(), &binaries())
        .expect_err("a symlink at the target must be refused");
    let text = format!("{err:#}");
    assert!(
        text.contains("symlink") || text.contains("exist"),
        "the refusal must say why, not just fail: {text}"
    );

    assert_outside_is_empty(&f);
    assert!(!registered(&f, &target), "a refused target must not be registered");
}

// 2 — a symlink on an ancestor: the boundary is crossed before the last component.
#[test]
fn a_symlinked_ancestor_is_refused() {
    let f = fixture();
    let target = worktree::resolve_in(&f.root, &f.clone, &slug(), &branch()).expect("target");
    // root/cli -> outside : two components of the target now live outside.
    let ancestor = target.parent().unwrap().parent().unwrap().to_path_buf();
    std::os::unix::fs::symlink(&f.outside, &ancestor).expect("plant ancestor symlink");

    let _ = worktree::create(&f.root, &f.clone, &slug(), &branch(), &binaries())
        .expect_err("a symlinked ancestor must be refused");

    assert_outside_is_empty(&f);

    // Positive hinge: an unrelated slug must still be creatable, otherwise the
    // refusal above is just a function that says no to everything.
    let clean = RepoSlug::parse("other/other").expect("slug");
    worktree::create(&f.root, &f.clone, &clean, &branch(), &binaries())
        .expect("anti-vacuity: a clean create must succeed");
}

// 3 — the control: the mechanism we rely on really behaves this way.
#[test]
fn create_dir_does_not_follow_a_symlink() {
    let f = fixture();
    let target = f.root.join("cli").join("cli").join("item-1");
    std::fs::create_dir_all(target.parent().unwrap()).expect("parent");
    std::os::unix::fs::symlink(&f.outside, &target).expect("plant symlink");

    let err = std::fs::create_dir(&target)
        .expect_err("create_dir must refuse an existing symlink, not follow it");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::AlreadyExists,
        "the refusal must be EEXIST: {err}"
    );
    assert_outside_is_empty(&f);
}

// 4 — a failure in the middle must not leave a registered worktree behind.
#[test]
fn a_failed_add_leaves_no_registered_worktree() {
    let f = fixture();
    let target = worktree::resolve_in(&f.root, &f.clone, &slug(), &branch()).expect("target");

    // `item-1` is already checked out in the main tree: `git worktree add` fails
    // with "already checked out" after we have created the directory ourselves.
    git(&f.clone, &["checkout", "-q", "-b", "item-1"]);

    let _ = worktree::create(&f.root, &f.clone, &slug(), &branch(), &binaries())
        .expect_err("a branch already checked out cannot be added again");

    assert!(
        !registered(&f, &target),
        "a failed add must not leave a registered worktree behind: {}",
        git(&f.clone, &["worktree", "list"])
    );
    assert!(
        !target.exists(),
        "a failed add must not leave the directory either: {}",
        target.display()
    );

    // Positive hinge: another branch is free, so it must be creatable.
    let other = BranchName::parse("item-2").expect("branch");
    worktree::create(&f.root, &f.clone, &slug(), &other, &binaries())
        .expect("anti-vacuity: a clean create must succeed");
}

// 5 — the branch is the one the type names, not the one the directory suggests.
#[test]
fn a_clean_create_succeeds_on_the_branch_the_type_names() {
    let f = fixture();
    let created = worktree::create(&f.root, &f.clone, &slug(), &branch(), &binaries())
        .expect("a clean create must succeed");

    assert_eq!(created.branch.as_str(), "item-1");
    let head = git(&created.path, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(
        head.trim(),
        "item-1",
        "the directory name and HEAD must agree — a mismatch is how `feat/x` and \
         `feat-x` become the same worktree"
    );
    assert!(registered(&f, &created.path));
    assert_outside_is_empty(&f);
}
