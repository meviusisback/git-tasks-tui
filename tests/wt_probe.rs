//! Vendored from @auditor's `gtt-probe-wt-collision.rs` (Task 10b red test).
//!
//! Adapted only where the typed API forces it: literals that used to travel
//! as `&str` now go through the same constructors the app uses
//! (`RepoSlug::parse`, `BranchName::parse`), so a constructor that wrongly
//! rejects a valid literal fails loudly here instead of silently changing
//! the case. The traversal lists are unchanged, but they now assert at the
//! type boundary — once `resolve_in` takes types, there is nothing left to
//! refuse at the mapping site, which is exactly the point of the task.

use std::path::Path;

use gtasks::actions::RepoSlug;
use gtasks::worktree::{BranchName, resolve_in};

fn base() -> &'static Path {
    Path::new("/base")
}
fn clone() -> &'static Path {
    Path::new("/clones/o/r")
}
fn slug(s: &str) -> RepoSlug {
    RepoSlug::parse(s).unwrap()
}
fn branch(b: &str) -> BranchName {
    BranchName::parse(b).unwrap()
}

#[test]
fn two_different_branches_must_not_resolve_to_the_same_directory() {
    let a = resolve_in(base(), clone(), &slug("o/r"), &branch("feat/x")).unwrap();
    let b = resolve_in(base(), clone(), &slug("o/r"), &branch("feat-x")).unwrap();
    println!("feat/x  -> {}", a.display());
    println!("feat-x  -> {}", b.display());
    assert_ne!(
        a, b,
        "two different branches collapsed into one worktree directory"
    );
    assert_eq!(a, Path::new("/base/o/r/feat-x"));
    assert_eq!(b, Path::new("/base/o/r/feat--x"));
}

#[test]
fn a_dot_slug_is_unrepresentable() {
    // "." has no owner/name shape, so it can never become a `RepoSlug`:
    // there is no string left to smuggle through the resolver.
    assert!(
        RepoSlug::parse(".").is_err(),
        "slug '.' must be unrepresentable"
    );
}

#[test]
fn the_resolver_rejects_traversal_it_should() {
    for bad in ["..", "o/..", "../x", "o/r/extra"] {
        assert!(
            RepoSlug::parse(bad).is_err(),
            "slug {bad:?} must be refused"
        );
    }
    for bad in ["..", "feat/../x", "/abs", "-x"] {
        assert!(
            BranchName::parse(bad).is_err(),
            "branch {bad:?} must be refused"
        );
    }
}
