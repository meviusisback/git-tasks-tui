//! Task 7a: fetch one repo through the controlled `gh` wrapper.
//!
//! The field list is a constant in this file and never comes from
//! configuration: a configurable `--json` argument would be an argv injection
//! point. Everything the payload says about origin is discarded — `repo` and
//! `kind` are set here, by the fetcher (S4).

use std::ffi::OsString;
use std::time::Duration;

use crate::gh::{self, Argv, Caps, gh};
use crate::model::{Item, ItemKind, parse_list};

/// How many items to ask per repo.
pub const DEFAULT_LIMIT: usize = 100;

/// Fields for pull requests. Fixed here, never user-supplied.
pub const FIELDS_PR: &str = "number,title,state,updatedAt,createdAt,url,isDraft,author,labels,headRefName,reviewDecision,mergeable,statusCheckRollup";

/// Fields for issues: the PR-only ones do not exist on issues.
pub const FIELDS_ISSUE: &str = "number,title,state,updatedAt,createdAt,url,author,labels";

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

/// The argv for one repo. Values are separate `OsString`s: `--repo` with a
/// value that starts with `-` is data, not a flag.
pub fn fetch_args(repo: &str, kind: ItemKind, limit: usize) -> Vec<OsString> {
    let (subcommand, fields) = match kind {
        ItemKind::Pr => ("pr", FIELDS_PR),
        ItemKind::Issue => ("issue", FIELDS_ISSUE),
    };
    vec![
        OsString::from(subcommand),
        OsString::from("list"),
        OsString::from("--repo"),
        OsString::from(repo),
        OsString::from("--limit"),
        OsString::from(limit.to_string()),
        OsString::from("--state"),
        OsString::from("open"),
        OsString::from("--json"),
        OsString::from(fields),
    ]
}

pub fn fetch_one(
    repo: &str,
    kind: ItemKind,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<Item>, FetchError> {
    let cwd = std::env::current_dir().map_err(|source| FetchError::Run {
        repo: repo.to_owned(),
        source: source.into(),
    })?;
    // `gh()` composes the argv; resolving the program is the caller's job (S2),
    // and the spawn refuses anything that is not absolute.
    let mut argv = gh(&cwd, fetch_args(repo, kind, limit));
    argv.program = crate::binaries::resolve("gh").map_err(|source| FetchError::Run {
        repo: repo.to_owned(),
        source,
    })?;
    fetch_with_caps(&argv, repo, kind, timeout, Caps::default())
}

/// Explicit caps, so tests can trigger [`FetchError::OutputCap`] with a few bytes.
pub fn fetch_with_caps(
    argv: &Argv,
    repo: &str,
    kind: ItemKind,
    timeout: Duration,
    caps: Caps,
) -> Result<Vec<Item>, FetchError> {
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
    let mut items = parse_list(&text).map_err(|source| FetchError::Parse {
        repo: repo.to_owned(),
        source,
    })?;
    for item in &mut items {
        item.repo = repo.to_owned();
        item.kind = kind;
    }
    Ok(items)
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

    let parallel = parallel.clamp(1, 4).min(repos.len().max(1));
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

/// Fetch every repo with the real `gh`.
pub fn fetch_many(
    repos: &[String],
    kind: ItemKind,
    limit: usize,
    timeout: Duration,
    parallel: usize,
) -> FetchOutcome {
    fetch_many_with(repos, parallel, |repo| {
        fetch_one(repo, kind, limit, timeout)
    })
}
