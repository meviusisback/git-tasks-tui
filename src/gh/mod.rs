//! Process layer: build argv arrays (never shell strings), spawn pipe children
//! with a bounded environment, and kill the whole process group on timeout.
//!
//! `Argv` is the inspectable contract the audit harness asserts on: `program`,
//! `args`, `env` and `cwd` are plain values, so tests never spawn to check them.
//!
//! Production pattern (S2): builders take no program — the caller resolves it:
//! ```ignore
//! let mut argv = gh(cwd, &args);
//! argv.program = binaries::resolve("gh")?;
//! ```
//! Tests override `argv.program` with an absolute fake (verified by
//! `run_pipe` through `binaries::verify`, so every spawned binary is
//! checked).

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};

use crate::env::{gh_env, git_env};

/// A fully-resolved child invocation. Nothing is looked up at spawn time
/// except a bare `program` name, resolved through `binaries::resolve`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Argv {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
}

impl Argv {
    /// Build the `Command` with a cleared environment and a fresh process
    /// group (so a timeout can kill the child **and its descendants**).
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .current_dir(&self.cwd)
            .env_clear()
            .stdin(Stdio::null());
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd.process_group(0);
        // S11/T2-6: if gtasks dies (even with SIGKILL) the child must not
        // outlive it. `pre_exec` runs in the child between fork and exec, so
        // only async-signal-safe calls belong here.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        cmd
    }
}

/// Flags that must never be produced by the builders (S9). Exact element
/// match only: `--force-with-lease` and friends stay legal.
const FORBIDDEN: &[&str] = &["--admin", "--force", "--auto"];

/// The choke point: every argv passes here, whether it came from `gh()`/`git()`
/// or from a literal `Argv` (the fields are `pub`, so a factory bypass must not
/// be enough). Returns an error, never panics: this is a runtime refusal, not a
/// programmer assertion.
pub fn check_argv(argv: &Argv) -> anyhow::Result<()> {
    for arg in &argv.args {
        let text = arg.to_string_lossy();
        anyhow::ensure!(
            !FORBIDDEN.iter().any(|flag| text == *flag),
            "refusing forbidden flag {text:?}"
        );
    }
    Ok(())
}

fn forbid_admin_force_auto(args: &[OsString], tool: &str) {
    for arg in args {
        let text = arg.to_string_lossy();
        assert!(
            !FORBIDDEN.iter().any(|flag| text == *flag),
            "{tool}: forbidden flag {text:?} must never be constructed"
        );
    }
}

/// `gh` invocation: argv only, `gh` environment (token + non-interactive knobs).
/// The caller fills in `program` with `binaries::resolve("gh")`.
pub fn gh(cwd: &Path, args: Vec<OsString>) -> Argv {
    forbid_admin_force_auto(&args, "gh");
    Argv {
        program: PathBuf::from("gh"),
        args,
        env: gh_env(),
        cwd: cwd.to_path_buf(),
    }
}

/// The **single** place where a `git` invocation is composed. Every call gets
/// the S16 overrides, so no repository hook, fsmonitor or pager can run.
/// The caller fills in `program` with `binaries::resolve("git")`.
pub fn git(cwd: &Path, args: Vec<OsString>) -> Argv {
    forbid_admin_force_auto(&args, "git");
    let mut full: Vec<OsString> = Vec::with_capacity(args.len() + 6);
    for value in [
        "-c",
        "core.hooksPath=/nonexistent-hooks",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.pager=cat",
    ] {
        full.push(value.into());
    }
    full.extend(args);
    Argv {
        program: PathBuf::from("git"),
        args: full,
        env: git_env(),
        cwd: cwd.to_path_buf(),
    }
}

/// Output caps for a pipe child. `stdout` is large enough for a `--json`
/// payload; `stderr` is capped much lower, since a flood there is either a
/// broken tool or an attack.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub stdout: usize,
    pub stderr: usize,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            stdout: 4 * 1024 * 1024,
            stderr: 64 * 1024,
        }
    }
}

