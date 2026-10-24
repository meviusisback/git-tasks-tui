//! S5: the worktree resolver is pure and deterministic.
//!
//! Since Task 10b the validation lives in the types (`RepoSlug`,
//! `BranchName`): the rejections below assert at the parse boundary, and
//! `resolve_in` only computes the path. `clone` is intentionally unchecked
//! here — its identity is established at creation time (Task 15).

use std::path::{Path, PathBuf};

use gtasks::actions::RepoSlug;
use gtasks::worktree::{BranchName, resolve, resolve_in};

fn slug(s: &str) -> RepoSlug {
    RepoSlug::parse(s).unwrap()
}

fn branch(b: &str) -> BranchName {
    BranchName::parse(b).unwrap()
}

/// Branch slashes are flattened; the mapping is deterministic.
#[test]
fn worktree_resolve_is_pure_and_deterministic() {
    let base = Path::new("/data/worktrees");
    let clone = Path::new("/repos/git-tasks-tui");

    let path = resolve_in(base, clone, &slug("o/git-tasks-tui"), &branch("feat/x")).unwrap();
    assert_eq!(
        path,
        PathBuf::from("/data/worktrees/o/git-tasks-tui/feat-x"),
        "branch slashes are flattened"
    );

    // Same inputs, same output; different branch, different path.
    assert_eq!(
        resolve_in(base, clone, &slug("o/r"), &branch("a")).unwrap(),
        resolve_in(base, clone, &slug("o/r"), &branch("a")).unwrap()
    );
    assert_ne!(
        resolve_in(base, clone, &slug("o/r"), &branch("a")).unwrap(),
        resolve_in(base, clone, &slug("o/r"), &branch("b")).unwrap()
    );

    // The env-based entry point agrees with the explicit core.
    let direct = resolve_in(
        &directories::ProjectDirs::from("", "", "gtasks")
            .unwrap()
            .data_dir()
            .join("worktrees"),
        clone,
        &slug("o/r"),
        &branch("a"),
    )
    .unwrap();
    assert_eq!(resolve(clone, "o/r", "a").unwrap(), direct);
}

/// Rejections live at the parse boundary now: empty, `..`, absolute, weird
/// characters — on slug parts and branch components alike.
#[test]
fn worktree_resolve_rejects_escapes() {
    for bad_slug in [
        "", "..", "a b", "../x", "-repo", "a/b/c", "r..e", "a//b", ".",
    ] {
        assert!(
            RepoSlug::parse(bad_slug).is_err(),
            "slug {bad_slug:?} must be refused"
        );
    }
    for bad_branch in ["", "..", "../etc", "/abs", "-x", "a b", "a..b"] {
        assert!(
            BranchName::parse(bad_branch).is_err(),
            "branch {bad_branch:?} must be refused"
        );
    }
    // Slashes are flattened, never preserved as subdirectories.
    let flat = resolve_in(
        Path::new("/data/worktrees"),
        Path::new("/repos/git-tasks-tui"),
        &slug("o/repo"),
        &branch("a/b/c"),
    )
    .unwrap();
    assert_eq!(flat, PathBuf::from("/data/worktrees/o/repo/a-b-c"));

    // T2-1: the path is keyed by the slug, so two repos that clone into the
    // same directory name never share a worktree.
    let base = Path::new("/data/worktrees");
    let acme = resolve_in(
        base,
        Path::new("/repos/widget"),
        &slug("acme/widget"),
        &branch("main"),
    )
    .unwrap();
    let other = resolve_in(
        base,
        Path::new("/repos/widget"),
        &slug("other-org/widget"),
        &branch("main"),
    )
    .unwrap();
    assert_ne!(acme, other, "different repos must not collide");
    assert_eq!(
        acme,
        PathBuf::from("/data/worktrees/acme/widget/main"),
        "the slug owns the directory"
    );

    // git's own branch rules (T2-4), beyond the charset.
    for bad in ["feat/.hidden", "release/a.lock", "release/v1."] {
        assert!(
            BranchName::parse(bad).is_err(),
            "branch {bad:?} must be refused"
        );
    }

    assert!(
        resolve_in(
            Path::new(""),
            Path::new("/repos/r"),
            &slug("o/r"),
            &branch("b")
        )
        .is_err()
    );
}

/// After `git worktree add`, git itself confirms which branch the tree is on.
/// A real scratch repo: `worktree add -b item-12`, then the check passes for
/// `item-12` and fails for anything else (including detached HEAD, which is
/// never the requested branch).
#[test]
fn confirm_branch_asks_git_not_the_path() {
    use gtasks::binaries::{self, Binaries};
    use gtasks::gh::{git, run_pipe};

    let git_bin = binaries::resolve("git").expect("test needs a real git");
    let binaries = Binaries::new(std::path::PathBuf::from("gh-unused-in-this-test"), git_bin);
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let target = tmp.path().join("wt");
    std::fs::create_dir_all(&repo).unwrap();

    for args in [
        vec!["init".into(), "-q".into()],
        vec![
            "-c".into(),
            "user.email=t@t".into(),
            "-c".into(),
            "user.name=t".into(),
            "commit".into(),
            "--allow-empty".into(),
            "-m".into(),
            "init".into(),
        ],
        vec![
            "worktree".into(),
            "add".into(),
            "-b".into(),
            "item-12".into(),
            target.to_str().unwrap().into(),
        ],
    ] {
        let argv = git(&binaries, &repo, args);
        let out = run_pipe(&argv, std::time::Duration::from_secs(30)).expect("git runs");
        assert!(out.status.success(), "setup failed: {out:?}");
    }

    let item12 = BranchName::parse("item-12").unwrap();
    assert!(
        gtasks::worktree::confirm_branch(&binaries, &target, &item12).is_ok(),
        "git must confirm the branch it just checked out"
    );
    let other = BranchName::parse("other").unwrap();
    assert!(
        gtasks::worktree::confirm_branch(&binaries, &target, &other).is_err(),
        "a different branch must be refused"
    );
}

/// The old flattening (`/` -> `--`) mapped two legal slugs to one directory:
/// `a--b/c` and `a/b--c` both became `a--b--c`. Keeping the owner as a real
/// component removes the ambiguity.
#[test]
fn distinct_slugs_can_never_flatten_into_one_directory() {
    let base = std::path::Path::new("/data/worktrees");
    let clone = std::path::Path::new("/src/widget");

    let one = resolve_in(base, clone, &slug("a--b/c"), &branch("main")).unwrap();
    let two = resolve_in(base, clone, &slug("a/b--c"), &branch("main")).unwrap();

    assert_ne!(one, two, "two different repos must not share a path");
    assert!(one.ends_with("a--b/c/main"), "got {one:?}");
    assert!(two.ends_with("a/b--c/main"), "got {two:?}");
}
