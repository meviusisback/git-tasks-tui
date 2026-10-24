//! Process layer: build argv arrays (never shell strings), spawn pipe children
//! with a bounded environment, and kill the whole process group on timeout.
//!
//! `Argv` is the inspectable contract the audit harness asserts on: `program`,
//! `args`, `env` and `cwd` are plain values, so tests never spawn to check them.

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};

use crate::env::{Child, env_for};

/// A fully-resolved child invocation. Nothing is looked up at spawn time.
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
        cmd
    }
}

/// `gh` invocation: argv only, `gh` environment (token + non-interactive knobs).
pub fn gh(cwd: &Path, program: &Path, args: Vec<OsString>, path: &Path) -> Argv {
    Argv {
        program: program.to_path_buf(),
        args,
        env: env_for(Child::Gh, path),
        cwd: cwd.to_path_buf(),
    }
}

/// The **single** place where a `git` invocation is composed. Every call gets
/// the S16 overrides, so no repository hook, fsmonitor or pager can run.
pub fn git(cwd: &Path, program: &Path, args: Vec<OsString>, path: &Path) -> Argv {
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
        program: program.to_path_buf(),
        args: full,
        env: env_for(Child::Git, path),
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

/// Run a pipe child to completion, bounded by `timeout` and `caps`.
///
/// On timeout the **process group** gets SIGTERM, then SIGKILL after a short
/// grace period; the readers are joined so no thread outlives the call.
pub fn run_pipe(argv: &Argv, timeout: Duration, caps: Caps) -> anyhow::Result<Output> {
    let mut cmd = argv.command();
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("cannot spawn {}", argv.program.display()))?;
    let pid = child.id() as libc::pid_t;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let reader_out = std::thread::spawn(move || read_capped(stdout, caps.stdout));
    let reader_err = std::thread::spawn(move || read_capped(stderr, caps.stderr));

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().context("cannot wait for child")? {
            let (stdout, out_truncated) = reader_out.join().unwrap_or_default();
            let (stderr, err_truncated) = reader_err.join().unwrap_or_default();
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
            let (stdout, _) = reader_out.join().unwrap_or_default();
            let (stderr, _) = reader_err.join().unwrap_or_default();
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