/// A finished pipe child.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

/// The child was still running at the deadline and was killed.
#[derive(Debug)]
pub struct Timeout {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl std::fmt::Display for Timeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "child timed out (stdout {} bytes, stderr {} bytes)",
            self.stdout.len(),
            self.stderr.len()
        )
    }
}

impl std::error::Error for Timeout {}

/// Kills the child's process group on drop unless disarmed — the synchronous
/// equivalent of `kill_on_drop`. Any early return after `spawn` (an error from
/// `try_wait`, a panic while collecting output) must not leave the child and
/// its descendants running (S11).
struct GroupGuard {
    pid: libc::pid_t,
    armed: bool,
}

impl GroupGuard {
    fn new(pid: libc::pid_t) -> Self {
        Self { pid, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.armed {
            terminate_group(self.pid);
        }
    }
}

/// Resolve `program` (bare names through `binaries::resolve`, absolute paths
/// through `binaries::verify`) so every spawned binary is checked (S2).
fn resolve_program(program: &Path) -> anyhow::Result<PathBuf> {
    if program.is_absolute() {
        crate::binaries::verify(program)
    } else {
        let name = program.to_string_lossy();
        crate::binaries::resolve(&name)
    }
}

/// Spawn with a bounded retry on `ETXTBSY` ("Text file busy"): executing a
/// file that was *just* written can transiently fail on a loaded machine
/// (measured flakes under parallel test load). Anything else fails fast.
fn spawn_retry(
    cmd: &mut std::process::Command,
    program: &Path,
) -> anyhow::Result<std::process::Child> {
    // Invariant: retrying is safe **only** because `ETXTBSY` comes from a failed
    // `execve` — the program never ran, so no effect can be duplicated. Never
    // retry a spawn that might have started (that is why writes are never
    // retried).
    let mut last = None;
    for _ in 0..10 {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if is_retryable_spawn_error(&e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                return Err(e).with_context(|| format!("cannot spawn {}", program.display()));
            }
        }
    }
    Err(last.expect("retry loop ran"))
        .with_context(|| format!("cannot spawn {} (still busy)", program.display()))
}

/// Only `ETXTBSY` is retryable: it comes from a failed `execve`, so nothing ran.
/// Every other error (missing file, permissions, bad format) fails immediately.
fn is_retryable_spawn_error(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(libc::ETXTBSY)
}

/// Run a pipe child to completion, bounded by `timeout` and the default caps.
pub fn run_pipe(argv: &Argv, timeout: Duration) -> anyhow::Result<Output> {
    run_pipe_with_caps(argv, timeout, Caps::default())
}

/// How long we wait for a reader after the child exits. A grandchild that
/// inherited the pipe can hold it open, and a plain `join()` would hang the
/// TUI forever (T2-5).
const READER_DEADLINE: Duration = Duration::from_secs(2);

/// Wait on a reader through a channel, so the wait can be bounded.
fn spawn_waiter<T: Send + 'static>(
    handle: std::thread::JoinHandle<T>,
) -> std::sync::mpsc::Receiver<Option<T>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(handle.join().ok());
    });
    rx
}

/// Collect both readers under **one** shared deadline. A reader that misses it
/// is left detached and its stream is reported as `(empty, truncated = true)`:
/// an incomplete payload must never look whole (T2-5).
fn collect_readers(
    out: std::sync::mpsc::Receiver<Option<(Vec<u8>, bool)>>,
    err: std::sync::mpsc::Receiver<Option<(Vec<u8>, bool)>>,
    deadline: Duration,
) -> ((Vec<u8>, bool), (Vec<u8>, bool)) {
    let end = Instant::now() + deadline;
    let take = |rx: std::sync::mpsc::Receiver<Option<(Vec<u8>, bool)>>| {
        let left = end.saturating_duration_since(Instant::now());
        rx.recv_timeout(left)
            .ok()
            .flatten()
            .unwrap_or((Vec::new(), true))
    };
    (take(out), take(err))
}

