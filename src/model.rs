//! GitHub item models (`gh … --json`) and the pure list operations.
//!
//! Parsing is `serde` only: no I/O and no panics — a malformed payload becomes
//! an `Err`. The fetcher fills `repo` and `kind` after parsing, because a `gh`
//! payload is per-repo and per-kind and does not carry either field.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Whether an item came from `gh pr list` or `gh issue list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ItemKind {
    Pr,
    #[default]
    Issue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Author {
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
}

/// One entry of `statusCheckRollup`; `gh` varies the shape by provider, so
/// every field is optional and unknown keys are ignored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub conclusion: Option<String>,
}

/// One entry of `closingIssuesReferences`; shape varies, so everything is
/// optional and unknown keys are ignored — same discipline as `Check`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClosingRef {
    #[serde(default)]
    pub number: Option<u64>,
    #[serde(default)]
    pub title: Option<String>,
}

/// A pull request or an issue, as returned by the pinned `--json` field list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub is_draft: Option<bool>,
    #[serde(default)]
    pub author: Option<Author>,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub assignees: Vec<Author>,
    /// RFC3339 timestamps from `gh`; compared lexicographically (same `Z` form).
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub head_ref_name: Option<String>,
    #[serde(default)]
    pub base_ref_name: Option<String>,
    #[serde(default)]
    pub review_decision: Option<String>,
    #[serde(default)]
    pub mergeable: Option<String>,
    #[serde(default)]
    pub status_check_rollup: Vec<Check>,
    #[serde(default)]
    pub closing_issues_references: Vec<ClosingRef>,
    /// Set by the fetcher: the payload does not say which repo it came from.
    /// Serialized for the items cache, defaulted when a payload omits it.
    #[serde(default)]
    pub repo: String,
    /// Set by the fetcher: the payload does not say PR vs issue.
    /// Serialized for the items cache, defaulted when a payload omits it.
    #[serde(default)]
    pub kind: ItemKind,
}

/// Parse a `gh … list --json` array. Errors carry the serde message; nothing
/// panics, however malformed the payload.
pub fn parse_list(json: &str) -> anyhow::Result<Vec<Item>> {
    serde_json::from_str(json).map_err(|e| anyhow::anyhow!("cannot parse gh JSON: {e}"))
}

/// Days since the civil date (Howard Hinnant's algorithm): the only calendar
/// math `relative_time` needs, so "3 days ago" does not need a date crate.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn parse_rfc3339_utc(text: &str) -> Option<u64> {
    let (date, time) = text.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let mut d = date.split('-');
    let (y, m, day) = (
        d.next()?.parse::<i64>().ok()?,
        d.next()?.parse::<i64>().ok()?,
        d.next()?.parse::<i64>().ok()?,
    );
    let mut t = time.split(':');
    let (hh, mm, ss) = (
        t.next()?.parse::<i64>().ok()?,
        t.next()?.parse::<i64>().ok()?,
        t.next()?.parse::<i64>().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m, day);
    days.checked_mul(86_400)?
        .checked_add(hh * 3600 + mm * 60 + ss)
        .and_then(|s| u64::try_from(s).ok())
}

/// Coarse relative time for the detail box ("5m ago", "3h ago", "4d ago").
/// `None` when the timestamp does not parse: the caller omits the field
/// instead of showing a wrong one. The future reads as "just now" — a
/// skewed clock is not a time machine.
pub fn relative_time(now_unix: u64, rfc3339: &str) -> Option<String> {
    let then = parse_rfc3339_utc(rfc3339)?;
    let diff = now_unix.saturating_sub(then);
    let text = if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3_600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3_600)
    } else {
        format!("{}d ago", diff / 86_400)
    };
    Some(if then > now_unix {
        "just now".to_string()
    } else {
        text
    })
}

