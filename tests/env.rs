//! S3: per-child allowlists, dirty-parent simulation, and live controls.
//!
//! The `_from` cores take the parent environment as a value, so a hostile
//! parent (`GIT_DIR`, `GIT_CONFIG_COUNT`, tokens) is simulated without ever
//! mutating the process environment. The two `*_control` tests run the real
//! `git` WITHOUT the wrapper to prove the injection works — otherwise the
//! stripped assertions would be vacuous.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use gtasks::binaries::{self, Binaries};
use gtasks::env::{agent_env_from, base_env_from, gh_env_from, git_env_from, has_no_git_vars};
use gtasks::gh::{git, run_pipe};

fn parent(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn keys(env: &[(OsString, OsString)]) -> Vec<String> {
    env.iter()
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .collect()
}

fn value(env: &[(OsString, OsString)], key: &str) -> Option<String> {
    env.iter()
        .find_map(|(k, v)| (k.to_string_lossy() == key).then(|| v.to_string_lossy().into_owned()))
}

fn has(env: &[(OsString, OsString)], key: &str) -> bool {
    keys(env).iter().any(|k| k == key)
}

/// No child ever sees a `GIT_*` variable from a dirty parent; the git child
/// keeps exactly one: the prompt switch it sets itself.
#[test]
fn child_env_has_no_git_vars() {
    let dirty = parent(&[
        ("PATH", "/usr/bin:/bin"),
        ("HOME", "/home/u"),
        ("GIT_DIR", "/elsewhere"),
        ("GIT_WORK_TREE", "/elsewhere"),
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "safe.directory"),
        ("GIT_CONFIG_VALUE_0", "/injected"),
    ]);
    for env in [
        base_env_from(&dirty),
        gh_env_from(&dirty),
        agent_env_from(&dirty),
    ] {
        assert!(
            has_no_git_vars(&env),
            "GIT_* leaked: {keys:?}",
            keys = keys(&env)
        );
    }
    // The git child sets exactly one GIT_* itself: the prompt switch.
    let git_keys: Vec<String> = keys(&git_env_from(&dirty))
        .into_iter()
        .filter(|k| k.starts_with("GIT_"))
        .collect();
    assert_eq!(git_keys, ["GIT_TERMINAL_PROMPT"]);
}

/// The agent never receives a GitHub token, even from a dirty parent.
#[test]
fn agent_env_has_no_gh_token() {
    let dirty = parent(&[
        ("GH_TOKEN", "s3cr3t"),
        ("GITHUB_TOKEN", "s3cr3t-2"),
        ("GIT_DIR", "/x"),
    ]);
    let env = agent_env_from(&dirty);
    assert!(!has(&env, "GH_TOKEN") && !has(&env, "GITHUB_TOKEN"));
    assert!(has_no_git_vars(&env));
}

/// `gh` gets its non-interactive knobs and the token passthrough.
#[test]
fn gh_env_knobs_present() {
    let env = gh_env_from(&parent(&[("PATH", "/usr/bin")]));
    for knob in [
        "GH_PROMPT_DISABLED",
        "GH_NO_UPDATE_NOTIFIER",
        "GH_PAGER",
        "NO_COLOR",
    ] {
        assert!(has(&env, knob), "{knob} must be set");
    }
    assert_eq!(value(&env, "GH_PAGER").as_deref(), Some("cat"));
    let with_token = gh_env_from(&parent(&[("GH_TOKEN", "t")]));
    assert_eq!(value(&with_token, "GH_TOKEN").as_deref(), Some("t"));
    assert!(has_no_git_vars(&env));
}

