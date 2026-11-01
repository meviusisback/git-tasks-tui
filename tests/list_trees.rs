//! Worktree rows in the bottom-left list: children under their issue/PR,
//! the orphan section below the divider, and the `v` toggle between that
//! and issues alone. Cursor, mouse, Enter and the detail all read the
//! same rows the screen draws.

use std::path::PathBuf;

use gtasks::app::{App, Key, ListRow, Msg, Pane};
use gtasks::fetch::FetchOutcome;
use gtasks::model::{Item, ItemKind};

fn item(repo: &str, number: u64, kind: ItemKind, head: Option<&str>) -> Item {
    Item {
        number,
        title: format!("title {number}"),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        created_at: String::new(),
        url: format!("https://example.test/{repo}/{number}"),
        head_ref_name: head.map(str::to_string),
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        closing_issues_references: Vec::new(),
        repo: repo.to_string(),
        kind,
    }
}

fn plant_worktree(root: &std::path::Path, branch: &str) -> PathBuf {
    plant_worktree_for(root, "o/r", branch)
}

fn plant_worktree_for(root: &std::path::Path, repo: &str, branch: &str) -> PathBuf {
    use gtasks::worktree::BranchName;
    let slug = gtasks::actions::RepoSlug::parse(repo).unwrap();
    let branch = BranchName::parse(branch).unwrap();
    let path =
        gtasks::worktree::resolve_in(root, std::path::Path::new(""), &slug, &branch).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    let gitdir = root.join("gitdir-").join(branch.as_str());
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
    std::fs::write(path.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    path
}

/// One PR on `fix-foo`, one issue (link branch `issue-8`), and one tree
/// on a branch no issue owns.
fn tree_app() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().join("r");
    std::fs::create_dir(&clone).unwrap();
    std::fs::create_dir(clone.join(".git")).unwrap();

    let mut app = App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("o/r").expect("valid slug"),
    ]);
    app.apply_outcome(FetchOutcome {
        items: vec![
            item("o/r", 7, ItemKind::Pr, Some("fix-foo")),
            item("o/r", 8, ItemKind::Issue, None),
        ],
        failures: Vec::new(),
    });
    app.worktree_root = Some(tmp.path().join("wt"));
    (app, tmp)
}

fn wt(app: &App) -> PathBuf {
    app.worktree_root.clone().expect("worktree root")
}

fn press(app: &mut App, key: Key) {
    app.update(Msg::Key(key));
}

fn row_summary(rows: &[ListRow]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            ListRow::Item { number, .. } => format!("issue #{number}"),
            ListRow::Tree(tree) => format!(
                "tree {} ({})",
                tree.branch,
                if tree.number.is_some() {
                    "child"
                } else {
                    "orphan"
                }
            ),
            ListRow::Divider => String::from("divider"),
        })
        .collect()
}

#[test]
fn children_show_under_their_issue_and_orphans_below() {
    let (app, _tmp) = tree_app();
    plant_worktree(&wt(&app), "fix-foo");
    plant_worktree(&wt(&app), "issue-8");
    plant_worktree(&wt(&app), "lonely-branch");
    let summary = row_summary(&app.visible_rows());
    assert_eq!(
        summary,
        vec![
            "issue #7",
            "tree fix-foo (child)",
            "issue #8",
            "tree issue-8 (child)",
            "divider",
            "tree lonely-branch (orphan)",
        ],
        "children follow their parent, orphans gather below the divider"
    );
}

#[test]
fn the_toggle_hides_every_tree_row() {
    let (mut app, _tmp) = tree_app();
    plant_worktree(&wt(&app), "fix-foo");
    plant_worktree(&wt(&app), "lonely-branch");
    assert!(app.visible_rows().len() > 2, "trees show by default");
    press(&mut app, Key::Char('v'));
    assert!(!app.show_trees, "v flips the filter off");
    let summary = row_summary(&app.visible_rows());
    assert_eq!(summary, vec!["issue #7", "issue #8"]);
    assert!(
        app.status.contains("off"),
        "the status says what changed: {}",
        app.status
    );
    press(&mut app, Key::Char('v'));
    assert!(app.show_trees, "v flips it back on");
    assert_eq!(app.visible_rows().len(), 5);
}

#[test]
fn the_cursor_skips_the_divider() {
    let (mut app, _tmp) = tree_app();
    plant_worktree(&wt(&app), "fix-foo");
    plant_worktree(&wt(&app), "lonely-branch");
    // Rows: #7, fix-foo, #8, divider, lonely-branch.
    app.pane = Pane::List;
    app.list_cursor = 2; // #8
    press(&mut app, Key::Down);
    assert_eq!(app.list_cursor, 4, "down jumps over the divider");
    assert!(
        matches!(app.selected_row(), Some(ListRow::Tree(_))),
        "and lands on the orphan"
    );
    press(&mut app, Key::Up);
    assert_eq!(app.list_cursor, 2, "up jumps back over it");
}

