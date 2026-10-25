//! Task 13: the detail box shows structured fields from data `gh` really
//! returned. Empty fields are omitted, never blank — a blank line would
//! claim "none", and that is a different sentence from "never asked".

use gtasks::model::{Author, Check, ClosingRef, Item, ItemKind, Label, relative_time};

// 2026-01-01T00:00:00Z. One verified anchor; every other timestamp in these
// tests is an offset from it, so the suite does not rot.
const NEW_YEAR: u64 = 1_767_225_600;

fn rich_pr() -> Item {
    Item {
        number: 12,
        title: "Add widget".to_string(),
        state: "OPEN".to_string(),
        is_draft: None,
        author: Some(Author {
            login: "alberto".to_string(),
        }),
        labels: vec![
            Label {
                name: "bug".to_string(),
                color: None,
            },
            Label {
                name: "ui".to_string(),
                color: None,
            },
        ],
        assignees: vec![Author {
            login: "bob".to_string(),
        }],
        created_at: "2025-12-30T00:00:00Z".to_string(),
        updated_at: "2025-12-31T23:00:00Z".to_string(),
        url: "https://github.com/o/r/pull/12".to_string(),
        head_ref_name: Some("feat/x".to_string()),
        base_ref_name: Some("main".to_string()),
        review_decision: Some("APPROVED".to_string()),
        mergeable: Some("MERGEABLE".to_string()),
        status_check_rollup: vec![
            Check {
                name: Some("ci".to_string()),
                status: Some("COMPLETED".to_string()),
                conclusion: Some("SUCCESS".to_string()),
            },
            Check {
                name: Some("lint".to_string()),
                status: Some("COMPLETED".to_string()),
                conclusion: Some("FAILURE".to_string()),
            },
            Check {
                name: None,
                status: Some("IN_PROGRESS".to_string()),
                conclusion: None,
            },
        ],
        closing_issues_references: vec![ClosingRef {
            number: Some(5),
            title: Some("linked issue".to_string()),
        }],
        repo: "o/r".to_string(),
        kind: ItemKind::Pr,
    }
}

#[test]
fn relative_time_counts_in_coarse_buckets() {
    assert_eq!(
        relative_time(NEW_YEAR, "2026-01-01T00:00:00Z"),
        Some("0s ago".to_string())
    );
    assert_eq!(
        relative_time(NEW_YEAR, "2025-12-31T23:59:00Z"),
        Some("1m ago".to_string())
    );
    assert_eq!(
        relative_time(NEW_YEAR, "2025-12-31T22:00:00Z"),
        Some("2h ago".to_string())
    );
    assert_eq!(
        relative_time(NEW_YEAR, "2025-12-28T00:00:00Z"),
        Some("4d ago".to_string())
    );
}

#[test]
fn relative_time_refuses_garbage_and_the_future() {
    assert_eq!(relative_time(NEW_YEAR, ""), None);
    assert_eq!(relative_time(NEW_YEAR, "not-a-date"), None);
    assert_eq!(relative_time(NEW_YEAR, "2026-13-45T99:99:99Z"), None);
    // A skewed clock is not a time machine: never print a future age.
    assert_eq!(
        relative_time(NEW_YEAR, "2026-01-02T00:00:00Z"),
        Some("just now".to_string())
    );
}

#[test]
fn a_full_pr_shows_every_present_field() {
    let rows = rich_pr().detail_fields(NEW_YEAR);
    assert_eq!(
        rows,
        vec![
            "#12 Add widget",
            "state: OPEN",
            "author: alberto",
            "labels: bug, ui",
            "assignees: bob",
            "created 2d ago · updated 1h ago",
            "feat/x → main",
            "checks: 1 passed, 1 failed (lint), 1 pending",
            "mergeable: MERGEABLE",
            "review: APPROVED",
            "linked: #5 linked issue",
            "url: https://github.com/o/r/pull/12",
            "repo: o/r",
        ]
    );
}

#[test]
fn empty_fields_are_omitted_never_blank() {
    let mut item = rich_pr();
    item.author = None;
    item.labels.clear();
    item.assignees.clear();
    item.created_at.clear();
    item.updated_at.clear();
    item.head_ref_name = None;
    item.base_ref_name = None;
    item.review_decision = Some(String::new()); // gh's other "missing"
    item.mergeable = None;
    item.status_check_rollup.clear();
    item.closing_issues_references.clear();
    item.url.clear();
    item.repo.clear();
    let rows = item.detail_fields(NEW_YEAR);
    assert_eq!(rows, vec!["#12 Add widget", "state: OPEN"]);
    for row in &rows {
        assert!(
            !row.ends_with(':'),
            "a blank field would claim \"none\": {row:?}"
        );
    }
}

#[test]
fn new_fields_parse_and_old_payloads_still_parse() {
    let json = r#"[{"number":9,"title":"T","state":"OPEN",
        "assignees":[{"login":"bob"}],
        "baseRefName":"main",
        "closingIssuesReferences":[{"number":5,"title":"linked"}]}]"#;
    let items = gtasks::model::parse_list(json).unwrap();
    assert_eq!(items[0].assignees[0].login, "bob");
    assert_eq!(items[0].base_ref_name.as_deref(), Some("main"));
    assert_eq!(items[0].closing_issues_references[0].number, Some(5));
    // ... and a payload without the new keys still parses (defaults).
    let minimal = r#"[{"number":1,"title":"M","state":"OPEN"}]"#;
    let items = gtasks::model::parse_list(minimal).unwrap();
    assert!(items[0].assignees.is_empty());
    assert!(items[0].base_ref_name.is_none());
    assert!(items[0].closing_issues_references.is_empty());
}
