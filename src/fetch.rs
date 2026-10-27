//! Task 13b: fetch every repo in GraphQL batches through the controlled wrapper.
//!
//! One call per [`BATCH_SIZE`](crate::graphql::BATCH_SIZE) repos replaces two
//! REST calls per repo (69 repos: 138 calls → 4 calls). The query text is
//! generated, never configured: a configurable query would be an injection
//! point. Everything the payload says about origin is discarded — `repo` is
//! set here, by the assembler, from the batch order (S4).

use std::ffi::OsString;
use std::time::Duration;

use crate::actions::RepoSlug;
use crate::binaries::Binaries;
use crate::gh::{self, Argv, Caps, CheckedArgv, gh};
use crate::model::{Item, ItemKind};

/// The repos behind the `R` menu, re-read from GitHub when the menu opens:
/// repos created outside the app become pickable without a restart. One
/// `gh repo list`, parsed strictly — invalid slugs are skipped, never
/// trusted. A capped (partial) payload is refused, like every other fetch.
pub fn fetch_repo_list(binaries: &Binaries, timeout: Duration) -> Result<Vec<RepoSlug>, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    let argv = gh(
        binaries,
        &cwd,
        vec![
            "repo".into(),
            "list".into(),
            "--limit".into(),
            "100".into(),
            "--json".into(),
            "nameWithOwner".into(),
        ],
    );
    let checked = CheckedArgv::from_checked(argv);
    let output = gh::run_checked(&checked, timeout).map_err(|err| format!("gh failed: {err:#}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{}: {}", output.status, stderr.trim()));
    }
    if output.truncated {
        return Err("output capped, refusing partial".to_string());
    }
    Ok(parse_repo_list(&String::from_utf8_lossy(&output.stdout)))
}

/// Fetch one issue/PR by number, open or closed, for a tab the OPEN-only
/// fetch never saw: closed work with a live tab resolves here instead of
/// falling back to whatever sits under the cursor. Issues are tried
/// first (the common tabbed-closed case), then PRs; anything else is
/// the entry's failure, not the tab's.
pub fn fetch_closed_item(
    binaries: &Binaries,
    repo: &str,
    number: u64,
    timeout: Duration,
) -> Result<Item, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    for kind in [ItemKind::Issue, ItemKind::Pr] {
        let subcommand = match kind {
            ItemKind::Issue => "issue",
            ItemKind::Pr => "pr",
        };
        // Issues have no head/base branches, reviews or drafts: asking
        // for them errors the whole call, so each kind asks its own list.
        let fields = match kind {
            ItemKind::Issue => "number,title,state,url,author,labels,assignees,createdAt,updatedAt",
            ItemKind::Pr => {
                "number,title,state,url,author,labels,assignees,createdAt,updatedAt,headRefName,baseRefName,reviewDecision,mergeable,isDraft"
            }
        };
        let argv = gh(
            binaries,
            &cwd,
            vec![
                subcommand.into(),
                "view".into(),
                number.to_string().into(),
                "--repo".into(),
                repo.into(),
                "--json".into(),
                fields.into(),
            ],
        );
        let checked = CheckedArgv::from_checked(argv);
        match gh::run_checked(&checked, timeout) {
            Ok(output) if output.status.success() && !output.truncated => {
                let text = String::from_utf8_lossy(&output.stdout);
                if let Ok(item) = parse_closed_item(kind, repo, &text) {
                    return Ok(item);
                }
            }
            _ => {}
        }
    }
    Err(format!("{repo}#{number}: neither issue nor PR found"))
}

