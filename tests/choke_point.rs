//! The spawn choke point: a literal `Argv` cannot smuggle a forbidden flag,
//! and a spawn error that is not ETXTBSY fails immediately (no retry loop).

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use gtasks::binaries::Binaries;
use gtasks::gh::{Argv, Caps, gh, run_pipe_with_caps};

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
        "-f",
        "-d",
        "--force=true",
        "--admin=yes",
    ] {
        let argv = argv(PathBuf::from("/bin/echo"), vec![OsString::from(flag)]);
        let err = run_pipe_with_caps(&argv, Duration::from_secs(5), Caps::default()).unwrap_err();
        assert!(
            err.to_string().contains("forbidden flag"),
            "{flag}: {err:#}"
        );
    }
}

/// `--delete-branch` left the deny-list when the Task 9 allowlist made the
/// action lists the authority (A-1): it is an option the app offers, with a
/// confirmation. The transport must not refuse what an action may emit.
#[test]
fn a_flag_an_action_may_emit_is_not_refused_by_the_transport() {
    let argv = argv(
        PathBuf::from("/bin/echo"),
        vec![OsString::from("--delete-branch")],
    );
    let out = run_pipe_with_caps(&argv, Duration::from_secs(5), Caps::default()).unwrap();
    assert!(out.status.success());
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

#[test]
fn a_short_flag_cluster_is_decomposed() {
    // `-fd` hides `-f` and `-d`: a per-element match would let it through.
    // Only clusters made entirely of tracked short flags are decomposed: `-af`
    // is not one, because `a` is not a flag we track (A-2).
    for cluster in ["-fd", "-df", "-dfd"] {
        let argv = argv(PathBuf::from("/bin/echo"), vec![OsString::from(cluster)]);
        let err = run_pipe_with_caps(&argv, Duration::from_secs(5), Caps::default()).unwrap_err();
        assert!(
            err.to_string().contains("forbidden flag"),
            "{cluster} must be refused: {err:#}"
        );
    }
}

#[test]
fn a_harmless_cluster_stays_legal() {
    let argv = argv(PathBuf::from("/bin/echo"), vec![OsString::from("-n")]);
    let out = run_pipe_with_caps(&argv, Duration::from_secs(5), Caps::default()).unwrap();
    assert!(out.status.success());
}

/// A public builder must not abort the process on a coding slip: the flag dies
/// at the transport, where there is a caller that can show the error.
#[test]
fn a_public_builder_cannot_panic_on_a_forbidden_flag() {
    let binaries = Binaries {
        gh: PathBuf::from("/bin/echo"),
        git: PathBuf::from("/bin/true"),
        hermes: None,
        xdg_open: None,
    };
    let cwd = std::env::current_dir().unwrap();

    // No panic: the argv is built.
    let built = gh(&binaries, &cwd, vec![OsString::from("--admin")]);

    // And the transport refuses it, which is where a caller can react.
    let err = run_pipe_with_caps(&built, Duration::from_secs(5), Caps::default()).unwrap_err();
    assert!(
        err.to_string().contains("forbidden flag"),
        "the transport must refuse it: {err:#}"
    );
}