/// The interactive child gets the agent allowlist too, not the parent's
/// environment: it works inside an untrusted worktree, where a planted
/// instruction reading `GH_TOKEN` out of the environment is the simplest
/// exfiltration there is (F2). The test spawns exactly what the handoff
/// spawns — same builder — and reads what the child really saw, because
/// `Command` exposes no environment getter.
#[test]
fn interactive_child_gets_no_tokens() {
    use gtasks::gh::{Argv, CheckedArgv, interactive_command};
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let dump = tmp.path().join("child-env.txt");
    let script = tmp.path().join("dump-env.sh");
    std::fs::write(&script, format!("#!/bin/sh\nenv > {}\n", dump.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();

    let dirty = parent(&[
        ("PATH", "/usr/bin:/bin"),
        ("HOME", tmp.path().to_str().unwrap()),
        ("TERM", "xterm-256color"),
        ("GH_TOKEN", "s3cr3t"),
        ("GITHUB_TOKEN", "s3cr3t-2"),
        ("GIT_DIR", "/x"),
    ]);
    let argv = CheckedArgv::unchecked_for_tests(Argv {
        program: script.clone(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: tmp.path().to_path_buf(),
    });
    // `output()` instead of `spawn().wait()`: a failure here has to be diagnosable. This test
    // failed once, on the first run after a merge, and the message said nothing — no status, no
    // stderr, no dump — so the only reading left was "it passes now". A test that fails sometimes
    // is worse than one that always fails: it teaches re-running instead of looking. Now the
    // panic carries the status, the child's stderr and the environment that landed on disk.
    let out = interactive_command(&script, &argv, &dirty)
        .output()
        .expect("the dump script runs");
    let text = std::fs::read_to_string(&dump)
        .unwrap_or_else(|err| format!("<the child dumped nothing: {err}>"));
    assert!(
        out.status.success(),
        "the dump script must exit 0, got {:?}, stderr: {:?}, dump-so-far: {text:?}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let child_env: Vec<(OsString, OsString)> = text
        .lines()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        })
        .collect();
    assert!(
        !child_env
            .iter()
            .any(|(k, _)| k == "GH_TOKEN" || k == "GITHUB_TOKEN"),
        "no token reaches the interactive child: {text}"
    );
    assert!(
        gtasks::env::has_no_git_vars(&child_env),
        "no GIT_* reaches the interactive child: {text}"
    );
    assert!(
        child_env.iter().any(|(k, _)| k == "PATH"),
        "the allowlist still carries the basics: {text}"
    );
}

/// `GIT_CONFIG_COUNT` injection is stripped by the wrapper env.
/// Control (no wrapper): the same injection visibly alters real git, so the
/// stripped assertion cannot pass vacuously.
#[test]
fn git_config_count_is_stripped() {
    let git_bin = binaries::resolve("git").expect("test needs a real git");
    let tmp = tempfile::tempdir().unwrap();

    // Control: WITHOUT the wrapper the planted config is honoured.
    let out = std::process::Command::new(&git_bin)
        .arg("config")
        .arg("--list")
        .current_dir(tmp.path())
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "safe.directory")
        .env("GIT_CONFIG_VALUE_0", "/tmp/injected-control")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .output()
        .expect("spawn git");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("safe.directory=/tmp/injected-control"),
        "control: without the wrapper the injection must work, got: {text}"
    );

    // Unit: the wrapper env drops every GIT_CONFIG_* variable.
    let dirty = parent(&[
        ("PATH", "/usr/bin:/bin"),
        ("HOME", tmp.path().to_str().unwrap()),
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "safe.directory"),
        ("GIT_CONFIG_VALUE_0", "/tmp/injected-control"),
    ]);
    let env = git_env_from(&dirty);
    assert!(
        !keys(&env).iter().any(|k| k.starts_with("GIT_CONFIG")),
        "GIT_CONFIG_* must be stripped: {keys:?}",
        keys = keys(&env)
    );

    // End to end: the same command through the wrapper ignores the injection.
    let binaries = Binaries::new(PathBuf::from("/bin/false"), git_bin.clone());
    let mut argv = git(
        &binaries,
        tmp.path(),
        vec!["config".into(), "--list".into()],
    );
    argv.env = env;
    let out = run_pipe(&argv, std::time::Duration::from_secs(10)).expect("wrapped git runs");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("/tmp/injected-control"),
        "wrapped git must ignore the injection, got: {text}"
    );
}

/// A planted `GIT_DIR` must not hijack the `-c` flags.
/// Control (no wrapper): `rev-parse` answers from the planted dir.
#[test]
fn git_dir_does_not_hijack_c_flag() {
    let git_bin = binaries::resolve("git").expect("test needs a real git");
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();

    let init = std::process::Command::new(&git_bin)
        .args(["init", "-q"])
        .arg(&repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .output()
        .expect("spawn git init");
    assert!(init.status.success(), "git init must succeed");

    let init_elsewhere = std::process::Command::new(&git_bin)
        .args(["init", "-q"])
        .arg(&elsewhere)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .output()
        .expect("spawn git init");
    assert!(
        init_elsewhere.status.success(),
        "git init elsewhere must succeed"
    );

    // Control: WITHOUT the wrapper the planted GIT_DIR wins — git answers
    // from the other repo's git dir.
    let out = std::process::Command::new(&git_bin)
        .args(["rev-parse", "--absolute-git-dir"])
        .current_dir(&repo)
        .env("GIT_DIR", elsewhere.join(".git"))
        .output()
        .expect("spawn git");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        PathBuf::from(text),
        elsewhere.join(".git").canonicalize().unwrap(),
        "control: without the wrapper GIT_DIR must hijack"
    );

    // Wrapped: the wrapper env drops GIT_DIR, so git answers from the repo.
    let dirty = parent(&[
        ("PATH", "/usr/bin:/bin"),
        ("HOME", tmp.path().to_str().unwrap()),
        ("GIT_DIR", elsewhere.to_str().unwrap()),
    ]);
    let binaries = Binaries::new(PathBuf::from("/bin/false"), git_bin.clone());
    let mut argv = git(
        &binaries,
        &repo,
        vec!["rev-parse".into(), "--absolute-git-dir".into()],
    );
    argv.env = git_env_from(&dirty);
    let out = run_pipe(&argv, std::time::Duration::from_secs(10)).expect("wrapped git runs");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        PathBuf::from(text),
        repo.join(".git").canonicalize().unwrap(),
        "wrapped git must answer from the repo, not the planted GIT_DIR"
    );
}
