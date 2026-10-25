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

/// How many items to ask per repo list.
pub const DEFAULT_LIMIT: usize = 100;

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
) -> FetchOutcome {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let chunks: Vec<&[RepoSlug]> = repos.chunks(crate::graphql::BATCH_SIZE).collect();
    let parallel = parallel.clamp(1, MAX_PARALLEL).min(chunks.len().max(1));
    let next = AtomicUsize::new(0);
    let collected: Mutex<Vec<BatchResult>> = Mutex::new(Vec::new());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));

    std::thread::scope(|scope| {
        for _ in 0..parallel {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    if index >= chunks.len() {
                        break;
                    }
                    let batch = chunks[index];
                    let query = crate::graphql::batch_query(batch, limit);
                    // `gh()` takes the resolved binary: no PATH lookup, nothing
                    // to overwrite. One `--field`, never `-f`; one argument per
                    // word — no shell anywhere near the query text.
                    let argv = gh(binaries, &cwd, crate::graphql::batch_argv(query));
                    let checked = CheckedArgv::from_checked(argv);
                    let reads = match gh::run_checked(&checked, timeout) {
                        Ok(output) => {
                            if !output.status.success() {
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                let msg = format!("{}: {}", output.status, stderr.trim());
                                (Vec::new(), Some(format!("batch {index}: gh failed: {msg}")))
                            } else if output.truncated {
                                (
                                    Vec::new(),
                                    Some(format!("batch {index}: output capped, refusing partial")),
                                )
                            } else {
                                let text = String::from_utf8_lossy(&output.stdout);
                                match crate::graphql::assemble(batch, &text) {
                                    Ok(reads) => (reads, None),
                                    Err(source) => (
                                        Vec::new(),
                                        Some(format!("batch {index}: cannot parse: {source:#}")),
                                    ),
                                }
                            }
                        }
                        Err(err) => {
                            if err.downcast_ref::<gh::Timeout>().is_some() {
                                (Vec::new(), Some(format!("batch {index}: gh timed out")))
                            } else {
                                (
                                    Vec::new(),
                                    Some(format!("batch {index}: gh failed: {err:#}")),
                                )
                            }
                        }
                    };
                    let mut guard = match collected.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    guard.push((index, reads.0, reads.1));
                }
            });
        }
    });

    let mut collected = match collected.into_inner() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    };
    collected.sort_by_key(|(index, _, _)| *index);

    let mut outcome = FetchOutcome::default();
    for (index, reads, batch_err) in collected {
        if let Some(msg) = batch_err {
            // The whole batch failed: every repo in it truly yielded nothing.
            for slug in chunks[index] {
                outcome
                    .failures
                    .push((slug.as_str().to_string(), msg.clone()));
            }
            continue;
        }
        for read in reads {
            if let Some(err) = read.error {
                outcome.failures.push((read.repo, err));
            } else {
                outcome.items.extend(read.items);
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

/// One batch's result, kept with its index so the merge can restore order.
type BatchResult = (usize, Vec<crate::graphql::RepoRead>, Option<String>);

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
