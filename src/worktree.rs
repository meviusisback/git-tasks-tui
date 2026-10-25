//! Worktree path resolution (S5) — **pure** functions: no filesystem access,
//! so tests can assert the policy without creating anything. Creation (with
//! the post-`add` branch check) is Task 15.
//!
//! Validation lives in the types, not in the resolver: [`RepoSlug`] (defined
//! in [`crate::actions`]) owns the `owner/name` grammar and [`BranchName`]
//! owns the branch grammar. `resolve_in` only computes the path, so a new
//! bypass cannot be introduced by forgetting an `ensure!` here.

use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// One existing worktree of a repo: the real branch (read from its own
/// `.git` file, never trusted from the directory name) plus where it lives.
/// Detached trees are skipped — with no branch, the agent cannot be placed
/// on them reliably.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRow {
    pub branch: BranchName,
    pub path: PathBuf,
}

/// Every worktree directory of `owner/repo` under `root`: subdirectories
/// with a `.git` file (a linked worktree keeps a `gitdir:` pointer there, a
/// main clone a directory — the base or a stray main clone is skipped).
/// Symlinks are skipped, same as creation refuses them. No git is run: the
/// caller resolves the real branch from the `.git` file when it needs it.
pub fn list_worktrees(root: &Path, slug: &RepoSlug) -> Vec<PathBuf> {
    let mut dir = root.to_path_buf();
    match slug.as_str().split_once('/') {
        Some((owner, name)) => {
            dir.push(owner);
            dir.push(name);
        }
        None => dir.push(slug.as_str()),
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            if path.is_symlink() {
                return false;
            }
            match std::fs::symlink_metadata(path.join(".git")) {
                Ok(meta) => !meta.is_dir(),
                Err(_) => false,
            }
        })
        .collect();
    out.sort();
    out
}

/// The branch a worktree is really on, from its own `.git` file
/// (`gitdir: <real-gitdir>` → `<gitdir>/HEAD` → `ref: refs/heads/<branch>`).
/// Pure file reads: the menus need this without owning git. Detached (`HEAD`
/// is a hash) or unreadable files mean `None` — those trees are listed
/// without a branch, never placed under an agent.
pub fn worktree_branch(path: &Path) -> Option<BranchName> {
    let pointer = std::fs::read_to_string(path.join(".git")).ok()?;
    let gitdir = pointer.strip_prefix("gitdir:")?.trim();
    // The pointer is usually relative (`../.git/worktrees/<name>`): anchor
    // it at the tree, or it resolves against our own cwd.
    let gitdir = Path::new(gitdir);
    let gitdir = if gitdir.is_absolute() {
        gitdir.to_path_buf()
    } else {
        path.join(gitdir)
    };
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let name = head
        .strip_prefix("ref:")?
        .trim()
        .strip_prefix("refs/heads/")?;
    BranchName::parse(name).ok()
}

/// Rename a worktree: the branch first, the directory after. Branch-first
/// means a failed move leaves a valid tree at the old path on the new
/// branch — still usable, never half-moved. Both steps run in the loop
/// (they need git); these builders are the testable argv half.
pub fn rename_branch_argv(
    binaries: &crate::binaries::Binaries,
    clone: &Path,
    old: &BranchName,
    new: &BranchName,
) -> crate::gh::Argv {
    crate::gh::git(
        binaries,
        clone,
        vec![
            "branch".into(),
            "-m".into(),
            old.as_str().into(),
            new.as_str().into(),
        ],
    )
}

pub fn move_argv(
    binaries: &crate::binaries::Binaries,
    clone: &Path,
    old_path: &Path,
    new_path: &Path,
) -> crate::gh::Argv {
    crate::gh::git(
        binaries,
        clone,
        vec![
            "worktree".into(),
            "move".into(),
            old_path.as_os_str().into(),
            new_path.as_os_str().into(),
        ],
    )
}

