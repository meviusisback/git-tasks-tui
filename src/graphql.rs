//! Task 13b: one GraphQL call fetches up to [`BATCH_SIZE`] repos at once.
//!
//! Two REST calls per repo (69 repos → 138 calls, ~71 s to first paint)
//! become a handful of batched reads (69 repos → 4 calls, ~5 s). The query
//! text is fully generated here from validated slugs plus a fixed template:
//! no configuration, no user input, and no `mutation` keyword can reach it —
//! a slug that could break out of a `"…"` literal cannot pass
//! [`RepoSlug::parse`](crate::actions::RepoSlug::parse). The batch travels
//! through the **checked** transport ([`run_checked`](crate::gh::run_checked)):
//! the untrusted path refuses `gh api` wholesale (it could `--method POST`),
//! and this is a read the builder vouches for.

use std::collections::HashMap;

use serde::Deserialize;

use crate::actions::RepoSlug;
use crate::model::{Author, Check, ClosingRef, Item, ItemKind, Label};

/// Repos per GraphQL call. Twenty aliases sit comfortably under the limits
/// (measured: ~4 s, 1.8 KB, cost spread over one call instead of forty).
pub const BATCH_SIZE: usize = 20;

/// `{ nodes: […] }`: every list in the query shares this shape. The explicit
/// bound keeps serde from demanding `T: Default` (imperfect derive).
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: serde::Deserialize<'de>"))]
struct NodeList<T> {
    #[serde(default)]
    nodes: Vec<T>,
}

impl<T> Default for NodeList<T> {
    fn default() -> Self {
        Self { nodes: Vec::new() }
    }
}

/// Fields of one pull request node. Identity (`number`) is required — a node
/// without it means the schema moved, and failing loudly beats showing half
/// a list. Everything else is optional metadata.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrNode {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    is_draft: Option<bool>,
    #[serde(default)]
    author: Option<Login>,
    #[serde(default)]
    labels: NodeList<LabelName>,
    #[serde(default)]
    assignees: NodeList<Login>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    head_ref_name: Option<String>,
    #[serde(default)]
    base_ref_name: Option<String>,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    mergeable: Option<String>,
    #[serde(default)]
    closing_issues_references: NodeList<Ref>,
    #[serde(default)]
    commits: NodeList<CommitNode>,
}

/// Fields of one issue node: the PR-only ones do not exist on issues, so
/// they are not requested (GraphQL errors on unknown fields).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueNode {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    author: Option<Login>,
    #[serde(default)]
    labels: NodeList<LabelName>,
    #[serde(default)]
    assignees: NodeList<Login>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    url: String,
}

