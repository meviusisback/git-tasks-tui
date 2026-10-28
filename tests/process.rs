//! S1/S11/S12: argv shape, forbidden flags, and bounded pipe execution.
//!
//! Runner tests override `argv.program` with an absolute fake (verified by
//! `run_pipe` through `binaries::verify`), so no `PATH` mutation is needed.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gtasks::binaries::Binaries;
use gtasks::gh::{Argv, Caps, Timeout, check_argv, gh, git, run_pipe, run_pipe_with_caps};

/// Builders take the resolved binaries; tests hand them explicit paths, so no
/// test ever repairs `argv.program` after the fact.
fn bins(gh_path: PathBuf, git_path: PathBuf) -> Binaries {
    Binaries::new(gh_path, git_path)
}

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
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(tmp.path(), "gh", "printf '<%s>' \"$@\"");
    let binaries = bins(program, PathBuf::from("/bin/false"));

    let argv = gh(&binaries, tmp.path(), args(&["repo", "view", nasty]));
    assert_eq!(argv.args.len(), 3);
    assert_eq!(argv.args[2], OsString::from(nasty));
    assert_eq!(
        argv.program, binaries.gh,
        "the builder uses the resolved path"
    );

    // End to end: the fake prints each argv element wrapped, proving the
    // nasty string arrived as ONE element, never interpreted.
    let out = run_pipe(&argv, Duration::from_secs(10)).expect("fake gh runs");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!("<{nasty}>")),
        "nasty arg must survive verbatim, got: {text}"
    );
}

/// S9: `--admin`, `--force` and `--auto` can never reach the spawn: the
/// builders compose without panicking, and the transport refuses the argv.
#[test]
fn forbidden_flags_die_at_the_transport_not_in_the_builder() {
    let binaries = bins(PathBuf::from("/bin/true"), PathBuf::from("/bin/true"));
    for flag in ["--admin", "--force", "--auto"] {
        for (tool, make) in [
            ("gh", gh as fn(&Binaries, &Path, Vec<OsString>) -> Argv),
            ("git", git as fn(&Binaries, &Path, Vec<OsString>) -> Argv),
        ] {
            // No panic: a public builder must not abort the process.
            let built = make(&binaries, Path::new("/tmp"), args(&["x", flag]));
            let err = check_argv(&built).unwrap_err().to_string();
            assert!(
                err.contains("forbidden flag"),
                "{tool} must be refused by the transport on {flag}: {err}"
            );
        }
    }
    // Benign neighbours stay legal: prefix matches and values are fine.
    let argv = gh(
        &binaries,
        Path::new("/tmp"),
        args(&["pr", "merge", "1", "--squash"]),
    );
    assert_eq!(flat(&argv), ["pr", "merge", "1", "--squash"]);
    let argv = git(&binaries, Path::new("/tmp"), args(&["--force-with-lease"]));
    assert!(flat(&argv).contains(&"--force-with-lease".to_string()));
}