impl Item {
    /// `gh` says "missing" two ways: JSON null and the empty string
    /// (`reviewDecision: ""` on an unreviewed PR). Both mean absent: a blank
    /// line would claim the review exists and says nothing.
    fn present(field: &Option<String>) -> Option<&str> {
        field.as_deref().filter(|s| !s.is_empty())
    }

    /// Structured detail rows, one per present field, in the order the
    /// plugin shows them: context first, CI verdicts after, links last.
    /// Empty fields are *omitted*, never blank: a blank line would claim
    /// "none". The title is a `Title` marker (the pane reads the number,
    /// title and kind off the item itself); the `DESCRIPTION` heading and
    /// the body live in the pane, not here.
    pub fn detail_rows(&self, now_unix: u64) -> Vec<DetailRow> {
        let mut rows = vec![DetailRow::Title];
        let mut field = |label: &'static str, value: String, tone: Option<FieldTone>| {
            rows.push(DetailRow::Field(DetailField { label, value, tone }));
        };
        if !self.repo.is_empty() {
            field("repository", self.repo.clone(), None);
        }
        field(
            "type",
            match self.kind {
                ItemKind::Pr => "Pull request".to_string(),
                ItemKind::Issue => "Issue".to_string(),
            },
            None,
        );
        if let Some(author) = &self.author {
            field("author", author.login.clone(), None);
        }
        if let Some(ago) = relative_time(now_unix, &self.created_at) {
            field("created", ago, None);
        }
        if let Some(ago) = relative_time(now_unix, &self.updated_at) {
            field("updated", ago, None);
        }
        match (
            Self::present(&self.head_ref_name),
            Self::present(&self.base_ref_name),
        ) {
            (Some(head), Some(base)) => field(
                "branch",
                format!("{head} → {base}"),
                Some(FieldTone::Accent),
            ),
            (Some(head), None) => field("branch", head.to_string(), Some(FieldTone::Accent)),
            (None, Some(base)) => field("branch", base.to_string(), Some(FieldTone::Accent)),
            (None, None) => {}
        }
        if !self.labels.is_empty() {
            field(
                "labels",
                self.labels
                    .iter()
                    .map(|label| label.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                None,
            );
        }
        if !self.assignees.is_empty() {
            field(
                "assignees",
                self.assignees
                    .iter()
                    .map(|a| a.login.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                None,
            );
        }
        if let Some(review) = Self::present(&self.review_decision) {
            let tone = if review.eq_ignore_ascii_case("approved") {
                Some(FieldTone::Good)
            } else if review.eq_ignore_ascii_case("changes_requested") {
                Some(FieldTone::Bad)
            } else {
                Some(FieldTone::Warn)
            };
            field("review", review.to_string(), tone);
        }
        // `checks: none` and `checks: never asked` are different sentences.
        // The batch query always asks for the rollup on PRs, so an empty one
        // honestly means none: the line always shows. Issues are never
        // asked, so the line is omitted.
        if self.kind == ItemKind::Pr || !self.status_check_rollup.is_empty() {
            let mut failed = Vec::new();
            let (mut passed, mut pending) = (0, 0);
            for check in &self.status_check_rollup {
                // `gh` spells these SUCCESS/FAILURE; compare loosely so a
                // provider that prefers lowercase still counts.
                let conclusion = check.conclusion.as_deref().unwrap_or("");
                if conclusion.eq_ignore_ascii_case("success") {
                    passed += 1;
                } else if conclusion.eq_ignore_ascii_case("failure") {
                    failed.push(check.name.as_deref().unwrap_or("unknown").to_string());
                } else {
                    pending += 1;
                }
            }
            let mut line = format!("{passed} passed");
            if !failed.is_empty() {
                let mut counts: Vec<(&str, usize)> = Vec::new();
                for name in &failed {
                    if let Some(entry) = counts.iter_mut().find(|(n, _)| *n == name) {
                        entry.1 += 1;
                    } else {
                        counts.push((name.as_str(), 1));
                    }
                }
                let mut shown: Vec<String> = counts
                    .iter()
                    .take(2)
                    .map(|(name, n)| {
                        if *n > 1 {
                            format!("{name} ×{n}")
                        } else {
                            (*name).to_string()
                        }
                    })
                    .collect();
                if counts.len() > 2 {
                    shown.push(format!("+{}", counts.len() - 2));
                }
                line.push_str(&format!(", {} failed ({})", failed.len(), shown.join(", ")));
            }
            if pending > 0 {
                line.push_str(&format!(", {pending} pending"));
            }
            let tone = if !failed.is_empty() {
                Some(FieldTone::Bad)
            } else if pending > 0 {
                Some(FieldTone::Warn)
            } else {
                Some(FieldTone::Good)
            };
            field("checks", line, tone);
        }
        if let Some(mergeable) = Self::present(&self.mergeable) {
            let tone = if mergeable.eq_ignore_ascii_case("mergeable") {
                Some(FieldTone::Good)
            } else if mergeable.eq_ignore_ascii_case("conflicting") {
                Some(FieldTone::Bad)
            } else {
                None
            };
            field("mergeable", mergeable.to_string(), tone);
        }
        let linked: Vec<String> = self
            .closing_issues_references
            .iter()
            .filter_map(|r| {
                r.number.map(|n| match &r.title {
                    Some(t) => format!("#{n} {t}"),
                    None => format!("#{n}"),
                })
            })
            .collect();
        if !linked.is_empty() {
            field("linked", linked.join(", "), None);
        }
        if !self.url.is_empty() {
            field("url", self.url.clone(), None);
        }
        rows
    }
}

/// One labelled value of the detail table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailField {
    pub label: &'static str,
    pub value: String,
    pub tone: Option<FieldTone>,
}

