//! S5: the worktree resolver is pure and refuses anything that could escape.

use std::path::{Path, PathBuf};

use gtasks::worktree::{resolve, resolve_in};

/// Branch slashes are flattened; the mapping is deterministic.
#[test]
fn worktree_resolve_is_pure_and_deterministic() {
    let base = Path::new("/data/worktrees");
    let clone = Path::new("/repos/git-tasks-tui");

    let path = resolve_in(base, clone, "git-tasks-tui", "feat/x").unwrap();
    assert_eq!(
        path,
        PathBuf::from("/data/worktrees/git-tasks-tui/feat-x"),
        "branch slashes are flattened"
    );

    // Same inputs, same output; different branch, different path.
    assert_eq!(
        resolve_in(base, clone, "r", "a").unwrap(),
        resolve_in(base, clone, "r", "a").unwrap()
    );
    assert_ne!(
        resolve_in(base, clone, "r", "a").unwrap(),
        resolve_in(base, clone, "r", "b").unwrap()
    );

    // The env-based entry point agrees with the explicit core.
    let direct = resolve_in(
        &directories::ProjectDirs::from("", "", "gtasks")
            .unwrap()
            .data_dir()
            .join("worktrees"),
        clone,
        "r",
        "a",
    )
    .unwrap();
    assert_eq!(resolve(clone, "r", "a").unwrap(), direct);
}

/// Rejections: empty, `..`, absolute, weird characters — on slug, branch,
/// and clone directory alike.
#[test]
fn worktree_resolve_rejects_escapes() {
    let base = Path::new("/data/worktrees");
    let clone = Path::new("/repos/git-tasks-tui");

    for bad_slug in ["", "..", "a b", "../x", "-repo", "a/b/c", "r..e", "a//b"] {
        assert!(
            resolve_in(base, clone, bad_slug, "main").is_err(),
            "slug {bad_slug:?} must be refused"
        );
    }
    for bad_branch in ["", "..", "../etc", "/abs", "-x", "a b", "a..b"] {
        assert!(
            resolve_in(base, clone, "repo", bad_branch).is_err(),
            "branch {bad_branch:?} must be refused"
        );
    }
    // Slashes are flattened, never preserved as subdirectories.
    let flat = resolve_in(base, clone, "repo", "a/b/c").unwrap();
    assert_eq!(flat, PathBuf::from("/data/worktrees/repo/a-b-c"));

    // T2-1: the path is keyed by the slug, so two repos that clone into the
    // same directory name never share a worktree.
    let acme = resolve_in(base, Path::new("/repos/widget"), "acme/widget", "main").unwrap();
    let other = resolve_in(base, Path::new("/repos/widget"), "other-org/widget", "main").unwrap();
    assert_ne!(acme, other, "different repos must not collide");
    assert_eq!(
        acme,
        PathBuf::from("/data/worktrees/acme/widget/main"),
        "the slug owns the directory"
    );

    // git's own branch rules (T2-4), beyond the charset.
    for bad in ["feat/.hidden", "release/a.lock", "release/v1."] {
        assert!(
            resolve_in(base, clone, "repo", bad).is_err(),
            "branch {bad:?} must be refused"
        );
    }

    assert!(resolve_in(base, Path::new("/"), "r", "b").is_err());
    assert!(resolve_in(Path::new(""), clone, "r", "b").is_err());
}

/// The old flattening (`/` -> `--`) mapped two legal slugs to one directory:
/// `a--b/c` and `a/b--c` both became `a--b--c`. Keeping the owner as a real
/// component removes the ambiguity.
#[test]
fn distinct_slugs_can_never_flatten_into_one_directory() {
    let base = std::path::Path::new("/data/worktrees");
    let clone = std::path::Path::new("/src/widget");

    let one = resolve_in(base, clone, "a--b/c", "main").unwrap();
    let two = resolve_in(base, clone, "a/b--c", "main").unwrap();

    assert_ne!(one, two, "two different repos must not share a path");
    assert!(one.ends_with("a--b/c/main"), "got {one:?}");
    assert!(two.ends_with("a/b--c/main"), "got {two:?}");
}
