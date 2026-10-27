//! The detail fetch must ask for the right object: `gh issue view` for
//! issues, `gh pr view` for PRs. A hardcoded `pr` asks GitHub for a pull
//! request with an issue's number ("Could not resolve to a PullRequest")
//! and the body never loads.

use gtasks::app::App;
use gtasks::model::{Item, ItemKind};

fn item(repo: &str, number: u64, kind: ItemKind) -> Item {
    Item {
        number,
        title: "t".to_string(),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        updated_at: "2026-10-20T10:00:00Z".into(),
        created_at: String::new(),
        url: format!("https://github.com/{repo}/pull/{number}"),
        head_ref_name: None,
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        closing_issues_references: Vec::new(),
        repo: repo.to_string(),
        kind,
    }
}

#[test]
fn detail_kind_follows_the_list_row() {
    let app = App {
        items: vec![
            item("acme/one", 5, ItemKind::Issue),
            item("acme/one", 6, ItemKind::Pr),
        ],
        ..App::default()
    };
    assert_eq!(app.item_kind("acme/one", 5), ItemKind::Issue);
    assert_eq!(app.item_kind("acme/one", 6), ItemKind::Pr);
}

#[test]
fn detail_kind_of_a_vanished_row_stays_pr() {
    let app = App {
        items: vec![item("acme/one", 5, ItemKind::Issue)],
        ..App::default()
    };
    // The row left the list: keep the old guess so the fetch runs and
    // fails honestly instead of trusting the disk.
    assert_eq!(app.item_kind("acme/one", 9), ItemKind::Pr);
    assert_eq!(app.item_kind("acme/two", 5), ItemKind::Pr);
}
