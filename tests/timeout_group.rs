//! Regression guard for the **process-group property** of `run_pipe` (S11/T2-6).
//!
//! `run_pipe` spawns with `process_group(0)` and `PR_SET_PDEATHSIG`, and on
//! timeout it signals the **group** (`kill(-pid, …)`), not the single child.
//! That is what makes a timeout safe for a long-running agent that spawns
//! grandchildren: killing only the child leaves the grandchildren alive.
//!
//! Measured against the tip (`1828ce3`):
//!   * as written                     → timeout ≈ 1.0 s, grandchild dead   → green
//!   * `kill(-pid, …)` → `kill(pid…)` → timeout ≈ 3.0 s, grandchild ALIVE  → red
//!
//! So this test fails both on the survival and on the latency: a wrong kill is
//! visible twice. Keep it green, and route any new executor through `run_pipe`
//! instead of writing a second spawn — a private spawn loses the group, the
//! parent-death signal and the group kill all at once.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gtasks::env::git_env_from;
use gtasks::gh::{run_pipe, Argv};

fn sh(script: &str) -> Argv {
    Argv {
        program: PathBuf::from("/bin/sh"),
        args: vec![OsString::from("-c"), OsString::from(script)],
        env: git_env_from(&Default::default()),
        cwd: std::env::temp_dir(),
    }
}

#[test]
fn a_timeout_does_not_leave_a_grandchild_alive() {
    let marker = format!("/tmp/gtt-orphan-{}", std::process::id());
    let _ = std::fs::remove_file(&marker);

    // The child records its grandchild's pid, then waits for it. If the timeout
    // kills only the child, the grandchild is reparented to init and survives.
    let script = format!("sleep 300 & echo $! > {marker}; wait");

    let started = Instant::now();
    let result = run_pipe(&sh(&script), Duration::from_millis(800));
    let took = started.elapsed();

    std::thread::sleep(Duration::from_millis(300));
    let pid = std::fs::read_to_string(&marker)
        .unwrap_or_default()
        .trim()
        .to_string();
    let alive = !pid.is_empty() && std::path::Path::new(&format!("/proc/{pid}")).exists();

    if !pid.is_empty() {
        let _ = std::process::Command::new("/bin/kill")
            .arg("-9")
            .arg(&pid)
            .status();
    }
    let _ = std::fs::remove_file(&marker);

    assert!(result.is_err(), "the run must report a timeout");
    assert!(!pid.is_empty(), "the probe must capture the grandchild pid");
    assert!(
        !alive,
        "a timed-out run left the grandchild {pid} alive: the timeout must signal \
         the process group, not just the child"
    );
    assert!(
        took < Duration::from_secs(2),
        "the timeout took {took:?}: killing only the child makes the wait long as \
         well as wrong"
    );
}
