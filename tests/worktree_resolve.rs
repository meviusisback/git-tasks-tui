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

    for bad_slug in ["", "..", "a b", "../x", "-repo", "a/b", "r..e"] {
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
    assert_eq!(flat, PathBuf::from("/data/worktrees/git-tasks-tui/a-b-c"));

    assert!(resolve_in(base, Path::new("/"), "r", "b").is_err());
    assert!(resolve_in(Path::new(""), clone, "r", "b").is_err());
}
