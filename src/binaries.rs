//! S2 — absolute, verified binaries.
//!
//! Callers resolve a bare tool name once (`resolve`) and hand the absolute
//! path to `Argv`; nothing is ever looked up at spawn time. Every candidate
//! is canonicalised (symlinks resolved) and must be a regular executable
//! file owned by root or by our own uid, with no group/world write bits.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};

/// The external programs, resolved **once at startup** (S2). Building an argv
/// through `gh()`/`git()` cannot perform a PATH lookup: the resolved paths are
/// already here, so no factory can produce an unverified program.
#[derive(Debug, Clone)]
pub struct Binaries {
    pub gh: PathBuf,
    pub git: PathBuf,
    /// Optional until the agent integration lands.
    pub hermes: Option<PathBuf>,
    /// Browser launcher; absent on a headless machine, which is why it is optional.
    /// `xdg-open` on Linux, `open` on macOS.
    pub opener: Option<PathBuf>,
}

impl Binaries {
    /// Resolve from `PATH` once. `gh` and `git` are required.
    pub fn detect() -> anyhow::Result<Self> {
        Ok(Self {
            gh: resolve("gh")?,
            git: resolve("git")?,
            hermes: resolve("hermes").ok(),
            #[cfg(target_os = "macos")]
            opener: resolve("open").ok(),
            #[cfg(not(target_os = "macos"))]
            opener: resolve("xdg-open").ok(),
        })
    }

    /// Explicit paths, for tests: no `PATH` involved, nothing to overwrite.
    pub fn new(gh: PathBuf, git: PathBuf) -> Self {
        Self {
            gh,
            git,
            hermes: None,
            opener: None,
        }
    }
}

/// Resolve a bare tool name against `PATH` (production entry point).
pub fn resolve(name: &str) -> anyhow::Result<PathBuf> {
    let owned: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let dirs: Vec<&Path> = owned.iter().map(PathBuf::as_path).collect();
    resolve_in(name, &dirs)
}

/// Resolve a bare tool name against explicit `dirs` (testable core: no
/// environment access, so tests never mutate `PATH`).
pub fn resolve_in(name: &str, dirs: &[&Path]) -> anyhow::Result<PathBuf> {
    ensure!(!name.is_empty(), "binary name must not be empty");
    ensure!(
        !name.contains('/'),
        "binary name must be bare, got {name:?}"
    );
    for dir in dirs {
        if let Ok(path) = check(&dir.join(name)) {
            return Ok(path);
        }
    }
    anyhow::bail!("no verified binary {name:?} in {} search dirs", dirs.len())
}

/// Verify an already-absolute path with the same checks (used by `run_pipe`
/// for test-supplied programs, so every spawned binary is verified).
pub fn verify(path: &Path) -> anyhow::Result<PathBuf> {
    ensure!(
        path.is_absolute(),
        "binary path must be absolute: {}",
        path.display()
    );
    check(path)
}

/// A program that exists but cannot be used: wrong type, no execute bit, unsafe
/// permissions. It is an **environment** failure, not a policy refusal — the
/// caller must not go looking for a rule that refused nothing (E-1).
#[derive(Debug)]
pub struct ProgramNotUsable {
    pub path: String,
    pub why: &'static str,
}

impl std::fmt::Display for ProgramNotUsable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.why, self.path)
    }
}

impl std::error::Error for ProgramNotUsable {}

/// Canonicalise and verify a single candidate path.
fn check(path: &Path) -> anyhow::Result<PathBuf> {
    let abs = std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve {}", path.display()))?;
    let meta = std::fs::metadata(&abs).with_context(|| format!("cannot stat {}", abs.display()))?;
    anyhow::ensure!(
        meta.is_file(),
        ProgramNotUsable {
            path: abs.display().to_string(),
            why: "not a regular file",
        }
    );
    let mode = meta.permissions().mode();
    anyhow::ensure!(
        mode & 0o111 != 0,
        ProgramNotUsable {
            path: abs.display().to_string(),
            why: "not executable",
        }
    );
    anyhow::ensure!(
        mode & 0o022 == 0,
        ProgramNotUsable {
            path: abs.display().to_string(),
            why: "group/world writable",
        }
    );
    let uid = meta.uid();
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    ensure!(
        uid == 0 || uid == euid,
        "owner is neither root nor us: {}",
        abs.display()
    );
    Ok(abs)
}