#[derive(Debug, Deserialize)]
struct Login {
    #[serde(default)]
    login: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LabelName {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Ref {
    #[serde(default)]
    number: Option<u64>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommitNode {
    #[serde(default)]
    commit: Commit,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    #[serde(default)]
    status_check_rollup: Option<Rollup>,
}

#[derive(Debug, Default, Deserialize)]
struct Rollup {
    #[serde(default)]
    contexts: NodeList<Context>,
}

/// A check run and a status context in one struct: `context` is set only on
/// the latter, which carries `state` where a run carries `conclusion`.
#[derive(Debug, Deserialize)]
struct Context {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoData {
    #[serde(default)]
    pull_requests: NodeList<PrNode>,
    #[serde(default)]
    issues: NodeList<IssueNode>,
}

#[derive(Debug, Default, Deserialize)]
struct Response {
    #[serde(default)]
    data: Option<HashMap<String, Option<RepoData>>>,
}

/// One repo's read: its items (origin stamped by the assembler, per S4),
/// or the reason this repo yielded nothing.
#[derive(Debug)]
pub struct RepoRead {
    pub repo: String,
    pub items: Vec<Item>,
    pub error: Option<String>,
}

fn checks(contexts: Vec<Context>) -> Vec<Check> {
    contexts
        .into_iter()
        .map(|c| {
            if let Some(context) = c.context {
                Check {
                    name: Some(context),
                    status: None,
                    conclusion: c.state,
                }
            } else {
                Check {
                    name: c.name,
                    status: c.status,
                    conclusion: c.conclusion,
                }
            }
        })
        .collect()
}

fn pr_item(node: PrNode) -> Item {
    Item {
        number: node.number,
        title: node.title,
        state: node.state,
        is_draft: node.is_draft,
        author: node
            .author
            .and_then(|a| a.login)
            .map(|login| Author { login }),
        labels: node
            .labels
            .nodes
            .into_iter()
            .filter_map(|l| l.name)
            .map(|name| Label { name, color: None })
            .collect(),
        assignees: node
            .assignees
            .nodes
            .into_iter()
            .filter_map(|a| a.login)
            .map(|login| Author { login })
            .collect(),
        updated_at: node.updated_at,
        created_at: node.created_at,
        url: node.url,
        head_ref_name: node.head_ref_name,
        base_ref_name: node.base_ref_name,
        review_decision: node.review_decision,
        mergeable: node.mergeable,
        status_check_rollup: node
            .commits
            .nodes
            .into_iter()
            .next()
            .and_then(|n| n.commit.status_check_rollup)
            .map(|r| checks(r.contexts.nodes))
            .unwrap_or_default(),
        closing_issues_references: node
            .closing_issues_references
            .nodes
            .into_iter()
            .map(|r| ClosingRef {
                number: r.number,
                title: r.title,
            })
            .collect(),
        repo: String::new(),
        kind: ItemKind::Pr,
    }
}

fn issue_item(node: IssueNode) -> Item {
    Item {
        number: node.number,
        title: node.title,
        state: node.state,
        is_draft: None,
        author: node
            .author
            .and_then(|a| a.login)
            .map(|login| Author { login }),
        labels: node
            .labels
            .nodes
            .into_iter()
            .filter_map(|l| l.name)
            .map(|name| Label { name, color: None })
            .collect(),
        assignees: node
            .assignees
            .nodes
            .into_iter()
            .filter_map(|a| a.login)
            .map(|login| Author { login })
            .collect(),
        updated_at: node.updated_at,
        created_at: node.created_at,
        url: node.url,
        head_ref_name: None,
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        closing_issues_references: Vec::new(),
        repo: String::new(),
        kind: ItemKind::Issue,
    }
}

/// Build one batched query: alias `r{i}` per repo, PRs plus issues, both
/// capped at `limit`. Pure: the same slugs always yield the same text.
/// Braces are balanced by construction — each block opens and closes in
/// the same literal — and `braces_balance_for_any_batch_size` pins it.
pub fn batch_query(batch: &[RepoSlug], limit: usize) -> String {
    const PR_FIELDS: &str = "number title state url createdAt updatedAt isDraft \
        author { login } labels(first: 20) { nodes { name } } \
        assignees(first: 20) { nodes { login } } \
        headRefName baseRefName reviewDecision mergeable \
        closingIssuesReferences(first: 10) { nodes { number title } } \
        commits(last: 1) { nodes { commit { statusCheckRollup { \
        contexts(first: 50) { nodes { __typename \
        ... on CheckRun { name conclusion status } \
        ... on StatusContext { context state } } } } } } }";
    const ISSUE_FIELDS: &str = "number title state url createdAt updatedAt \
        author { login } labels(first: 20) { nodes { name } } \
        assignees(first: 20) { nodes { login } }";
    let mut out = String::from("query {");
    for (i, slug) in batch.iter().enumerate() {
        let (owner, name) = slug
            .as_str()
            .split_once('/')
            .expect("validated slugs are owner/name");
        out.push_str(&format!(
            " r{i}: repository(owner: \"{owner}\", name: \"{name}\") {{ \
                pullRequests(first: {limit}, states: OPEN) {{ nodes {{ {PR_FIELDS} }} }} \
                issues(first: {limit}, states: OPEN) {{ nodes {{ {ISSUE_FIELDS} }} }} }}"
        ));
    }
    out.push_str(" }");
    out
}

/// Split a query batch into its `gh api graphql` argv: one `--field`, never
/// `-f` (the builder path does not take short flags), one argument per
/// word — no shell anywhere near the query text.
pub fn batch_argv(query: String) -> Vec<std::ffi::OsString> {
    vec![
        "api".into(),
        "graphql".into(),
        "--field".into(),
        format!("query={query}").into(),
    ]
}

/// Parse one batch response into per-repo reads, zipped with `batch` by
/// alias (`r{i}` ↔ `batch[i]`). A null alias is that repo's failure, not the
/// batch's: the rest still parses.
pub fn assemble(batch: &[RepoSlug], text: &str) -> anyhow::Result<Vec<RepoRead>> {
    let response: Response = serde_json::from_str(text)
        .map_err(|e| anyhow::anyhow!("cannot parse GraphQL batch: {e}"))?;
    // A null `data` with errors is a server-level refusal, not twenty
    // per-repo failures: report it as a batch failure so the caller bisects
    // instead of blaming every repo in the batch.
    let mut data = response
        .data
        .ok_or_else(|| anyhow::anyhow!("GraphQL batch returned no data"))?;
    let mut reads = Vec::with_capacity(batch.len());
    for (i, slug) in batch.iter().enumerate() {
        let alias = format!("r{i}");
        match data.remove(&alias).flatten() {
            Some(repo) => {
                let mut items: Vec<Item> =
                    repo.pull_requests.nodes.into_iter().map(pr_item).collect();
                items.extend(repo.issues.nodes.into_iter().map(issue_item));
                // S4: origin comes from the fetcher, never the payload.
                for item in &mut items {
                    item.repo = slug.as_str().to_string();
                }
                reads.push(RepoRead {
                    repo: slug.as_str().to_string(),
                    items,
                    error: None,
                });
            }
            _ => reads.push(RepoRead {
                repo: slug.as_str().to_string(),
                items: Vec::new(),
                error: Some(format!("no data for alias {alias}")),
            }),
        }
    }
    Ok(reads)
}
