//! Agent launch helpers — seed-file part (S8, S14, C1).
//!
//! The fenced seed is written to a `0600` regular file inside a `0700`
//! runtime dir (`$XDG_RUNTIME_DIR/gtasks`) and passed as
//! `--query-file <path>` — never argv, never `--query-file -` (C1).
//! [`SeedFile`] unlinks the file on [`Drop`], so the seed is gone after
//! the child exits on both the normal and the failure path.
//!
//! Command building (`build_command`, `build_seed_message`) and the
//! synchronous suspend/run window are Task 11a/11b.

use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context;

static SEED_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Default runtime dir: `$XDG_RUNTIME_DIR/gtasks`.
pub fn default_runtime_dir() -> anyhow::Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "gtasks")
        .context("cannot locate runtime directory")?;
    dirs.runtime_dir()
        .map(Path::to_path_buf)
        .context("XDG_RUNTIME_DIR is not set; cannot stage the seed file")
}

/// Ensure `dir` exists with mode `0700` via the shared helper (F-1, S14).
/// The single implementation lives in `logging`; both call sites delegate.
pub fn ensure_runtime_dir(dir: &Path) -> anyhow::Result<()> {
    crate::logging::ensure_state_dir(dir)
}

/// A seed staged in a `0600` file, unlinked on drop (S8).
pub struct SeedFile {
    path: PathBuf,
}

impl SeedFile {
    /// Write `seed` to a fresh unique file inside `dir` and return the owner.
    /// The directory guarantee is **enforced here**, not assumed: `dir` must be
    /// a real `0700` directory (a symlink is refused), so a call site that
    /// forgets `ensure_runtime_dir` cannot leak the seed elsewhere (G-2/S-1).
    /// The file is created atomically with mode `0600` (`create_new` refuses
    /// to follow or trample an existing path, symlink included) and verified
    /// afterwards with `symlink_metadata`.
    pub fn create_in(dir: &Path, seed: &str) -> anyhow::Result<Self> {
        ensure_runtime_dir(dir)?;
        for _ in 0..100 {
            let n = SEED_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("seed-{}-{n}", std::process::id()));
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path);
            let mut file = match file {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("cannot create seed file {}", path.display()));
                }
            };
            use std::io::Write as _;
            file.write_all(seed.as_bytes())
                .with_context(|| format!("cannot write seed file {}", path.display()))?;
            drop(file);

            let meta = std::fs::symlink_metadata(&path)
                .with_context(|| format!("cannot stat seed file {}", path.display()))?;
            anyhow::ensure!(
                meta.file_type().is_file() && !meta.file_type().is_symlink(),
                "seed path {} is not a regular file",
                path.display()
            );
            return Ok(Self { path });
        }
        anyhow::bail!("could not pick a fresh seed-file name in {}", dir.display())
    }

    /// Absolute path to pass as `--query-file <path>`.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SeedFile {
    fn drop(&mut self) {
        // Best effort by design: teardown never unwraps (S15).
        let _ = std::fs::remove_file(&self.path);
    }
}
