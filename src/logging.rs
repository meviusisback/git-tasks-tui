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

/// A log writer that rotates **in session** (R5-1): the event loop no longer has
/// to check anything, the rotation happens on the write that would exceed the
/// bound. Cheap because `tracing` writes a line at a time and the size is
/// tracked in memory.
#[derive(Clone)]
pub struct RotatingFile {
    inner: std::sync::Arc<std::sync::Mutex<RotatingState>>,
}

struct RotatingState {
    path: PathBuf,
    file: std::fs::File,
    written: u64,
    max_bytes: u64,
}

impl RotatingFile {
    /// Open the log and remember its current size, so rotation triggers even
    /// when the file was already over the bound at startup.
    pub fn open(path: &Path, max_bytes: u64) -> anyhow::Result<Self> {
        let file = open_log_file(path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(RotatingState {
                path: path.to_path_buf(),
                file,
                written,
                max_bytes,
            })),
        })
    }

    /// Rotate, then reopen. If anything fails the current descriptor keeps
    /// writing: losing the record is worse than a file slightly over the bound.
    fn rotate(state: &mut RotatingState) {
        if rotate_log(&state.path, 0).is_err() {
            // Silence here would void the size bound without a trace (S14-1).
            tracing::warn!(
                path = %state.path.display(),
                "log rotation failed; the size bound no longer holds"
            );
            return;
        }
        match open_log_file(&state.path) {
            Ok(file) => {
                state.file = file;
                state.written = 0;
            }
            Err(err) => {
                tracing::warn!(
                    path = %state.path.display(),
                    error = %err,
                    "cannot reopen the log after rotation"
                );
            }
        }
    }
}

impl std::io::Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("log lock is poisoned"))?;
        if state.written + buf.len() as u64 > state.max_bytes {
            Self::rotate(&mut state);
        }
        let written = state.file.write(buf)?;
        state.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("log lock is poisoned"))?;
        state.file.flush()
    }
}

/// Every log line goes through [`crate::sanitize::sanitize`] on the way out.
///
/// The log is the one sink that used to write untrusted text verbatim: a `gh`
/// failure carries stderr, and stderr carries tokens (L-1). Fixing it in the
/// layer covers every present and future call site, which a `Secret<T>` newtype
/// would not — that only protects the call sites that remember to use it.
pub struct SanitisingWriter<W> {
    inner: W,
    /// Bytes of an incomplete line. The unit of sanitising is the **line**, not
    /// the `write` call: a token split across two writes must not slip through
    /// (L-3), and a newline from untrusted text must not forge a record (L-2).
    pending: Vec<u8>,
}

impl<W> SanitisingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }
}

/// Sanitise one line of log text: the escape that keeps a record a record.
fn sanitise_line(text: &str) -> String {
    crate::sanitize::sanitize(text).replace('\n', "\\n")
}

impl<W: io::Write> SanitisingWriter<W> {
    /// Drain whole records. A record ends at the **last** newline in the
    /// buffer: newlines before it are untrusted content and get escaped, so
    /// text such as `"a\nFAKE …\n"` stays one record instead of forging one.
    fn drain_complete_lines(&mut self) -> io::Result<()> {
        while let Some(at) = self.pending.iter().rposition(|&byte| byte == b'\n') {
            let record: Vec<u8> = self.pending.drain(..=at).collect();
            let text = String::from_utf8_lossy(&record[..record.len() - 1]);
            let clean = sanitise_line(&text);
            self.inner.write_all(clean.as_bytes())?;
            self.inner.write_all(b"\n")?;
        }
        Ok(())
    }
}

impl<W: io::Write> io::Write for SanitisingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        self.drain_complete_lines()?;
        // Report the whole buffer as consumed: the caller must not retry the
        // remainder, the sanitised text is not byte-for-byte identical.
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let text = String::from_utf8_lossy(&self.pending).into_owned();
            self.pending.clear();
            let clean = sanitise_line(&text);
            self.inner.write_all(clean.as_bytes())?;
        }
        self.inner.flush()
    }
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
/// Tracks a successful `init_file_logging_at`. The guard is taken **before**
/// any filesystem work: a second call must not rotate the live log only to
/// fail at `try_init` — the subscriber would keep writing into the renamed
/// inode while `gtasks.log` became a fresh empty file (P2).
static LOGGING_INIT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

pub fn init_file_logging_at(path: &Path) -> anyhow::Result<PathBuf> {
    let mut initialised = LOGGING_INIT
        .lock()
        .map_err(|_| anyhow::anyhow!("logging init lock is poisoned"))?;
    if initialised.is_some() {
        anyhow::bail!("logging subscriber already initialised");
    }
    if let Some(parent) = path.parent() {
        ensure_state_dir(parent)?;
    }
    rotate_log(path, MAX_LOG_BYTES)?;
    let writer = RotatingFile::open(path, MAX_LOG_BYTES)?;
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(move || SanitisingWriter::new(writer.clone()))
        .with_ansi(false)
        .try_init()
        .map_err(|e| anyhow::anyhow!("logging subscriber already initialised: {e}"))?;
    *initialised = Some(path.to_path_buf());
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