/// Parse one `gh (issue|pr) view --json`: the entry's own shape decides,
/// so a wrong-typed payload is `Err` and the caller tries the other kind.
fn parse_closed_item(kind: ItemKind, repo: &str, text: &str) -> Result<Item, String> {
    use crate::model::{Author, Label};
    let entry: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("cannot parse: {err:#}"))?;
    let number = entry
        .get("number")
        .and_then(|n| n.as_u64())
        .ok_or_else(|| "no number in entry".to_string())?;
    let str_field = |name: &str| entry.get(name).and_then(|v| v.as_str()).map(str::to_string);
    Ok(Item {
        number,
        title: str_field("title").unwrap_or_default(),
        state: str_field("state").unwrap_or_else(|| "UNKNOWN".to_string()),
        is_draft: entry.get("isDraft").and_then(|v| v.as_bool()),
        author: entry
            .get("author")
            .and_then(|a| a.get("login"))
            .and_then(|l| l.as_str())
            .map(|login| Author {
                login: login.to_string(),
            }),
        labels: entry
            .get("labels")
            .and_then(|l| l.as_array())
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| n.get("name")?.as_str())
                    .map(|name| Label {
                        name: name.to_string(),
                        color: None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        assignees: entry
            .get("assignees")
            .and_then(|a| a.as_array())
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| n.get("login")?.as_str())
                    .map(|login| Author {
                        login: login.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        updated_at: str_field("updatedAt").unwrap_or_default(),
        created_at: str_field("createdAt").unwrap_or_default(),
        url: str_field("url").unwrap_or_default(),
        head_ref_name: str_field("headRefName"),
        base_ref_name: str_field("baseRefName"),
        review_decision: str_field("reviewDecision"),
        mergeable: str_field("mergeable"),
        status_check_rollup: Vec::new(),
        closing_issues_references: Vec::new(),
        repo: repo.to_string(),
        kind,
    })
}

/// Parse `gh repo list --json nameWithOwner`: an array of
/// `{"nameWithOwner": "owner/repo"}`. A wrong-typed entry is that entry's
/// failure, not the list's: the rest still parses.
fn parse_repo_list(text: &str) -> Vec<RepoSlug> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(text).unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| entry.get("nameWithOwner")?.as_str())
        .filter_map(|slug| RepoSlug::parse(slug).ok())
        .collect()
}

/// How many items to ask per repo list: the gateway drops calls that run
/// past ~10 s, and past 30 the limit buys timeouts instead of items.
pub const DEFAULT_LIMIT: usize = 30;

/// Concurrent fetch workers, ceiling included. Batches are heavy GraphQL
/// reads and GitHub throttles abusive concurrency with secondary rate
/// limits — eight at a time is the documented compromise, not a guess.
/// The config load rejects anything outside 1..=8, so the executor never
/// silently downgrades a value the user set: fewer threads than chunks is
/// arithmetic (one thread per chunk at most), not policy.
pub const MAX_PARALLEL: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("gh timed out")]
    Timeout,
    #[error("gh output was capped: refusing to parse a partial payload")]
    OutputCap,
    #[error("{repo}: gh failed: {source}")]
    Run { repo: String, source: anyhow::Error },
    #[error("{repo}: cannot parse gh output: {source}")]
    Parse { repo: String, source: anyhow::Error },
}

/// One attempt of one batch: transport plus parse. Reads are idempotent, so
/// a failed attempt is retried once before it counts — most gateway hiccups
/// (the nondeterministic 502s) become latency instead of lost data.
fn run_batch(
    binaries: &Binaries,
    batch: &[RepoSlug],
    limit: usize,
    timeout: Duration,
    cwd: &std::path::Path,
    include_closed: bool,
) -> Result<Vec<crate::graphql::RepoRead>, String> {
    // `gh()` takes the resolved binary: no PATH lookup, nothing to overwrite.
    // One `--field`, never `-f`; one argument per word — no shell anywhere
    // near the query text.
    let query = crate::graphql::batch_query(batch, limit, include_closed);
    let argv = gh(binaries, cwd, crate::graphql::batch_argv(query));
    let checked = CheckedArgv::from_checked(argv);
    let mut attempts = 0;
    loop {
        attempts += 1;
        let result = match gh::run_checked(&checked, timeout) {
            Ok(output) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!("{}: {}", output.status, stderr.trim()))
                } else if output.truncated {
                    Err("output capped, refusing partial".to_string())
                } else {
                    let text = String::from_utf8_lossy(&output.stdout);
                    crate::graphql::assemble(batch, &text)
                        .map_err(|source| format!("cannot parse: {source:#}"))
                }
            }
            Err(err) => {
                if err.downcast_ref::<gh::Timeout>().is_some() {
                    Err("gh timed out".to_string())
                } else {
                    Err(format!("gh failed: {err:#}"))
                }
            }
        };
        match result {
            Ok(reads) => return Ok(reads),
            Err(_) if attempts < 2 => continue,
            Err(msg) => return Err(msg),
        }
    }
}

