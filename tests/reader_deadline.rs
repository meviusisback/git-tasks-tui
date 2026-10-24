//! T2-5: a grandchild that inherits the pipe must not hang the caller.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gtasks::gh::{Argv, run_pipe};

fn sh(script: &str) -> Argv {
    Argv {
        program: PathBuf::from("/bin/sh"),
        args: vec![OsString::from("-c"), OsString::from(script)],
        env: vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))],
        cwd: std::env::temp_dir(),
    }
}

#[test]
fn a_grandchild_holding_the_pipe_does_not_hang_the_caller() {
    let start = Instant::now();
    // `sleep` inherits stdout, so the pipe stays open after `sh` exits.
    let output = run_pipe(&sh("sleep 5 & exit 0"), Duration::from_secs(10)).unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "the caller must not wait for the grandchild, took {elapsed:?}"
    );
    assert!(
        output.truncated,
        "incomplete output must be flagged, not silently empty"
    );
}

#[test]
fn a_normal_child_still_returns_whole_output() {
    let output = run_pipe(&sh("echo hello"), Duration::from_secs(10)).unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
    assert!(!output.truncated, "a complete read is not flagged");
}
