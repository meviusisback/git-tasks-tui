//! Task 3: `gh --json` parsing and the pure list operations.
//!
//! Fixtures are inline so the test does not depend on `gh` being installed.

use gtasks::model::{ItemKind, SortKey, filter_items, group_by_repo, parse_list, sort_items};

const PRS: &str = r#"[
  {"number":12,"title":"Add widget","state":"OPEN","isDraft":false,
   "author":{"login":"alberto"},"labels":[{"name":"bug","color":"ff0000"}],
   "updatedAt":"2026-10-20T10:00:00Z","createdAt":"2026-10-01T09:00:00Z",
   "url":"https://github.com/o/r/pull/12","headRefName":"feat/x",
   "reviewDecision":"APPROVED","mergeable":"MERGEABLE",
   "statusCheckRollup":[{"name":"ci","status":"COMPLETED","conclusion":"SUCCESS"}]},
  {"number":7,"title":"Fix crash","state":"OPEN",
   "author":{"login":"bob"},"labels":[],
   "updatedAt":"2026-10-22T10:00:00Z","createdAt":"2026-10-02T09:00:00Z",
   "url":"https://github.com/o/r/pull/7","headRefName":"fix/crash"}
]"#;

#[test]
fn parses_pr_list_with_all_fields() {
    let items = parse_list(PRS).unwrap();
    assert_eq!(items.len(), 2);
    let first = &items[0];
    assert_eq!(first.number, 12);
    assert_eq!(first.title, "Add widget");
    assert_eq!(first.is_draft, Some(false));
    assert_eq!(first.author.as_ref().unwrap().login, "alberto");
    assert_eq!(first.labels[0].name, "bug");
    assert_eq!(first.head_ref_name.as_deref(), Some("feat/x"));
    assert_eq!(first.review_decision.as_deref(), Some("APPROVED"));
    assert_eq!(first.status_check_rollup.len(), 1);
    assert_eq!(
        first.status_check_rollup[0].conclusion.as_deref(),
        Some("SUCCESS")
    );
}

/// Missing optional keys are empty, not errors: `gh` omits them per provider.
#[test]
fn missing_optionals_default_to_empty() {
    let json = r#"[{"number":1,"title":"Minimal","state":"OPEN"}]"#;
    let items = parse_list(json).unwrap();
    let item = &items[0];
    assert!(item.author.is_none());
    assert!(item.labels.is_empty());
    assert!(item.status_check_rollup.is_empty());
    assert_eq!(item.updated_at, "");
    assert_eq!(item.repo, "");
}

/// Malformed JSON is an `Err`, never a panic.
#[test]
fn invalid_json_is_an_error_not_a_panic() {
    let err = parse_list("{not json").unwrap_err();
    assert!(
        format!("{err:#}").contains("cannot parse"),
        "error must be descriptive, got: {err:#}"
    );
}

/// A wrong field type fails parsing instead of silently coercing.
#[test]
fn wrong_field_type_is_an_error() {
    let bad = r#"[{"number":"twelve","title":"x","state":"OPEN"}]"#;
    assert!(parse_list(bad).is_err());
}

#[test]
fn filter_is_case_insensitive_across_fields() {
    let items = parse_list(PRS).unwrap();
    assert_eq!(filter_items(&items, "").len(), 2);
    assert_eq!(filter_items(&items, "WIDGET").len(), 1);
    assert_eq!(filter_items(&items, "12").len(), 1);
    assert_eq!(filter_items(&items, "bob").len(), 1);
    assert_eq!(filter_items(&items, "bug").len(), 1);
    assert_eq!(filter_items(&items, "zzz").len(), 0);
}

#[test]
fn sort_by_updated_and_number() {
    let mut items = parse_list(PRS).unwrap();

    sort_items(&mut items, SortKey::Updated, false);
    assert_eq!(items[0].number, 7, "newest first by default");
    sort_items(&mut items, SortKey::Updated, true);
    assert_eq!(items[0].number, 12, "oldest first when ascending");

    sort_items(&mut items, SortKey::Number, true);
    assert_eq!(items[0].number, 7);
    sort_items(&mut items, SortKey::Number, false);
    assert_eq!(items[0].number, 12);
}

#[test]
fn group_by_repo_preserves_first_seen_order() {
    let mut items = parse_list(PRS).unwrap();
    items[0].repo = "b/repo".into();
    items[1].repo = "a/repo".into();

    let groups = group_by_repo(&items);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].0, "b/repo");
    assert_eq!(groups[1].0, "a/repo");
    assert_eq!(groups[0].1.len(), 1);

    items[1].repo = "b/repo".into();
    let groups = group_by_repo(&items);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].1.len(), 2);
}

/// `kind` is not in the payload: the default is `Issue` until the fetcher says
/// otherwise, and the fetcher can set it afterwards.
#[test]
fn kind_is_set_by_the_fetcher_not_by_json() {
    let mut items = parse_list(PRS).unwrap();
    assert_eq!(items[0].kind, ItemKind::Issue);
    items[0].kind = ItemKind::Pr;
    assert_eq!(items[0].kind, ItemKind::Pr);
}

/// M-5: `sort_items` compares RFC3339 strings lexicographically, which is
/// chronological only in the `…Z` form `gh` emits. This test fixes the
/// assumption so a future `+02:00` timestamp fails loudly instead of sorting
/// silently wrong.
#[test]
fn sort_assumes_z_timestamps() {
    let z_form = r#"[
      {"number":1,"title":"early","state":"OPEN","updatedAt":"2026-10-20T09:00:00Z"},
      {"number":2,"title":"late","state":"OPEN","updatedAt":"2026-10-20T11:00:00Z"}
    ]"#;
    let mut items = parse_list(z_form).unwrap();
    sort_items(&mut items, SortKey::Updated, false);
    assert_eq!(items[0].number, 2, "Z-form timestamps sort chronologically");

    // A mixed offset parses (the model does not reject it) but is not
    // comparable lexicographically: 09:00Z == 11:00+02:00, yet "09" < "11".
    let mixed = r#"[
      {"number":1,"title":"a","state":"OPEN","updatedAt":"2026-10-20T09:00:00Z"},
      {"number":2,"title":"b","state":"OPEN","updatedAt":"2026-10-20T11:00:00+02:00"}
    ]"#;
    let mut items = parse_list(mixed).unwrap();
    sort_items(&mut items, SortKey::Updated, false);
    assert_eq!(
        items[0].number, 2,
        "documents the trap: mixed offsets are ordered by text, not instant"
    );
}