/// One repo's fate after a batch ran: its items, or the reason it yielded none.
type ResolvedRepo = (String, Result<Vec<Item>, String>);

/// Resolve a batch into per-repo results. A failed batch is bisected —
/// halves, quarters, down to one repo — so a heavy repo degrades to itself
/// instead of taking nineteen healthy ones with it. The happy path runs one
/// query and never touches this recursion.
fn resolve_batch(
    binaries: &Binaries,
    batch: &[RepoSlug],
    limit: usize,
    timeout: Duration,
    cwd: &std::path::Path,
    include_closed: bool,
    out: &mut Vec<ResolvedRepo>,
) {
    match run_batch(binaries, batch, limit, timeout, cwd, include_closed) {
        Ok(reads) => {
            for read in reads {
                match read.error {
                    Some(err) => out.push((read.repo, Err(err))),
                    None => out.push((read.repo, Ok(read.items))),
                }
            }
        }
        Err(msg) if batch.len() == 1 => {
            out.push((batch[0].as_str().to_string(), Err(msg)));
        }
        Err(_) => {
            let mid = batch.len() / 2;
            resolve_batch(
                binaries,
                &batch[..mid],
                limit,
                timeout,
                cwd,
                include_closed,
                out,
            );
            resolve_batch(
                binaries,
                &batch[mid..],
                limit,
                timeout,
                cwd,
                include_closed,
                out,
            );
        }
    }
}

/// Fetch every repo in batches of [`BATCH_SIZE`](crate::graphql::BATCH_SIZE)
/// through the checked transport. The untrusted path refuses `gh api`
/// (it could `--method POST`); the batch is a read the query builder
/// vouches for, so it travels checked. Results are kept in **repo order**
/// regardless of which batch finished first, so the list does not jump
/// around between refreshes.
pub fn fetch_graphql_many(
    binaries: &Binaries,
    repos: &[RepoSlug],
    limit: usize,
    timeout: Duration,
    parallel: usize,
    include_closed: bool,
) -> FetchOutcome {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let chunks: Vec<&[RepoSlug]> = repos.chunks(crate::graphql::BATCH_SIZE).collect();
    let parallel = parallel.clamp(1, MAX_PARALLEL).min(chunks.len().max(1));
    let next = AtomicUsize::new(0);
    let collected: Mutex<Vec<(usize, Vec<ResolvedRepo>)>> = Mutex::new(Vec::new());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));

    std::thread::scope(|scope| {
        for _ in 0..parallel {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    if index >= chunks.len() {
                        break;
                    }
                    let mut resolved = Vec::new();
                    resolve_batch(
                        binaries,
                        chunks[index],
                        limit,
                        timeout,
                        &cwd,
                        include_closed,
                        &mut resolved,
                    );
                    let mut guard = match collected.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    guard.push((index, resolved));
                }
            });
        }
    });

    let mut collected = match collected.into_inner() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    };
    collected.sort_by_key(|(index, _)| *index);

    let mut outcome = FetchOutcome::default();
    for (_, resolved) in collected {
        for (repo, result) in resolved {
            match result {
                Ok(items) => outcome.items.extend(items),
                Err(err) => outcome.failures.push((repo, err)),
            }
        }
    }
    outcome
}