#[test]
fn enter_on_a_child_opens_the_tab_on_that_tree() {
    let (mut app, _tmp) = tree_app();
    let path = plant_worktree(&wt(&app), "fix-foo");
    app.pane = Pane::List;
    app.list_cursor = 1; // the fix-foo child
    press(&mut app, Key::Enter);
    assert_eq!(app.tabs.len(), 1, "one tab opens");
    assert_eq!(app.tabs[0].repo, "o/r");
    assert_eq!(app.tabs[0].number, 7);
    assert_eq!(
        app.tabs[0].tree,
        path.display().to_string(),
        "placed on the child's tree from the start"
    );
    assert_eq!(app.active_tab, Some(0));
    assert_eq!(app.pane, Pane::Detail, "focus moves right like Enter does");
}

#[test]
fn enter_on_an_orphan_without_a_tab_says_so() {
    let (mut app, _tmp) = tree_app();
    plant_worktree(&wt(&app), "lonely-branch");
    app.pane = Pane::List;
    app.list_cursor = 2; // rows: #7, #8, lonely-branch (no divider: no, wait)
    let summary = row_summary(&app.visible_rows());
    let orphan_idx = summary
        .iter()
        .position(|row| row.contains("orphan"))
        .expect("an orphan row");
    app.list_cursor = orphan_idx;
    press(&mut app, Key::Enter);
    assert!(app.tabs.is_empty(), "orphans name no issue: no tab opens");
    assert!(
        app.status.contains("no tab on this tree"),
        "and the status says why: {}",
        app.status
    );
}

#[test]
fn a_child_resolves_to_its_parent_everywhere() {
    let (mut app, _tmp) = tree_app();
    plant_worktree(&wt(&app), "fix-foo");
    app.pane = Pane::List;
    app.list_cursor = 1; // the fix-foo child
    assert_eq!(
        app.selected_item().map(|item| item.number),
        Some(7),
        "actions on the child act on #7"
    );
    assert_eq!(
        app.effective_item().map(|item| item.number),
        Some(7),
        "and the detail shows the parent while no tab is open"
    );
}

#[test]
fn hiding_the_parent_hides_its_trees_instead_of_orphaning_them() {
    let (mut app, _tmp) = tree_app();
    plant_worktree(&wt(&app), "fix-foo");
    app.query = "zzz-no-match".to_string();
    app.apply_outcome(FetchOutcome {
        items: vec![
            item("o/r", 7, ItemKind::Pr, Some("fix-foo")),
            item("o/r", 8, ItemKind::Issue, None),
        ],
        failures: Vec::new(),
    });
    assert!(
        app.visible_rows().is_empty(),
        "no items, and the linked tree stays hidden with its parent: {:?}",
        row_summary(&app.visible_rows())
    );
}

#[test]
fn a_click_on_the_divider_selects_nothing() {
    let (mut app, _tmp) = tree_app();
    plant_worktree(&wt(&app), "lonely-branch");
    // Rows: #7, #8, divider, lonely-branch.
    app.list_cursor = 0;
    app.click_list_row(2, std::time::Instant::now());
    assert_eq!(app.list_cursor, 0, "the divider is drawn, not selected");
    app.click_list_row(3, std::time::Instant::now());
    assert_eq!(app.list_cursor, 3, "but the orphan below selects fine");
}

#[test]
fn the_choice_survives_a_restart() {
    let (mut app, _tmp) = tree_app();
    assert!(app.show_trees, "trees show by default");
    press(&mut app, Key::Char('v'));
    let mut state = gtasks::state::State::default();
    app.capture_session(&mut state);
    assert!(!state.show_trees, "the off choice is captured");
    let mut fresh = App::default();
    fresh.restore_session(&state);
    assert!(!fresh.show_trees, "and restored");
    // An old file without the field reads as shown, not hidden.
    let old: gtasks::state::State =
        serde_json::from_str("{\"selected_repos\":[]}").expect("old files still parse");
    assert!(old.show_trees, "missing field means shown");
}

#[test]
fn orphans_show_for_repos_with_zero_issues_under_all() {
    // The screenshot: agent-orchestr has tabs and trees but no issues, so
    // under "all repos" its trees must still land in the orphan section.
    let (mut app, _tmp) = tree_app();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("o/r").expect("valid slug"),
        gtasks::actions::RepoSlug::parse("o/z").expect("valid slug"),
    ]);
    plant_worktree_for(&wt(&app), "o/z", "scratch-dev");
    let summary = row_summary(&app.visible_rows());
    assert!(
        summary.iter().any(|row| row == "tree scratch-dev (orphan)"),
        "the issueless repo's tree shows: {summary:?}"
    );
    // And drilled in, the scope is just that repo: same orphan, no items.
    app.repo_filter =
        gtasks::app::RepoFilter::One(gtasks::actions::RepoSlug::parse("o/z").expect("valid slug"));
    let drilled = row_summary(&app.visible_rows());
    assert_eq!(
        drilled,
        vec!["tree scratch-dev (orphan)"],
        "drilled in: only the orphan, no divider, no items"
    );
}