/// Run a pipe child with explicit caps (tests use small caps to trigger
/// truncation without moving megabytes around).
pub fn run_pipe_with_caps(argv: &Argv, timeout: Duration, caps: Caps) -> anyhow::Result<Output> {
    // S2: the program is resolved once, by the caller. At spawn there is no
    // PATH lookup — a relative program is a bug, not a convenience (T2-2).
    anyhow::ensure!(
        argv.program.is_absolute(),
        "refusing a non-absolute program: {}",
        argv.program.display()
    );
    check_argv(argv)?;
    let mut argv = argv.clone();
    argv.program = resolve_program(&argv.program)?;
    let mut cmd = argv.command();
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = spawn_retry(&mut cmd, &argv.program)?;
    let pid = child.id() as libc::pid_t;
    let mut guard = GroupGuard::new(pid);

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let reader_out = std::thread::spawn(move || read_capped(stdout, caps.stdout));
    let reader_err = std::thread::spawn(move || read_capped(stderr, caps.stderr));
    let wait_out = spawn_waiter(reader_out);
    let wait_err = spawn_waiter(reader_err);

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().context("cannot wait for child")? {
            guard.disarm();
            // An incomplete read is reported as truncated: the caller must not
            // treat a partial payload as whole (T2-5).
            let ((stdout, out_truncated), (stderr, err_truncated)) =
                collect_readers(wait_out, wait_err, READER_DEADLINE);
            return Ok(Output {
                status,
                stdout,
                stderr,
                truncated: out_truncated || err_truncated,
            });
        }
        if Instant::now() >= deadline {
            terminate_group(pid);
            let _ = child.wait();
            guard.disarm();
            let ((stdout, out_truncated), (stderr, err_truncated)) =
                collect_readers(wait_out, wait_err, READER_DEADLINE);
            tracing::warn!(
                out_truncated,
                err_truncated,
                "child timed out; output may be incomplete"
            );
            return Err(Timeout { stdout, stderr }.into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// SIGTERM the group, give it a moment, then SIGKILL whatever is left.
fn terminate_group(pid: libc::pid_t) {
    // SAFETY: kill(2) on a negative pid signals our own process group, which
    // exists because the child was spawned with `process_group(0)`.
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(200));
    // SAFETY: as above; harmless if the group is already gone.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

/// Read until EOF, keeping at most `cap` bytes and reporting truncation.
fn read_capped<R: Read>(mut reader: R, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if kept.len() < cap {
                    let take = (cap - kept.len()).min(n);
                    kept.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }
    (kept, truncated)
}

/// Validated `gh --json` field list helper: rejects empty fields and anything
/// that is not a plain field name (S4).
pub fn json_fields(fields: &[&str]) -> anyhow::Result<String> {
    ensure!(!fields.is_empty(), "at least one --json field is required");
    for field in fields {
        ensure!(
            !field.is_empty()
                && field
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '{' || c == '}'),
            "invalid --json field: {field:?}"
        );
    }
    Ok(fields.join(","))
}

#[cfg(test)]
mod retry_tests {
    use super::is_retryable_spawn_error;

    #[test]
    fn only_etxtbsy_is_retryable() {
        let etxtbsy = std::io::Error::from_raw_os_error(libc::ETXTBSY);
        assert!(
            is_retryable_spawn_error(&etxtbsy),
            "ETXTBSY is the one case"
        );

        for code in [libc::EACCES, libc::ENOENT, libc::ENOEXEC, libc::EPERM] {
            let err = std::io::Error::from_raw_os_error(code);
            assert!(
                !is_retryable_spawn_error(&err),
                "errno {code} must not be retried"
            );
        }
    }
}