/// How the detail pane paints a field value. `None` (on the row) means
/// plain text; a tone means the value carries a verdict (pass/fail,
/// approved, conflicting) that the colour says before the words are read.
/// The palette lives in `theme`; this enum only names the intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTone {
    Good,
    Bad,
    Warn,
    Accent,
}

/// One row of the detail table. The title is a marker (the pane reads the
/// number, title and kind off the item itself); everything else is data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailRow {
    Title,
    Field(DetailField),
}

/// Case-insensitive substring match over number, title, author and labels.
pub fn matches(item: &Item, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    if item.number.to_string().contains(&query) {
        return true;
    }
    if item.title.to_lowercase().contains(&query) {
        return true;
    }
    if let Some(author) = &item.author
        && author.login.to_lowercase().contains(&query)
    {
        return true;
    }
    item.labels
        .iter()
        .any(|label| label.name.to_lowercase().contains(&query))
}

/// Filter preserving the input order.
pub fn filter_items<'a>(items: &'a [Item], query: &str) -> Vec<&'a Item> {
    items.iter().filter(|item| matches(item, query)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Updated,
    Created,
    Number,
}

/// Stable sort by `key`, descending by default (`ascending = false`).
pub fn sort_items(items: &mut [Item], key: SortKey, ascending: bool) {
    items.sort_by(|a, b| {
        let order = match key {
            SortKey::Updated => a.updated_at.cmp(&b.updated_at),
            SortKey::Created => a.created_at.cmp(&b.created_at),
            SortKey::Number => a.number.cmp(&b.number),
        };
        if ascending { order } else { order.reverse() }
    });
}

/// Group by `repo`, preserving first-seen repo order and within-group order.
pub fn group_by_repo(items: &[Item]) -> Vec<(String, Vec<&Item>)> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
    for item in items {
        if !groups.contains_key(&item.repo) {
            order.push(item.repo.clone());
        }
        groups.entry(item.repo.clone()).or_default().push(item);
    }
    order
        .into_iter()
        .map(|repo| {
            let group = groups.remove(&repo).unwrap_or_default();
            (repo, group)
        })
        .collect()
}