fn classify(repo: &str, err: anyhow::Error) -> FetchError {
    if err.downcast_ref::<gh::Timeout>().is_some() {
        return FetchError::Timeout;
    }
    FetchError::Run {
        repo: repo.to_owned(),
        source: err,
    }
}

/// `gh pr view <n>` / `gh issue view <n>`, argv only.
pub fn detail_args(repo: &str, kind: ItemKind, number: u64) -> Vec<OsString> {
    let subcommand = match kind {
        ItemKind::Pr => "pr",
        ItemKind::Issue => "issue",
    };
    vec![
        OsString::from(subcommand),
        OsString::from("view"),
        OsString::from(number.to_string()),
        OsString::from("--repo"),
        OsString::from(repo),
        OsString::from("--json"),
        OsString::from("body"),
    ]
}

/// `gh pr diff <n> --repo <slug>`, argv only. Diffs are pull-request-only:
/// issues have no diff, so there is no `kind` parameter to get wrong.
pub fn diff_args(repo: &str, number: u64) -> Vec<OsString> {
    vec![
        OsString::from("pr"),
        OsString::from("diff"),
        OsString::from(number.to_string()),
        OsString::from("--repo"),
        OsString::from(repo),
    ]
}

/// Fetch the unified diff of one PR as plain text (no JSON envelope to
/// parse). The caller caps the lines: a huge diff must not fill memory.
pub fn fetch_diff(
    binaries: &Binaries,
    repo: &str,
    number: u64,
    timeout: Duration,
) -> Result<String, FetchError> {
    let cwd = std::env::current_dir().map_err(|source| FetchError::Run {
        repo: repo.to_owned(),
        source: source.into(),
    })?;
    let argv = gh(binaries, &cwd, diff_args(repo, number));
    fetch_diff_with_caps(&argv, repo, timeout, Caps::default())
}

pub fn fetch_diff_with_caps(
    argv: &Argv,
    repo: &str,
    timeout: Duration,
    caps: Caps,
) -> Result<String, FetchError> {
    let output = gh::run_pipe_with_caps(argv, timeout, caps).map_err(|err| classify(repo, err))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FetchError::Run {
            repo: repo.to_owned(),
            source: anyhow::anyhow!("{}: {}", output.status, stderr.trim()),
        });
    }
    if output.truncated {
        return Err(FetchError::OutputCap);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Only the body: the list already carries the metadata, and a smaller JSON
/// keeps the output well inside `Caps`.
#[derive(serde::Deserialize)]
struct BodyPayload {
    #[serde(default)]
    body: String,
}

/// Fetch the markdown body of one item.
pub fn fetch_detail(
    binaries: &Binaries,
    repo: &str,
    kind: ItemKind,
    number: u64,
    timeout: Duration,
) -> Result<String, FetchError> {
    let cwd = std::env::current_dir().map_err(|source| FetchError::Run {
        repo: repo.to_owned(),
        source: source.into(),
    })?;
    let argv = gh(binaries, &cwd, detail_args(repo, kind, number));
    fetch_detail_with_caps(&argv, repo, timeout, Caps::default())
}

pub fn fetch_detail_with_caps(
    argv: &Argv,
    repo: &str,
    timeout: Duration,
    caps: Caps,
) -> Result<String, FetchError> {
    let output = gh::run_pipe_with_caps(argv, timeout, caps).map_err(|err| classify(repo, err))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FetchError::Run {
            repo: repo.to_owned(),
            source: anyhow::anyhow!("{}: {}", output.status, stderr.trim()),
        });
    }
    if output.truncated {
        return Err(FetchError::OutputCap);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let payload: BodyPayload = serde_json::from_str(&text).map_err(|source| FetchError::Parse {
        repo: repo.to_owned(),
        source: source.into(),
    })?;
    Ok(payload.body)
}

