//! Task 13b: one GraphQL call per batch of repos. These tests pin the grouping
//! (pure: how repos become batches and aliases) and the parsing (fixtures,
//! never the network) — including that a null alias fails one repo, not the
//! batch.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gtasks::actions::RepoSlug;
use gtasks::gh::{CheckedArgv, gh};
use gtasks::graphql::{BATCH_SIZE, assemble, batch_argv, batch_query};
use gtasks::model::ItemKind;

fn slugs(names: &[&str]) -> Vec<RepoSlug> {
    names
        .iter()
        .map(|s| RepoSlug::parse(s).expect("valid slug"))
        .collect()
}

#[test]
fn sixty_nine_repos_become_four_batches_with_stable_aliases() {
    // The grouping contract: 69 repos → 20 + 20 + 20 + 9, alias r{i} is
    // always batch[i], every repo appears exactly once.
    let names: Vec<String> = (0..69).map(|n| format!("owner{n}/repo{n}")).collect();
    let all: Vec<&str> = names.iter().map(String::as_str).collect();
    let repos = slugs(&all);
    let chunks: Vec<&[RepoSlug]> = repos.chunks(BATCH_SIZE).collect();
    assert_eq!(
        chunks.iter().map(|c| c.len()).collect::<Vec<_>>(),
        vec![20, 20, 20, 9],
        "69 repos ride in four calls"
    );
    let mut seen = std::collections::HashSet::new();
    for (batch_no, chunk) in chunks.iter().enumerate() {
        let query = batch_query(chunk, 100);
        for (i, slug) in chunk.iter().enumerate() {
            let (owner, name) = slug.as_str().split_once('/').unwrap();
            assert!(
                query.contains(&format!(
                    "r{i}: repository(owner: \"{owner}\", name: \"{name}\")"
                )),
                "batch {batch_no}: alias r{i} is {slug}"
            );
            assert!(seen.insert(slug.as_str()), "each repo rides once");
        }
        assert!(
            !query.contains(&format!("r{}:", chunk.len())),
            "batch {batch_no}: no alias past the batch"
        );
    }
    assert_eq!(seen.len(), 69);
}

#[test]
fn braces_balance_for_any_batch_size() {
    // The builder concatenates literals; a missing brace fails every repo in
    // the batch at once. Count, don't eyeball.
    for n in [1, 7, 20] {
        let names: Vec<String> = (0..n).map(|i| format!("o{i}/r{i}")).collect();
        let all: Vec<&str> = names.iter().map(String::as_str).collect();
        let query = batch_query(&slugs(&all), 100);
        let open = query.chars().filter(|&c| c == '{').count();
        let close = query.chars().filter(|&c| c == '}').count();
        assert_eq!(open, close, "unbalanced for batch size {n}");
    }
}

#[test]
fn the_batch_argv_is_one_field_never_a_short_flag() {
    // `--field`, not `-f`: the builder path takes no short flags, and the
    // query rides as one argument — no shell anywhere near it.
    let repos = slugs(&["acme/one", "acme/two"]);
    let args = batch_argv(batch_query(&repos, 5));
    assert_eq!(args[0], "api");
    assert_eq!(args[1], "graphql");
    assert_eq!(args[2], "--field");
    assert!(
        args[3].to_string_lossy().starts_with("query=query {"),
        "the query is a value, not a command"
    );
    assert!(
        args.iter().all(|a| a != "-f"),
        "no short flag on the builder path"
    );
}

const BATCH_JSON: &str = r#"{"data":{
"r0":{"pullRequests":{"nodes":[{"number":9,"title":"T","state":"OPEN",
"url":"https://github.com/o/a/pull/9","createdAt":"2026-10-25T00:30:59Z",
"updatedAt":"2026-10-25T01:00:00Z","isDraft":false,"author":{"login":"meviusisback"},
"labels":{"nodes":[{"name":"bug"}]},"assignees":{"nodes":[{"login":"bob"}]},
"headRefName":"feat/x","baseRefName":"main","reviewDecision":null,"mergeable":"MERGEABLE",
"closingIssuesReferences":{"nodes":[{"number":5,"title":"linked"}]},
"commits":{"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"nodes":[
{"__typename":"CheckRun","name":"ci","conclusion":"FAILURE","status":"COMPLETED"},
{"__typename":"StatusContext","context":"docs","state":"SUCCESS"}]}}}}]}}]},
"issues":{"nodes":[{"number":1,"title":"I","state":"OPEN",
"url":"https://github.com/o/a/issues/1","createdAt":"2026-10-24T00:00:00Z",
"updatedAt":"2026-10-24T01:00:00Z","author":null,
"labels":{"nodes":[]},"assignees":{"nodes":[]}}]}},
"r1":null}}"#;

