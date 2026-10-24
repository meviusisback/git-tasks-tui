//! Worktree path resolution (S5) — a **pure** function: no filesystem access,
//! so tests can assert the policy without creating anything. Creation is
//! Task 10b.

use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};

/// Longest accepted branch name; git itself allows more, we do not need it.
const MAX_BRANCH: usize = 128;
/// Longest accepted repo slug.
const MAX_SLUG: usize = 64;

/// Where a worktree for `branch` of `clone` will live: under
/// `$XDG_DATA_HOME/gtasks/worktrees` (pure: the base comes from the
/// environment, nothing is touched on disk).
pub fn resolve(clone: &Path, slug: &str, branch: &str) -> anyhow::Result<PathBuf> {
    let base = directories::ProjectDirs::from("", "", "gtasks")
        .context("cannot locate data directory")?
        .data_dir()
        .join("worktrees");
    resolve_in(&base, clone, slug, branch)
}

/// Pure core with an explicit base (tests and Task 10b pin the base
/// directly instead of depending on the environment).
///
/// Rejects anything that could escape the root or smuggle a path: empty names,
/// `..`, absolute paths, leading `-`, and characters outside the allowlist.
/// `/` in a branch becomes `-`, so `feat/x` and `feat-x` map to different
/// directories only if the rest differs — the branch is also length-checked.
pub fn resolve_in(base: &Path, clone: &Path, slug: &str, branch: &str) -> anyhow::Result<PathBuf> {
    ensure!(!base.as_os_str().is_empty(), "worktree base must be set");
    ensure!(
        !slug.is_empty() && slug.len() <= MAX_SLUG,
        "invalid repo slug length"
    );
    // Either `name` or `owner/name`: each part must be a safe component.
    let parts: Vec<&str> = slug.split('/').collect();
    ensure!(
        parts.len() <= 2,
        "repo slug must be `name` or `owner/name`: {slug:?}"
    );
    for part in &parts {
        ensure!(!part.is_empty(), "empty part in repo slug {slug:?}");
        ensure!(
            !part.starts_with('-'),
            "repo slug part must not start with `-`: {slug:?}"
        );
        ensure!(
            part.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "invalid character in repo slug: {slug:?}"
        );
    }
    ensure!(!slug.contains(".."), "repo slug must not contain `..`");

    ensure!(
        !branch.is_empty() && branch.len() <= MAX_BRANCH,
        "invalid branch length"
    );
    ensure!(
        !branch.starts_with('-'),
        "branch must not start with `-`: {branch:?}"
    );
    ensure!(
        !branch.contains(".."),
        "branch must not contain `..`: {branch:?}"
    );
    ensure!(!branch.starts_with('/'), "branch must be relative");
    ensure!(
        branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')),
        "invalid character in branch: {branch:?}"
    );
    // git's own rules that a charset check alone would miss (T2-4).
    for component in branch.split('/') {
        ensure!(
            !component.is_empty(),
            "empty component in branch {branch:?}"
        );
        ensure!(
            !component.starts_with('.'),
            "branch component must not start with `.`: {branch:?}"
        );
        ensure!(
            !component.ends_with(".lock"),
            "branch component must not end with `.lock`: {branch:?}"
        );
        ensure!(
            !component.ends_with('.'),
            "branch component must not end with `.`: {branch:?}"
        );
    }

    let repo = clone
        .file_name()
        .and_then(|name| name.to_str())
        .context("clone path has no usable file name")?;
    ensure!(
        repo.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "invalid character in clone directory name: {repo:?}"
    );
    ensure!(
        repo.len() <= 64,
        "clone directory name too long ({} bytes)",
        repo.len()
    );

    // The path is keyed by the **slug**, not by the clone directory name:
    // `acme/widget` and `other-org/widget` clone into `widget` and would
    // otherwise share a worktree (T2-1). `/` is flattened so the slug stays a
    // single component.
    let flat_slug = slug.replace('/', "--");
    let flat_branch = branch.replace('/', "-");
    Ok(base.join(flat_slug).join(flat_branch))
}