/// One repo's result, kept with its index so the merge can restore repo order.
type IndexedResult = (usize, Result<Vec<Item>, FetchError>);

/// Result of fetching several repos: what arrived, and what did not (with the
/// reason), so the UI can say `failed=N` instead of hiding a broken repo.
#[derive(Debug, Default)]
pub struct FetchOutcome {
    pub items: Vec<Item>,
    pub failures: Vec<(String, String)>,
}

impl FetchOutcome {
    pub fn failed(&self) -> usize {
        self.failures.len()
    }
}

/// Fetch every repo with at most `parallel` calls in flight. Results are kept
/// in **repo order** regardless of which call finished first, so the list does
/// not jump around between refreshes.
pub fn fetch_many_with<F>(repos: &[String], parallel: usize, fetch: F) -> FetchOutcome
where
    F: Fn(&str) -> Result<Vec<Item>, FetchError> + Sync,
{
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let parallel = parallel.clamp(1, MAX_PARALLEL).min(repos.len().max(1));
    let next = AtomicUsize::new(0);
    let collected: Mutex<Vec<IndexedResult>> = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..parallel {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    if index >= repos.len() {
                        break;
                    }
                    let outcome = fetch(&repos[index]);
                    let mut guard = match collected.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    guard.push((index, outcome));
                }
            });
        }
    });

    let mut collected = match collected.into_inner() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    };
    collected.sort_by_key(|(index, _)| *index);

    let mut outcome = FetchOutcome::default();
    for (index, result) in collected {
        match result {
            Ok(items) => outcome.items.extend(items),
            Err(err) => outcome
                .failures
                .push((repos[index].clone(), err.to_string())),
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::{parse_closed_item, parse_repo_list};
    use crate::model::ItemKind;

    #[test]
    fn repo_list_parses_slugs_and_skips_garbage() {
        let slugs = parse_repo_list(
            r#"[{"nameWithOwner":"me/a"},{"nameWithOwner":"me/b"},{"nameWithOwner":"nope"},{"nope":1}]"#,
        );
        let names: Vec<&str> = slugs.iter().map(|slug| slug.as_str()).collect();
        assert_eq!(names, vec!["me/a", "me/b"]);
    }

    #[test]
    fn repo_list_refuses_non_arrays() {
        assert!(parse_repo_list("{\"oops\": true}").is_empty());
        assert!(parse_repo_list("not json").is_empty());
    }

    #[test]
    fn closed_item_parses_an_issue() {
        let item = parse_closed_item(
            ItemKind::Issue,
            "o/r",
            r#"{"number":4,"title":"bug","state":"CLOSED","url":"https://github.com/o/r/issues/4","author":{"login":"me"},"labels":[{"name":"bug"}],"assignees":[]}"#,
        )
        .expect("an issue parses");
        assert_eq!(item.number, 4);
        assert_eq!(item.state, "CLOSED");
        assert_eq!(item.kind, ItemKind::Issue);
        assert_eq!(item.author.map(|a| a.login), Some("me".to_string()));
        assert_eq!(
            item.labels
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>(),
            vec!["bug"]
        );
    }

    #[test]
    fn closed_item_parses_a_pr() {
        let item = parse_closed_item(
            ItemKind::Pr,
            "o/r",
            r#"{"number":10,"title":"pr","state":"MERGED","url":"https://github.com/o/r/pull/10","headRefName":"fix","baseRefName":"main","reviewDecision":"APPROVED","mergeable":"MERGEABLE","isDraft":false}"#,
        )
        .expect("a PR parses");
        assert_eq!(item.head_ref_name.as_deref(), Some("fix"));
        assert_eq!(item.review_decision.as_deref(), Some("APPROVED"));
    }

    #[test]
    fn closed_item_refuses_garbage() {
        assert!(parse_closed_item(ItemKind::Issue, "o/r", "not json").is_err());
        assert!(parse_closed_item(ItemKind::Pr, "o/r", "{\"nope\":1}").is_err());
    }
}