#[test]
fn a_good_batch_parses_and_stamps_origin() {
    let repos = slugs(&["o/a", "o/b"]);
    let reads = assemble(&repos, BATCH_JSON).unwrap();
    assert_eq!(reads.len(), 2);
    // r0: one PR plus one issue, origin stamped by the assembler.
    let pr = reads[0]
        .items
        .iter()
        .find(|i| i.kind == ItemKind::Pr)
        .unwrap();
    assert_eq!(pr.number, 9);
    assert_eq!(pr.repo, "o/a", "origin comes from the batch order");
    assert_eq!(pr.author.as_ref().unwrap().login, "meviusisback");
    assert_eq!(pr.labels[0].name, "bug");
    assert_eq!(pr.assignees[0].login, "bob");
    assert_eq!(pr.head_ref_name.as_deref(), Some("feat/x"));
    assert_eq!(pr.base_ref_name.as_deref(), Some("main"));
    assert_eq!(pr.mergeable.as_deref(), Some("MERGEABLE"));
    assert_eq!(pr.closing_issues_references[0].number, Some(5));
    // A CheckRun and a StatusContext side by side, mapped to one shape.
    let checks = &pr.status_check_rollup;
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name.as_deref(), Some("ci"));
    assert_eq!(checks[0].conclusion.as_deref(), Some("FAILURE"));
    assert_eq!(checks[1].name.as_deref(), Some("docs"));
    assert_eq!(checks[1].conclusion.as_deref(), Some("SUCCESS"));
    let issue = reads[0]
        .items
        .iter()
        .find(|i| i.kind == ItemKind::Issue)
        .unwrap();
    assert_eq!(issue.number, 1);
    assert_eq!(issue.repo, "o/a");
    // r1 is null: that repo's failure, not the batch's.
    assert!(reads[0].error.is_none());
    assert_eq!(reads[1].items.len(), 0);
    let err = reads[1].error.as_deref().unwrap();
    assert!(err.contains("r1"), "the failure names the alias: {err}");
    assert_eq!(reads[1].repo, "o/b");
}

#[test]
fn malformed_json_is_an_error_not_a_panic() {
    let repos = slugs(&["o/a"]);
    let err = assemble(&repos, "not json at all").unwrap_err();
    assert!(err.to_string().contains("GraphQL"), "got {err}");
}

/// A fake `gh` that prints `body` and exits 0, so the real spawn path is used.
fn fake_gh(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("gh");
    std::fs::write(&path, format!("#!/bin/sh\ncat <<'JSON'\n{body}\nJSON\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn bins(program: PathBuf) -> gtasks::binaries::Binaries {
    gtasks::binaries::Binaries::new(program, PathBuf::from("/bin/false"))
}

#[test]
fn a_batch_rides_the_checked_transport_end_to_end() {
    // The full argv shape (`api graphql --field query=…`) through a real
    // spawn: the guard refuses `gh api` on the untrusted path, so the batch
    // must travel checked — this is the proof that it does.
    let dir = tempfile::tempdir().unwrap();
    let program = fake_gh(dir.path(), BATCH_JSON);
    let binaries = bins(program);
    let repos = slugs(&["o/a"]);
    let argv = gh(&binaries, dir.path(), batch_argv(batch_query(&repos, 100)));
    let checked = CheckedArgv::unchecked_for_tests(argv);
    let output = gtasks::gh::run_checked(&checked, Duration::from_secs(10)).unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let reads = assemble(&repos, &text).unwrap();
    assert_eq!(reads[0].items.len(), 2, "one PR plus one issue");
    assert!(reads[0].error.is_none());
}

#[test]
fn a_failing_gh_reports_the_exit_status_and_stderr() {
    // The auditor's 401 probe, moved to the batch path: a failure stays a
    // failed spawn carrying stderr (which can contain a token — the log
    // layer sanitises it, L-1).
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("gh");
    std::fs::write(
        &program,
        "#!/bin/sh\necho 'HTTP 401: Bad credentials' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let binaries = bins(program);
    let repos = slugs(&["o/a"]);
    let argv = gh(&binaries, dir.path(), batch_argv(batch_query(&repos, 100)));
    let checked = CheckedArgv::unchecked_for_tests(argv);
    let output = gtasks::gh::run_checked(&checked, Duration::from_secs(10)).unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("401"), "got {stderr}");
    let logged = gtasks::sanitize::sanitize(&stderr);
    assert!(!logged.contains("gho_"), "got {logged}");
}