/// Delete a worktree. `--force`: the dialog already asked, and a dirty tree
/// would otherwise refuse — the confirmation IS the force. The branch is
/// kept on purpose: deleting history is a separate decision.
pub fn remove_argv(
    binaries: &crate::binaries::Binaries,
    clone: &Path,
    path: &Path,
) -> crate::gh::Argv {
    crate::gh::git(
        binaries,
        clone,
        vec![
            "worktree".into(),
            "remove".into(),
            "--force".into(),
            path.as_os_str().into(),
        ],
    )
}

/// A validated rename order: both paths resolved, both branches parsed. The
/// loop executes it after the guards (no agent running inside).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRename {
    pub clone: PathBuf,
    pub old_branch: BranchName,
    pub new_branch: BranchName,
    pub old_path: PathBuf,
    pub new_path: PathBuf,
}

/// A validated delete order: the loop executes it after the guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDelete {
    pub clone: PathBuf,
    pub branch: BranchName,
    pub path: PathBuf,
}
/// The default worktree root: `$XDG_DATA_HOME/gtasks/worktrees`. The caller
/// applies the configured `worktree_root` when set. One function, so every
/// path computation starts from the same base.
pub fn default_root() -> anyhow::Result<PathBuf> {
    let base = directories::ProjectDirs::from("", "", "gtasks")
        .context("cannot locate data directory")?
        .data_dir()
        .to_path_buf();
    Ok(base.join("worktrees"))
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
/// The signature itself is pinned: stringly-typed calls must not compile.
/// ```compile_fail,E0308
/// use std::path::Path;
/// use gtasks::worktree::resolve_in;
///
/// let _ = resolve_in(Path::new("/base"), Path::new("/clones/o/r"), ".", "x");
/// ```
/// while the typed form keeps working:
/// ```rust
/// use std::path::Path;
/// use gtasks::actions::RepoSlug;
/// use gtasks::worktree::{BranchName, resolve_in};
///
/// let slug = RepoSlug::parse("o/r").unwrap();
/// let branch = BranchName::parse("x").unwrap();
/// let path = resolve_in(Path::new("/base"), Path::new("/clones/o/r"), &slug, &branch).unwrap();
/// assert_eq!(path, Path::new("/base/o/r/x"));
/// ```
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

/// A worktree that exists on disk, registered with git, on the branch we asked for.
#[derive(Debug, Clone)]
pub struct CreatedWorktree {
    pub path: PathBuf,
    pub branch: BranchName,
}

/// Delete the worktree on disk (`git worktree remove --force`): the typed
/// path, so the `--force` the untrusted transport refuses reaches the child.
/// The caller (the TUI) owns the session/record bookkeeping; this owns the
/// spawn, which is what the regression test drives.
pub fn remove_tree(
    binaries: &crate::binaries::Binaries,
    order: &WorktreeDelete,
) -> anyhow::Result<()> {
    let remove = remove_argv(binaries, &order.clone, &order.path);
    let out = run_mutation(&remove, std::time::Duration::from_secs(60))?;
    anyhow::ensure!(
        out.status.success(),
        "cannot delete the worktree: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// Run a worktree-mutating `git` argv (create/rename/move/remove): the typed
/// path, so `--force` on `worktree remove` survives the untrusted
/// transport's refusal instead of failing every delete closed. Only fixed
/// builders call this — the argv was composed by `git()` above, never by
/// user input — which is exactly what `from_checked` vouches for.
pub fn run_mutation(
    argv: &crate::gh::Argv,
    timeout: std::time::Duration,
) -> anyhow::Result<crate::gh::Output> {
    crate::gh::run_checked(&crate::gh::CheckedArgv::from_checked(argv.clone()), timeout)
}

/// Create a worktree for `branch` under `root`, or refuse without touching
/// anything outside it.
///
/// The recipe (docs/ux-redesign.md): canonicalise the **parent**, require it to be
/// inside `root`, create the last component ourselves with `create_dir` (it fails
/// on an existing symlink instead of following it), run `git worktree add` on the
/// now-existing empty directory, then re-canonicalise and undo if anything moved.
/// `--force` is never passed, and the git call carries the S16 hook hardening.
///
/// Refusals name the reason (`symlink`, `exists`, `already …`): a bare error
/// would be indistinguishable from a bug in the check itself.
pub fn create(
    root: &Path,
    clone: &Path,
    slug: &RepoSlug,
    branch: &BranchName,
    binaries: &crate::binaries::Binaries,
) -> anyhow::Result<CreatedWorktree> {
    // The target is computed with the same function the tests plant symlinks
    // with: a check somewhere the implementation never looks proves nothing.
    let target = resolve_in(root, clone, slug, branch)?;
    let canon_root = root
        .canonicalize()
        .with_context(|| format!("cannot canonicalise worktree root {}", root.display()))?;

    // Walk every ancestor from the root outward: an existing symlink anywhere
    // on the way (test 2 plants one two levels up) is a refusal, and a missing
    // level is created with `create_dir` — one component at a time, so a link
    // can never be crossed silently.
    let parent = target.parent().context("worktree target has no parent")?;
    let mut prefix = canon_root.clone();
    let rel = parent
        .strip_prefix(root)
        .map_err(|_| anyhow::anyhow!("worktree target escapes the root: {}", target.display()))?;
    for component in rel.components() {
        prefix.push(component);
        match std::fs::symlink_metadata(&prefix) {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink(),
                "refusing symlink in worktree path: {}",
                prefix.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&prefix).with_context(|| {
                    format!("cannot create worktree directory {}", prefix.display())
                })?;
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("cannot inspect worktree path {}", prefix.display()));
            }
        }
    }
    // The walk ended inside the root by construction; re-canonicalise the
    // parent now that every level exists, in case anything moved under us.
    let canon_parent = parent
        .canonicalize()
        .with_context(|| format!("cannot canonicalise {}", parent.display()))?;
    ensure!(
        canon_parent.starts_with(&canon_root),
        "worktree parent escapes the root: {}",
        canon_parent.display()
    );

    // The last component is ours to create: `create_dir` refuses an existing
    // symlink with EEXIST instead of following it (test 1), and an existing
    // directory means someone is already there (test 6: the first worktree).
    match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            anyhow::bail!("refusing symlink at worktree target: {}", target.display());
        }
        Ok(_) => {
            anyhow::bail!("worktree target already exists: {}", target.display());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e)
                .with_context(|| format!("cannot inspect worktree target {}", target.display()));
        }
    }
    std::fs::create_dir(&target)
        .with_context(|| format!("cannot create worktree directory {}", target.display()))?;

    // `git worktree add -b`: the branch is created from HEAD, so an existing
    // branch (test 4: already checked out; test 6: already used) fails here
    // instead of being silently reused (W-1). Never `--force`.
    let argv = crate::gh::git(
        binaries,
        clone,
        vec![
            "worktree".into(),
            "add".into(),
            target.clone().into_os_string(),
            "-b".into(),
            branch.as_str().into(),
        ],
    );
    let out = run_mutation(&argv, Duration::from_secs(60))
        .with_context(|| format!("cannot run git worktree add for {}", target.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // The add failed: undo our directory so neither it nor a registry
        // entry is left behind (tests 4 and 4b read the real registry).
        let _ = std::fs::remove_dir(&target);
        anyhow::bail!("git worktree add failed: {}", stderr.trim());
    }

    // The directory git just populated must still be the one we made: if the
    // re-canonicalised path left the root, undo and refuse.
    let canon_target = target
        .canonicalize()
        .with_context(|| format!("cannot canonicalise {}", target.display()))?;
    ensure!(
        canon_target.starts_with(&canon_root),
        "worktree target escapes the root: {}",
        canon_target.display()
    );

    // Git derives the branch from the directory name; ask git which branch
    // the tree is really on, so `feat/x` and `feat-x` can never share a tree.
    confirm_branch(binaries, &target, branch)?;

    Ok(CreatedWorktree {
        path: target,
        branch: branch.clone(),
    })
}
