//! Probe 2/4 for the execution block — **an action runs once**.
//!
//! A write that times out is not retried: we cannot know whether the remote
//! applied it, so a second attempt would be a second write. This probe counts
//! invocations through a file, because "no retry" is a statement about how many
//! processes started, not about what the code says.
//!
//! Measured on the tip: 1 invocation, outcome `Unknown`.
//! Mutation that must make it fail: adding a retry loop around `run_pipe_with_caps`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use gtasks::actions_ui::{execute, Executed};
use gtasks::env::git_env_from;
use gtasks::gh::{Argv, CheckedArgv};

fn sh(script: &str) -> CheckedArgv {
    // `unchecked_for_tests` is the documented test-only door; it panics in debug
    // builds if given an argv the strict check refuses.
    CheckedArgv::unchecked_for_tests(Argv {
        program: PathBuf::from("/bin/sh"),
        args: vec![OsString::from("-c"), OsString::from(script)],
        env: git_env_from(&Default::default()),
        cwd: std::env::temp_dir(),
    })
}

#[test]
fn a_timed_out_write_runs_exactly_once() {
    let marker = format!("/tmp/gtt-runs-{}", std::process::id());
    let _ = std::fs::remove_file(&marker);

    let outcome = execute(
        &sh(&format!("echo x >> {marker}; sleep 10")),
        Duration::from_millis(500),
    );

    // Give a hypothetical retry time to leave its own mark.
    std::thread::sleep(Duration::from_millis(1200));
    let runs = std::fs::read_to_string(&marker)
        .unwrap_or_default()
        .lines()
        .count();
    let _ = std::fs::remove_file(&marker);

    assert_eq!(runs, 1, "a timed-out write must not be retried: {runs} processes ran");
    assert!(
        matches!(outcome, Executed::Unknown(_)),
        "a timeout must be reported as Unknown (the remote may have applied it), not as {outcome:?}"
    );
}
