//! S1/S11/S12: argv shape, forbidden flags, and bounded pipe execution.
//!
//! Runner tests override `argv.program` with an absolute fake (verified by
//! `run_pipe` through `binaries::verify`), so no `PATH` mutation is needed.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gtasks::gh::{Argv, Caps, Timeout, gh, git, run_pipe, run_pipe_with_caps};

fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn flat(argv: &Argv) -> Vec<String> {
    argv.args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// `Argv` with an absolute fake program and a minimal env.
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

/// S1: metacharacters stay inside a single argv element; nothing is re-split
/// or interpreted by a shell.
#[test]
fn argv_never_shell_metachars() {
    let nasty = "owner/repo; rm -rf / #$(id) `whoami` | tee";
    let argv = gh(Path::new("/tmp"), args(&["repo", "view", nasty]));
    assert_eq!(argv.args.len(), 3);
    assert_eq!(argv.args[2], OsString::from(nasty));

    // End to end: the fake prints each argv element wrapped, proving the
    // nasty string arrived as ONE element, never interpreted.
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(tmp.path(), "gh", "printf '<%s>' \"$@\"");
    let mut argv = gh(tmp.path(), args(&["repo", "view", nasty]));
    argv.program = program;
    let out = run_pipe(&argv, Duration::from_secs(10)).expect("fake gh runs");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!("<{nasty}>")),
        "nasty arg must survive verbatim, got: {text}"
    );
}

/// S9: `--admin`, `--force` and `--auto` can never be constructed — the
/// builders panic on the exact element, for `gh` and `git` alike.
#[test]
fn argv_forbids_admin_force_auto() {
    for flag in ["--admin", "--force", "--auto"] {
        for (tool, make) in [
            ("gh", gh as fn(&Path, Vec<OsString>) -> Argv),
            ("git", git as fn(&Path, Vec<OsString>) -> Argv),
        ] {
            let caught = std::panic::catch_unwind(|| make(Path::new("/tmp"), args(&["x", flag])));
            assert!(
                caught.is_err(),
                "{tool} must panic on forbidden flag {flag}"
            );
        }
    }
    // Benign neighbours stay legal: prefix matches and values are fine.
    let argv = gh(Path::new("/tmp"), args(&["pr", "merge", "1", "--squash"]));
    assert_eq!(flat(&argv), ["pr", "merge", "1", "--squash"]);
    let argv = git(Path::new("/tmp"), args(&["--force-with-lease"]));
    assert!(flat(&argv).contains(&"--force-with-lease".to_string()));
}

/// S16: every git invocation carries the three overrides, first, in one place.
#[test]
fn git_always_carries_s16_overrides() {
    let argv = git(Path::new("/tmp"), args(&["status", "--porcelain"]));
    let flat = flat(&argv);
    assert_eq!(
        &flat[..6],
        [
            "-c",
            "core.hooksPath=/nonexistent-hooks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.pager=cat"
        ]
    );
    assert_eq!(&flat[6..], ["status", "--porcelain"]);
}

/// S11: a hung child is killed at the deadline (SIGTERM then SIGKILL to the
/// group) and the call returns instead of blocking forever. The trap script
/// spawns a grandchild `sleep` in the same group: if only the direct child
/// died, the sleeper would linger and the /proc scan would catch it.
#[test]
fn hung_gh_kills_process_group() {
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(tmp.path(), "hang", "sleep 25");

    let started = Instant::now();
    let err = run_pipe(&argv_for(program, tmp.path()), Duration::from_millis(300))
        .expect_err("must time out");
    assert!(
        err.downcast_ref::<Timeout>().is_some(),
        "the error must be a Timeout, got: {err:#}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "timeout must fire promptly"
    );

    // No live `sleep 25` may survive: scan /proc directly (no pgrep
    // dependency) and ignore zombies, which init has yet to reap.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let live = live_processes_matching("sleep 25");
        if live.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "sleepers survived the group kill: {live:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Pids whose full command line equals `want` and whose state is not zombie.
fn live_processes_matching(want: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid): Option<u32> = name.to_string_lossy().parse().ok() else {
            continue;
        };
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let flat: Vec<String> = cmdline
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        if flat.join(" ") != want {
            continue;
        }
        // /proc/<pid>/stat: state is the field after the last ')' (comm may
        // contain spaces or parens).
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let state = stat
            .rfind(')')
            .and_then(|i| stat[i + 2..].split(' ').next());
        if state != Some("Z") {
            out.push(pid);
        }
    }
    out
}

/// S12: a stderr flood is capped, and the flag says so.
#[test]
fn stderr_flood_capped() {
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(
        tmp.path(),
        "flood",
        "head -c 200000 /dev/zero | tr '\\0' x >&2",
    );

    let out = run_pipe_with_caps(
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

/// S12: an 8 MiB JSON payload is capped before any parse (parsing is Task 3).
#[test]
fn huge_json_capped_before_parse() {
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(
        tmp.path(),
        "huge",
        "printf '{\"data\":\"'; head -c 8388608 /dev/zero | tr '\\0' x; printf '\"}'",
    );

    let out =
        run_pipe(&argv_for(program, tmp.path()), Duration::from_secs(20)).expect("must finish");
    assert!(out.status.success(), "fake child must exit 0");
    assert!(
        out.stdout.len() <= 4 * 1024 * 1024,
        "stdout must be capped at 4 MiB, got {}",
        out.stdout.len()
    );
    assert!(out.truncated, "truncation must be reported");
}

/// S12: both pipes are drained concurrently — a child that fills stdout and
/// stderr past the pipe buffers before exiting cannot deadlock the reader.
#[test]
fn both_pipes_drained_no_deadlock() {
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(
        tmp.path(),
        "both",
        "head -c 3000000 /dev/zero | tr '\\0' a; head -c 30000 /dev/zero | tr '\\0' b >&2; exit 0",
    );

    let out = run_pipe(&argv_for(program, tmp.path()), Duration::from_secs(20))
        .expect("must finish without deadlock");
    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 3_000_000);
    assert_eq!(out.stderr.len(), 30_000);
    assert!(!out.truncated, "nothing should be truncated here");
}
