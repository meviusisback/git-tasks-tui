//! The spawn choke point: a literal `Argv` cannot smuggle a forbidden flag,
//! and a spawn error that is not ETXTBSY fails immediately (no retry loop).

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use gtasks::gh::{Argv, Caps, check_argv, run_pipe_with_caps};

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

/// `-d` is `--delete-branch`, which an action emits on request: the deny-list
/// must not forbid a spelling of a flag the program legitimately builds. The
/// cluster guard still catches `-fd`, because `f` is inside it.
#[test]
fn the_short_delete_flag_is_allowed_but_force_inside_a_cluster_is_not() {
    let alone = argv(PathBuf::from("/bin/true"), vec![OsString::from("-d")]);
    assert!(check_argv(&alone).is_ok(), "-d is --delete-branch: allowed");

    let cluster = argv(PathBuf::from("/bin/true"), vec![OsString::from("-fd")]);
    let err = check_argv(&cluster).unwrap_err().to_string();
    assert!(
        err.contains(r#""-f" inside "-fd""#),
        "the cluster hides -f: {err}"
    );

    // And a value that merely starts with a dash is still a value.
    let value = argv(
        PathBuf::from("/bin/true"),
        vec![OsString::from("-done"), OsString::from("-admin")],
    );
    assert!(check_argv(&value).is_ok(), "a value is not a flag");
}
