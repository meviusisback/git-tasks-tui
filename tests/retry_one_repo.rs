//! The last step of B-1: a **transient** failure on one repo must not lose the repo.
//!
//! Measured on `61ac1a8`: the bisection contains a failed batch, so for batches larger
//! than one the narrowing *is* the implicit retry — halves are re-queried and the healthy
//! ones come back. At `batch.len() == 1` there is no second attempt:
//!
//! ```text
//! Err(msg) if batch.len() == 1 => out.push((repo, Err(msg))),
//! ```
//!
//! and that is exactly the case measured as non-deterministic: the same query failed at
//! 10.9 s and succeeded at 6.7 s minutes later (`docs/probes.md` §"The 502 boundary").
//! A repo that lives there is lost on every unlucky start.
//!
//! Two assertions, and the second is the one that keeps the fix from becoming a loop:
//!
//!   1. a repo whose first call fails and whose second succeeds **arrives**;
//!   2. a repo whose every call fails is reported **failed once, after exactly one
//!      retry** — two calls to `gh`, not three and not `while`.
//!
//! The fake `gh` counts its own invocations in a file baked into the script: the app
//! sanitises the environment it hands to children (S16), so an env var would not survive.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use gtasks::actions::RepoSlug;
use gtasks::binaries::Binaries;
use gtasks::fetch;

const FLAKY: &str = "flaky/repo";

/// A fake `gh` that fails its first `fail_first` calls and answers validly afterwards.
/// Every call appends a line to `count_path`, which is baked in — not passed by env.
fn fake_gh(dir: &Path, fail_first: usize) -> (PathBuf, PathBuf) {
    let path = dir.join("gh");
    let count = dir.join("calls.log");
    let script = format!(
        r#"#!/usr/bin/env python3
import json, re, sys
count_path = "{count}"
try:
    with open(count_path) as fh:
        calls = len(fh.read().splitlines())
except FileNotFoundError:
    calls = 0
with open(count_path, "a") as fh:
    fh.write("call\n")

if calls < {fail_first}:
    sys.stderr.write("gh: HTTP 502\n")
    sys.exit(1)

argv = sys.argv[1:]
query = ""
for i, a in enumerate(argv):
    if a.startswith("query="):
        query = a[len("query="):]
    elif a == "--field" and i + 1 < len(argv) and argv[i + 1].startswith("query="):
        query = argv[i + 1][len("query="):]
pairs = re.findall(r'repository\(owner: "([^"]+)", name: "([^"]+)"\)', query)
data = {{}}
for i, (owner, name) in enumerate(pairs):
    data[f"r{{i}}"] = {{
        "pullRequests": {{"nodes": [{{
            "number": 7, "title": "t-" + name, "state": "OPEN",
            "url": "https://example.invalid/" + name + "/7",
        }}]}},
        "issues": {{"nodes": []}},
    }}
print(json.dumps({{"data": data}}))
"#,
        count = count.display(),
        fail_first = fail_first
    );
    std::fs::write(&path, script).expect("write fake gh");
    let mut perms = std::fs::metadata(&path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    std::fs::set_permissions(&path, perms).expect("chmod");
    (path, count)
}

fn git_binary() -> PathBuf {
    String::from_utf8(
        Command::new("sh")
            .arg("-c")
            .arg("command -v git")
            .output()
            .expect("sh")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .into()
}

fn calls(count: &Path) -> usize {
    std::fs::read_to_string(count)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn one_repo() -> Vec<RepoSlug> {
    vec![RepoSlug::parse(FLAKY).expect("slug")]
}

#[test]
fn a_transient_failure_is_retried_once() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (gh, count) = fake_gh(tmp.path(), 1);
    let binaries = Binaries::new(gh, git_binary());

    let outcome = fetch::fetch_graphql_many(&binaries, &one_repo(), 10, Duration::from_secs(30), 4);

    println!(
        "transient 502: {} items · {} failures · {} calls to gh",
        outcome.items.len(),
        outcome.failures.len(),
        calls(&count)
    );

    assert!(
        outcome.failures.is_empty(),
        "the first call failed and the second would have succeeded: the repo must arrive, \
         not be reported failed ({:?})",
        outcome
            .failures
            .iter()
            .map(|f| f.0.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        outcome.items.len(),
        1,
        "the retried call returned one pull request for one repo"
    );
}

#[test]
fn a_persistent_failure_is_reported_once_after_one_retry() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (gh, count) = fake_gh(tmp.path(), usize::MAX / 2); // always fails
    let binaries = Binaries::new(gh, git_binary());

    let outcome = fetch::fetch_graphql_many(&binaries, &one_repo(), 10, Duration::from_secs(30), 4);

    println!(
        "persistent 502: {} items · {} failures · {} calls to gh",
        outcome.items.len(),
        outcome.failures.len(),
        calls(&count)
    );

    assert_eq!(
        outcome.failures.len(),
        1,
        "one repo failed, so one failure is reported: {:?}",
        outcome
            .failures
            .iter()
            .map(|f| f.0.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(outcome.failures[0].0, FLAKY);
    assert_eq!(
        calls(&count),
        2,
        "a retry is one extra attempt, not a loop: expected 2 calls to gh"
    );
}
