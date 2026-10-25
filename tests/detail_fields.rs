//! The detail table: one structured row per present field, plugin order
//! (context, CI verdicts, links), tones on the verdicts. Empty fields are
//! omitted, never blank.

use gtasks::model::{
    Author, Check, ClosingRef, DetailRow, FieldTone, Item, ItemKind, Label, relative_time,
};

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

fn values(rows: &[DetailRow]) -> Vec<(&'static str, String, Option<FieldTone>)> {
    rows.iter()
        .filter_map(|row| match row {
            DetailRow::Title => None,
            DetailRow::Field(field) => Some((field.label, field.value.clone(), field.tone)),
        })
        .collect()
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
fn a_full_pr_shows_every_present_field_in_plugin_order() {
    let rows = rich_pr().detail_rows(NEW_YEAR);
    assert!(matches!(rows.first(), Some(DetailRow::Title)));
    assert_eq!(
        values(&rows),
        vec![
            ("repository", "o/r".to_string(), None),
            ("type", "Pull request".to_string(), None),
            ("author", "alberto".to_string(), None),
            ("created", "2d ago".to_string(), None),
            ("updated", "1h ago".to_string(), None),
            (
                "branch",
                "feat/x → main".to_string(),
                Some(FieldTone::Accent)
            ),
            ("labels", "bug, ui".to_string(), None),
            ("assignees", "bob".to_string(), None),
            ("review", "APPROVED".to_string(), Some(FieldTone::Good)),
            (
                "checks",
                "1 passed, 1 failed (lint), 1 pending".to_string(),
                Some(FieldTone::Bad),
            ),
            ("mergeable", "MERGEABLE".to_string(), Some(FieldTone::Good)),
            ("linked", "#5 linked issue".to_string(), None),
            ("url", "https://github.com/o/r/pull/12".to_string(), None),
        ]
    );
}

#[test]
fn failed_checks_count_by_name_and_cap_at_two_plus_n() {
    // (ci, ci) reads like a misprint; the user wants to know *which* workflow
    // is red. And a long tail of names would burst the pane at 80 columns.
    fn failing(name: &str) -> Check {
        Check {
            name: Some(name.to_string()),
            status: Some("COMPLETED".to_string()),
            conclusion: Some("FAILURE".to_string()),
        }
    }
    let mut item = rich_pr();
    item.status_check_rollup = vec![failing("ci"), failing("ci")];
    let checks = values(&item.detail_rows(NEW_YEAR));
    let checks = checks
        .iter()
        .find(|(label, _, _)| *label == "checks")
        .expect("checks row");
    assert_eq!(checks.1, "0 passed, 2 failed (ci ×2)");
    assert_eq!(checks.2, Some(FieldTone::Bad));

    item.status_check_rollup = vec![failing("lint"), failing("ci"), failing("docs")];
    let checks = values(&item.detail_rows(NEW_YEAR));
    let checks = checks
        .iter()
        .find(|(label, _, _)| *label == "checks")
        .expect("checks row");
    assert_eq!(checks.1, "0 passed, 3 failed (lint, ci, +1)");
}

#[test]
fn tones_read_the_verdicts() {
    let mut item = rich_pr();
    item.review_decision = Some("CHANGES_REQUESTED".to_string());
    item.mergeable = Some("CONFLICTING".to_string());
    item.status_check_rollup = vec![];
    let rows = values(&item.detail_rows(NEW_YEAR));
    let tone = |label: &str| {
        rows.iter()
            .find(|(l, _, _)| *l == label)
            .map(|(_, _, t)| *t)
    };
    assert_eq!(tone("review"), Some(Some(FieldTone::Bad)));
    assert_eq!(tone("mergeable"), Some(Some(FieldTone::Bad)));
    // Empty rollup on a PR honestly means none: the line shows, all green.
    assert_eq!(tone("checks"), Some(Some(FieldTone::Good)));

    let mut pending = rich_pr();
    pending.status_check_rollup = vec![Check {
        name: Some("ci".to_string()),
        status: Some("IN_PROGRESS".to_string()),
        conclusion: None,
    }];
    let rows = values(&pending.detail_rows(NEW_YEAR));
    let checks = rows.iter().find(|(l, _, _)| *l == "checks").unwrap();
    assert_eq!(checks.1, "0 passed, 1 pending");
    assert_eq!(checks.2, Some(FieldTone::Warn));
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
    let rows = values(&item.detail_rows(NEW_YEAR));
    let labels: Vec<&str> = rows.iter().map(|(l, _, _)| *l).collect();
    for missing in [
        "repository",
        "author",
        "labels",
        "assignees",
        "created",
        "updated",
        "branch",
        "review",
        "mergeable",
        "linked",
        "url",
    ] {
        assert!(
            !labels.contains(&missing),
            "{missing} is omitted: {labels:?}"
        );
    }
    // A PR with no checks says none — the rollup was asked for, so zero is
    // a datum, not an absence. The type row always shows.
    let shown: Vec<(&str, String)> = rows.iter().map(|(l, v, _)| (*l, v.clone())).collect();
    assert_eq!(
        shown,
        vec![
            ("type", "Pull request".to_string()),
            ("checks", "0 passed".to_string()),
        ]
    );
    // An issue is never asked for checks: there the line is omitted —
    // "never asked" is not "none".
    item.kind = ItemKind::Issue;
    let rows = values(&item.detail_rows(NEW_YEAR));
    let labels: Vec<&str> = rows.iter().map(|(l, _, _)| *l).collect();
    assert_eq!(labels, vec!["type"]);
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