/// S16: every git invocation carries the three overrides, first, in one place.
#[test]
fn git_always_carries_s16_overrides() {
    let binaries = bins(PathBuf::from("/bin/true"), PathBuf::from("/bin/true"));
    let argv = git(
        &binaries,
        Path::new("/tmp"),
        args(&["status", "--porcelain"]),
    );
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
/// Burial is verified through /proc, so this probe is Linux-only: on
/// macOS the scanner below would find nothing and the test would pass
/// while proving nothing.
#[cfg(target_os = "linux")]
#[test]
fn hung_gh_kills_process_group() {
    let tmp = tempfile::tempdir().unwrap();
    // The script records its own pid, so the assertion below can name the **group** it
    // ran in instead of scanning the machine for a command line that happens to match.
    let pidfile = tmp.path().join("child.pid");
    let program = fake(
        tmp.path(),
        "hang",
        &format!("echo $$ > {}\nsleep 25", pidfile.display()),
    );

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

    // The runner starts the child with `process_group(0)`, so the group id is its own
    // pid. Looking at the group — not at the command line — is what makes this probe
    // about *our* child: a `sleep 25` from anywhere else on the machine is none of its
    // business, and a leftover from a previous run of this very test used to be
    // indistinguishable from a survivor.
    let group: u32 = std::fs::read_to_string(&pidfile)
        .expect("the fake must record its pid: a timeout that never started is not a timeout")
        .trim()
        .parse()
        .expect("pid");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let live = live_in_group(group);
        if live.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "processes in group {group} survived the kill: {live:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Anti-vacuity for the probe above: if [`live_in_group`] were broken — wrong field,
/// wrong parse, wrong file — the loop would find an empty group every time and the
/// timeout test would pass while proving nothing. This control asserts the scanner can
/// see a group that is demonstrably alive.
#[cfg(target_os = "linux")]
#[test]
fn the_group_scanner_sees_a_live_group() {
    use std::os::unix::process::CommandExt;

    let mut decoy = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 5")
        .process_group(0)
        .spawn()
        .expect("spawn decoy");
    let group = decoy.id();

    let seen = live_in_group(group);
    let _ = decoy.kill();
    let _ = decoy.wait();

    assert!(
        !seen.is_empty(),
        "the scanner found nothing in group {group}, which was alive: the probe above \
         would pass by finding nothing anywhere"
    );
}

/// Live pids whose **process group** is `group`, zombies excluded.
/// Linux-only (see above): cfg-gated with its two tests so macOS
/// builds stay warning-free under `-D warnings`.
#[cfg(target_os = "linux")]
fn live_in_group(group: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid): Option<u32> = name.to_string_lossy().parse().ok() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        // `comm` may contain spaces and parens, so fields are counted from the last ')'.
        // After it: state, ppid, pgrp, …
        let Some(i) = stat.rfind(')') else { continue };
        let rest: Vec<&str> = stat[i + 2..].split(' ').collect();
        let (Some(state), Some(pgrp)) = (rest.first(), rest.get(2)) else {
            continue;
        };
        if *state == "Z" {
            continue;
        }
        if pgrp.trim().parse::<u32>() == Ok(group) {
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

/// S2/T2-2: a relative program is refused at the single spawn point — the
/// caller resolves binaries, never `run_pipe`.
#[test]
fn relative_program_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let argv = Argv {
        program: "true".into(),
        args: Vec::new(),
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        cwd: tmp.path().to_path_buf(),
    };
    let err =
        run_pipe(&argv, Duration::from_secs(1)).expect_err("a relative program must be refused");
    assert!(
        format!("{err:#}").contains("non-absolute"),
        "error must explain the refusal, got: {err:#}"
    );
}

/// The other side of `relative_program_is_refused`: the builders must produce
/// an argv that the spawn accepts. Without this, a broken builder and a strict
/// spawn cancel each other out and every other test stays green.
#[test]
fn builders_produce_a_runnable_argv() {
    let binaries = Binaries::detect().expect("git and gh are on PATH here");

    let git_argv = git(&binaries, Path::new("/tmp"), vec!["--version".into()]);
    let out = run_pipe(&git_argv, Duration::from_secs(10)).expect("git builder is runnable");
    assert!(out.status.success(), "git --version failed");
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("git version"),
        "unexpected output"
    );

    let gh_argv = gh(&binaries, Path::new("/tmp"), vec!["--version".into()]);
    let out = run_pipe(&gh_argv, Duration::from_secs(10)).expect("gh builder is runnable");
    assert!(out.status.success(), "gh --version failed");
}

/// The untrusted transport accepts only read-only `gh` pairs: a probe or a
/// test that calls `run_pipe` with a real `gh` binary must not be able to
/// execute a write for real. This is the guard that prevents a test from
/// posting a comment to a live repository.
#[test]
fn untrusted_transport_refuses_gh_write_verbs() {
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(tmp.path(), "gh", "printf '<%s>' \"$@\"");
    let binaries = bins(program, PathBuf::from("/bin/false"));
    for verb in [
        "comment", "merge", "close", "reopen", "approve", "review", "delete",
    ] {
        let argv = gh(
            &binaries,
            Path::new("/tmp"),
            args(&["pr", verb, "1", "--repo", "cli/cli", "--body", "-done"]),
        );
        let err = run_pipe(&argv, Duration::from_secs(5))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("refusing gh subcommand"),
            "gh pr {verb} must be refused by the untrusted transport: {err}"
        );
    }
}

/// The guard also catches `gh api`, which can write via `--method POST`,
/// and every other non-read pair (`auth`, `extension`, …).
#[test]
fn untrusted_transport_refuses_gh_api() {
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(tmp.path(), "gh", "printf '<%s>' \"$@\"");
    let binaries = bins(program, PathBuf::from("/bin/false"));
    let argv = gh(
        &binaries,
        Path::new("/tmp"),
        args(&["api", "/repos/cli/cli"]),
    );
    let err = run_pipe(&argv, Duration::from_secs(5))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("refusing gh subcommand"),
        "gh api must be refused: {err}"
    );
}

/// Reads pass: the guard only fires on write verbs, so `gh pr list` and
/// `gh issue view` still work through the untrusted transport.
#[test]
fn untrusted_transport_allows_gh_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(tmp.path(), "gh", "printf '<%s>' \"$@\"");
    let binaries = bins(program, PathBuf::from("/bin/false"));
    for read in [
        vec!["pr", "list", "--repo", "cli/cli"],
        vec!["issue", "view", "1", "--repo", "cli/cli"],
        vec!["repo", "list"],
    ] {
        let argv = gh(&binaries, tmp.path(), args(&read));
        let out = run_pipe(&argv, Duration::from_secs(10)).expect("gh read runs");
        assert!(out.status.success(), "gh {} must succeed", read.join(" "));
    }
}

/// The guard is gh-only: `git checkout`, `git pull` and `git push` are local
/// operations the app needs through the untrusted transport.
#[test]
fn untrusted_transport_does_not_block_git() {
    let tmp = tempfile::tempdir().unwrap();
    let program = fake(tmp.path(), "git", "printf '<%s>' \"$@\"");
    let binaries = bins(PathBuf::from("/bin/false"), program);
    for op in [vec!["checkout", "main"], vec!["pull"], vec!["push"]] {
        let argv = git(&binaries, tmp.path(), args(&op));
        let out = run_pipe(&argv, Duration::from_secs(10)).expect("git runs");
        assert!(out.status.success(), "git {} must succeed", op.join(" "));
    }
}
