//! The spawn choke point: a literal `Argv` cannot smuggle a forbidden flag,
//! and a spawn error that is not ETXTBSY fails immediately (no retry loop).

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use gtasks::gh::{Argv, Caps, run_pipe_with_caps};

fn argv(program: PathBuf, args: Vec<OsString>) -> Argv {
    Argv {
        program,
        args,
        env: vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))],
        cwd: std::env::temp_dir(),
    }
}

#[test]
fn a_literal_argv_cannot_pass_a_forbidden_flag() {
    // Built by hand, bypassing `gh()`/`git()` on purpose.
    let argv = argv(
        PathBuf::from("/bin/echo"),
        vec![OsString::from("--admin"), OsString::from("merge")],
    );

    let err = run_pipe_with_caps(&argv, Duration::from_secs(5), Caps::default()).unwrap_err();
    assert!(
        err.to_string().contains("forbidden flag"),
        "must be refused before spawn: {err:#}"
    );
}

#[test]
fn every_forbidden_flag_is_refused() {
    // Long and short forms, plus the value-normalised spelling: `--force=true`
    // must not slip past an exact match (T2-3b).
    for flag in [
        "--admin",
        "--force",
        "--auto",
        "--delete-branch",
        "-f",
        "-d",
        "--force=true",
        "--admin=yes",
        "--delete-branch=1",
    ] {
        let argv = argv(PathBuf::from("/bin/echo"), vec![OsString::from(flag)]);
        let err = run_pipe_with_caps(&argv, Duration::from_secs(5), Caps::default()).unwrap_err();
        assert!(
            err.to_string().contains("forbidden flag"),
            "{flag}: {err:#}"
        );
    }
}

#[test]
fn a_similar_flag_stays_legal() {
    let argv = argv(
        PathBuf::from("/bin/echo"),
        vec![OsString::from("--force-with-lease")],
    );
    let output = run_pipe_with_caps(&argv, Duration::from_secs(5), Caps::default()).unwrap();
    assert!(output.status.success());
}
