//! File logging bootstrap: state dir + log file, symlink-safe (S5/S14, F-1/F-2).
//!
//! - State dir: created **per component** (never `create_dir_all`, which
//!   follows symlinks); every existing component must be a real directory.
//!   Final permissions are set on an `O_DIRECTORY | O_NOFOLLOW` fd, never on
//!   a path, so a swapped-in symlink chmods nothing.
//! - Log file: opened with `O_NOFOLLOW` (`ELOOP` on a symlink → `Err` → the
//!   stderr fallback in `main`); permissions set on the fd.
//! - Rotation (F-5): at 5 MiB `gtasks.log` becomes `gtasks.log.1` (one
//!   backup); symlinks are refused before any rename.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, ensure};
use tracing_subscriber::EnvFilter;

/// Max log size before rotation: 5 MiB, a single `gtasks.log.1` backup (F-5).
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Resolve the log file path: `$XDG_STATE_HOME/gtasks/gtasks.log`
/// (falls back to `~/.local/state/gtasks/gtasks.log`).
pub fn log_file_path() -> anyhow::Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "gtasks")
        .context("cannot locate state directory")?;
    let state_dir = dirs.state_dir();
    debug_assert!(
        state_dir.is_some(),
        "ProjectDirs::state_dir is always Some on Linux"
    );
    let state_dir = state_dir.context("no state directory on this platform")?;
    Ok(state_dir.join("gtasks.log"))
}

fn fchmod(fd: std::os::unix::io::RawFd, mode: u32, what: &str) -> anyhow::Result<()> {
    if unsafe { libc::fchmod(fd, mode) } != 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("cannot chmod {mode:o} {what}"));
    }
    Ok(())
}

/// Ensure `dir` exists as a chain of REAL directories, mode 0700 (S14, F-1).
///
/// Every existing component is checked with `symlink_metadata` and refused
/// unless it is a real directory; missing components are created one level
/// at a time with mode 0700. A `..` component is refused outright. Final
/// permissions go on an `O_DIRECTORY | O_NOFOLLOW` fd, never on the path.
pub fn ensure_state_dir(dir: &Path) -> anyhow::Result<()> {
    // Refuse `..` before creating anything: otherwise a trailing `..` after a
    // missing component creates that component and only then bails, leaving a
    // half-built tree behind.
    if dir.components().any(|c| c == Component::ParentDir) {
        anyhow::bail!("refusing state dir with `..`: {}", dir.display());
    }
    let mut prefix = PathBuf::new();
    for comp in dir.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                prefix.push(comp);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                anyhow::bail!("refusing state dir with `..`: {}", dir.display());
            }
            Component::Normal(name) => {
                prefix.push(name);
                match std::fs::symlink_metadata(&prefix) {
                    Ok(meta) => {
                        let ft = meta.file_type();
                        ensure!(
                            !ft.is_symlink(),
                            "refusing symlinked state dir component {}",
                            prefix.display()
                        );
                        ensure!(
                            ft.is_dir(),
                            "state dir component {} is not a directory",
                            prefix.display()
                        );
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        std::fs::DirBuilder::new()
                            .mode(0o700)
                            .create(&prefix)
                            .with_context(|| {
                                format!("cannot create state dir {}", prefix.display())
                            })?;
                    }
                    Err(e) => {
                        return Err(e).with_context(|| {
                            format!("cannot stat state dir {}", prefix.display())
                        });
                    }
                }
            }
        }
    }

    // Open the final dir without following a swapped-in symlink, verify it
    // is a directory, and chmod the fd (never the path).
    let fd = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(dir)
        .with_context(|| format!("cannot open state dir {}", dir.display()))?;
    ensure!(
        fd.metadata()
            .with_context(|| format!("cannot fstat state dir {}", dir.display()))?
            .is_dir(),
        "state dir {} is not a directory",
        dir.display()
    );
    fchmod(
        fd.as_raw_fd(),
        0o700,
        &format!("state dir {}", dir.display()),
    )?;
    Ok(())
}

