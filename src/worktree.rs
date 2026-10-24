//! Worktree path resolution (S5) — **pure** functions: no filesystem access,
//! so tests can assert the policy without creating anything. Creation (with
//! the post-`add` branch check) is Task 15.
//!
//! Validation lives in the types, not in the resolver: [`RepoSlug`] (defined
//! in [`crate::actions`]) owns the `owner/name` grammar and [`BranchName`]
//! owns the branch grammar. `resolve_in` only computes the path, so a new
//! bypass cannot be introduced by forgetting an `ensure!` here.

use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};

use crate::actions::RepoSlug;

/// Longest accepted branch name; git itself allows more, we do not need it.
const MAX_BRANCH: usize = 128;

/// A branch name that has already been validated. The field is private on
/// purpose: the type constrains only if `parse` is the only way in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchName(String);

impl BranchName {
    /// The single door: git's own rules that a charset check alone would miss
    /// (T2-4) live here, next to the charset, instead of scattered at the
    /// call sites.
    pub fn parse(branch: &str) -> anyhow::Result<Self> {
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
        Ok(Self(branch.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Injective flattening for one directory component: `-` is escaped
    /// first, so `feat/x` and `feat-x` map to different names (`feat-x` and
    /// `feat--x`). The parse rules above guarantee the output never contains
    /// `..`: dots are never consecutive in the input, and only `-` is added.
    fn flattened(&self) -> String {
        self.0.replace('-', "--").replace('/', "-")
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a worktree for `branch` of `clone` will live: under
/// `$XDG_DATA_HOME/gtasks/worktrees` (pure: the base comes from the
/// environment, nothing is touched on disk).
pub fn resolve(clone: &Path, slug: &str, branch: &str) -> anyhow::Result<PathBuf> {
    let base = directories::ProjectDirs::from("", "", "gtasks")
        .context("cannot locate data directory")?
        .data_dir()
        .join("worktrees");
    resolve_in(
        &base,
        clone,
        &RepoSlug::parse(slug)?,
        &BranchName::parse(branch)?,
    )
}

/// Pure core with an explicit base (tests pin the base directly instead of
/// depending on the environment).
///
/// `clone` is not inspected here: it only selects the repo for Task 15's
/// `git -C`, whose identity is established at creation time (canonicalise +
/// containment). Everything that shapes the path already passed through the
/// types above.
pub fn resolve_in(
    base: &Path,
    clone: &Path,
    slug: &RepoSlug,
    branch: &BranchName,
) -> anyhow::Result<PathBuf> {
    ensure!(!base.as_os_str().is_empty(), "worktree base must be set");
    let _ = clone;
    // The path keeps the slug's structure: `owner/repo` becomes two
    // components, so two different repos can never flatten into the same
    // directory.
    let mut path = base.to_path_buf();
    match slug.as_str().split_once('/') {
        Some((owner, name)) => {
            path.push(owner);
            path.push(name);
        }
        None => path.push(slug.as_str()),
    }
    Ok(path.join(branch.flattened()))
}

/// After `git worktree add`, ask git which branch the tree is really on: a
/// path never identifies a tree, git does. Fails closed (including on
/// detached HEAD) so Task 15 aborts instead of working in the wrong tree.
pub fn confirm_branch(
    binaries: &crate::binaries::Binaries,
    worktree: &Path,
    branch: &BranchName,
) -> anyhow::Result<()> {
    let argv = crate::gh::git(
        binaries,
        worktree,
        vec!["rev-parse".into(), "--abbrev-ref".into(), "HEAD".into()],
    );
    let out = crate::gh::run_pipe(&argv, std::time::Duration::from_secs(10))
        .with_context(|| format!("cannot ask git for the branch of {}", worktree.display()))?;
    ensure!(
        out.status.success(),
        "git rev-parse failed in {}",
        worktree.display()
    );
    let actual = String::from_utf8_lossy(&out.stdout).trim().to_string();
    ensure!(
        actual == branch.as_str(),
        "worktree {} is on {actual:?}, expected branch {:?}",
        worktree.display(),
        branch.as_str()
    );
    Ok(())
}
