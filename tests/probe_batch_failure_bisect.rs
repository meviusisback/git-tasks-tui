//! B-1 — a batch that fails to run must not take its healthy repos with it.
//!
//! Measured on `4597e0c`: `rust-lang/rust` + `kubernetes/kubernetes` + `torvalds/linux`
//! at `limit = 100` made GitHub answer `HTTP 502` to the whole batch, and the screen
//! said `0 items from 3 repos · 3 failed` — a true count of the **wrong partition**:
//! it counts the repos in the failed batch, not the repos that actually failed.
//!
//! The fix this test defines: on a failed batch, **bisect** until the failure belongs
//! to one repo. Three assertions, and the last two are the ones nobody writes:
//!
//!   1. the healthy repos arrive at all (the failure is contained);
//!   2. exactly **one** repo is reported as failed — not twenty;
//!   3. the healthy repo that shared the batch with the failing one is **present**:
//!      bisection must not lose pieces while it narrows.
//!
//! The fake `gh` never answers on its own: it refuses the query when the batch
//! contains the marked repo (the way GitHub refused it with a 502) and returns a valid
//! payload otherwise, so a bisecting implementation can succeed on the halves.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use gtasks::actions::RepoSlug;
use gtasks::binaries::Binaries;
use gtasks::fetch;

const HEAVY: &str = "heavy/boom";

/// A fake `gh` that fails any query mentioning [`HEAVY`] and answers validly otherwise.
fn fake_gh(dir: &Path) -> PathBuf {
    let path = dir.join("gh");
    let script = format!(
        r#"#!/usr/bin/env python3
import json, re, sys
argv = sys.argv[1:]
query = ""
for i, a in enumerate(argv):
    if a.startswith("query="):
        query = a[len("query="):]
    elif a == "--field" and i + 1 < len(argv) and argv[i + 1].startswith("query="):
        query = argv[i + 1][len("query="):]
if '{heavy_pair}' in query:
    sys.stderr.write("gh: HTTP 502\n")
    sys.exit(1)
# One alias per repository(...) in the order the query declares them.
pairs = re.findall(r'repository\(owner: "([^"]+)", name: "([^"]+)"\)', query)
data = {{}}
for i, (owner, name) in enumerate(pairs):
    data[f"r{{i}}"] = {{
        "pullRequests": {{"nodes": [{{
            "number": 1, "title": "t-" + name, "state": "OPEN",
            "url": "https://example.invalid/" + name + "/1",
        }}]}},
        "issues": {{"nodes": []}},
    }}
print(json.dumps({{"data": data}}))
"#,
        heavy_pair = "owner: \"heavy\", name: \"boom\""
    );
    std::fs::write(&path, script).expect("write fake gh");
    let mut perms = std::fs::metadata(&path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
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

fn repos(n: usize) -> Vec<RepoSlug> {
    let mut out: Vec<RepoSlug> = Vec::new();
    for i in 0..n {
        // The heavy one sits in the middle of the first batch, so a bisection has to
        // find it instead of guessing a side.
        if i == 5 {
            out.push(RepoSlug::parse(HEAVY).expect("slug"));
        } else {
            out.push(RepoSlug::parse(&format!("ok/repo{i}")).expect("slug"));
        }
    }
    out
}

#[test]
fn one_heavy_repo_does_not_fail_the_whole_batch() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path()), git_binary());
    let repos = repos(20);

    let outcome = fetch::fetch_graphql_many(&binaries, &repos, 10, Duration::from_secs(30), 4);

    let arrived: Vec<String> = outcome.items.iter().map(|i| i.repo.clone()).collect();
    println!(
        "arrived: {} items over {} repos · failed: {}",
        arrived.len(),
        arrived
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        outcome.failures.len()
    );

    // 1. the healthy repos arrive at all
    assert!(
        !outcome.items.is_empty(),
        "a failure in one batch swallowed every repo: {} failures, no items",
        outcome.failures.len()
    );

    // 2. exactly one repo is reported failed — not the size of the batch
    assert_eq!(
        outcome.failures.len(),
        1,
        "the failure must belong to one repo, not to the batch: {:?}",
        outcome
            .failures
            .iter()
            .map(|f| f.0.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(outcome.failures[0].0, HEAVY);

    // 3. the healthy repo that shared the batch with the failing one is present
    for neighbour in ["ok/repo4", "ok/repo6"] {
        assert!(
            arrived.iter().any(|r| r == neighbour),
            "{neighbour} sat in the same batch as {HEAVY} and was lost by the narrow-down"
        );
    }
}

// Anti-vacuity hinge: the fake must be able to succeed. Without this, the test above
// would go green as soon as someone made the code divide failures by 20, or as soon as
// the fake refused everything for an unrelated reason — and nothing would notice.
#[test]
fn the_same_fixture_succeeds_without_the_heavy_repo() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path()), git_binary());
    let mut repos = repos(20);
    repos.retain(|r| r.as_str() != HEAVY);

    let outcome = fetch::fetch_graphql_many(&binaries, &repos, 10, Duration::from_secs(30), 4);

    println!(
        "without the heavy repo: {} items over {} repos · failed: {}",
        outcome.items.len(),
        outcome
            .items
            .iter()
            .map(|i| i.repo.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        outcome.failures.len()
    );
    assert!(
        outcome.failures.is_empty(),
        "the fake refused a healthy batch: {:?}",
        outcome.failures
    );
    assert_eq!(
        outcome
            .items
            .iter()
            .map(|i| i.repo.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        repos.len(),
        "every healthy repo must arrive when nothing fails"
    );
}