/// Open (or create) the log file with mode 0600 (S14, F-2).
///
/// `O_NOFOLLOW` makes a symlinked path fail with `ELOOP` → `Err` → the
/// caller falls back to stderr instead of writing through the link.
/// Permissions of pre-existing regular files are hardened via `fchmod` on
/// the fd; fresh files are born 0600 (`mode()` is atomic at creation).
pub fn open_log_file(path: &Path) -> anyhow::Result<std::fs::File> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        ensure!(
            !meta.file_type().is_symlink(),
            "refusing symlinked log file {}",
            path.display()
        );
        ensure!(
            meta.file_type().is_file(),
            "log path {} is not a regular file",
            path.display()
        );
    }
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("cannot open log file {}", path.display()))?;
    ensure!(
        file.metadata()
            .with_context(|| format!("cannot fstat log file {}", path.display()))?
            .is_file(),
        "log path {} is not a regular file",
        path.display()
    );
    fchmod(
        file.as_raw_fd(),
        0o600,
        &format!("log file {}", path.display()),
    )?;
    Ok(file)
}

/// Rotate `gtasks.log` → `gtasks.log.1` once it reaches `max_bytes`
/// (F-5, a single backup). Symlinks and non-files are refused before any
/// rename; a missing file is a no-op.
///
/// Public and threshold-parameterised so the rotation contract (refuse links,
/// replace the old backup, keep one generation) is testable with a few bytes
/// instead of 5 MiB. Production callers pass [`MAX_LOG_BYTES`].
pub fn rotate_log(path: &Path, max_bytes: u64) -> anyhow::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e).with_context(|| format!("cannot stat log file {}", path.display()));
        }
        Ok(meta) => meta,
    };
    ensure!(
        !meta.file_type().is_symlink(),
        "refusing to rotate symlinked log file {}",
        path.display()
    );
    ensure!(
        meta.file_type().is_file(),
        "log path {} is not a regular file",
        path.display()
    );
    if meta.len() < max_bytes {
        return Ok(());
    }
    let backup = path.with_extension("log.1");
    // Single history: drop any previous backup first (missing is fine).
    match std::fs::remove_file(&backup) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e)
                .with_context(|| format!("cannot remove old backup {}", backup.display()));
        }
    }
    std::fs::rename(path, &backup)
        .with_context(|| format!("cannot rotate {} to {}", path.display(), backup.display()))?;
    Ok(())
}

/// Log level from `GTASKS_LOG` (a `tracing_subscriber::EnvFilter` directive),
/// defaulting to `info`.
pub fn log_filter() -> EnvFilter {
    EnvFilter::try_from_env("GTASKS_LOG").unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Initialise `tracing` to append to the state-dir log file (no ANSI codes).
/// Returns the log path so `main` can report it.
pub fn init_file_logging() -> anyhow::Result<PathBuf> {
    init_file_logging_at(&log_file_path()?)
}

/// Same as [`init_file_logging`], for an explicit path (tests and callers that
/// already resolved it).
///
/// Uses `try_init`, **not** `init`: a second initialisation in the same process
/// (Task 5 suspend/restore, Task 11a spawning an agent) returns `Err` instead of
/// panicking. The log is rotated **at startup** only (R5-1); in-session rotation
/// belongs to the Task 5 event loop.
pub fn init_file_logging_at(path: &Path) -> anyhow::Result<PathBuf> {
    if let Some(parent) = path.parent() {
        ensure_state_dir(parent)?;
    }
    rotate_log(path, MAX_LOG_BYTES)?;
    let file = open_log_file(path)?;
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init()
        .map_err(|e| anyhow::anyhow!("logging subscriber already initialised: {e}"))?;
    Ok(path.to_path_buf())
}

/// Fallback when the log file cannot be opened: log to stderr instead of
/// failing to start.
pub fn init_stderr_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init()
        .ok();
}
