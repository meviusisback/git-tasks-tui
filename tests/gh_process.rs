//! S11/S12: pipe children are bounded — timeout kills the group, output is
//! capped, and both streams are drained concurrently (no deadlock).
//!
//! The fake children are real scripts on disk, executed by argv; no shell is
//! involved in our code path.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gtasks::gh::{Argv, Caps, Timeout, run_pipe};

/// Serialises the three tests: each writes an executable script and spawns it,
/// and concurrent fork/exec of a file another thread has open for writing can
/// fail with ETXTBSY. Serialising here keeps the product API honest (no retry
/// loop inside `run_pipe` just for tests).
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn argv_for(program: PathBuf, cwd: &Path) -> Argv {
    Argv {
        program,
        args: Vec::new(),
        env: vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), cwd.as_os_str().to_owned()),
        ],
        cwd: cwd.to_path_buf(),
    }
}

/// A hung child is killed at the deadline (SIGTERM then SIGKILL to the group),
/// and the call returns instead of blocking forever.
#[test]
fn hung_child_is_killed_on_timeout() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(tmp.path(), "hang", "sleep 30");

    let started = Instant::now();
    let err = run_pipe(
        &argv_for(program, tmp.path()),
        Duration::from_millis(300),
        Caps::default(),
    )
    .expect_err("must time out");

    assert!(
        err.downcast_ref::<Timeout>().is_some(),
        "the error must be a Timeout, got: {err:#}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "timeout must fire promptly"
    );
}

/// A stderr flood is capped, and the flag says so.
#[test]
fn stderr_flood_is_capped() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(
        tmp.path(),
        "flood",
        "head -c 200000 /dev/zero | tr '\\0' x >&2",
    );

    let out = run_pipe(
        &argv_for(program, tmp.path()),
        Duration::from_secs(10),
        Caps {
            stdout: 1024,
            stderr: 4096,
        },
    )
    .expect("must finish");

    assert!(out.status.success(), "fake child must exit 0");
    assert!(out.stderr.len() <= 4096, "stderr must be capped");
    assert!(out.truncated, "truncation must be reported");
}

/// Both pipes are drained concurrently: a child that fills stdout and stderr
/// before exiting cannot deadlock the reader.
#[test]
fn both_pipes_are_drained_no_deadlock() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(
        tmp.path(),
        "both",
        "head -c 100000 /dev/zero | tr '\\0' a; head -c 100000 /dev/zero | tr '\\0' b >&2; exit 0",
    );

    let out = run_pipe(
        &argv_for(program, tmp.path()),
        Duration::from_secs(10),
        Caps {
            stdout: 8192,
            stderr: 8192,
        },
    )
    .expect("must finish without deadlock");

    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 8192, "stdout keeps the cap");
    assert_eq!(out.stderr.len(), 8192, "stderr keeps the cap");
    assert!(out.truncated);
}
