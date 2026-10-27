//! The cross: one grid for draw, pty sizing and mouse drags. These pin
//! the tiling (no gaps, no overlaps), the divider grabs and the drag
//! guards.

use gtasks::ui::{col_pct_for, grid, list_row_at, pane_at, repos_row_at, row_pct_for};
use ratatui::layout::Rect;

fn area() -> Rect {
    Rect::new(0, 0, 200, 40)
}

fn two_item_app() -> gtasks::app::App {
    use gtasks::fetch::FetchOutcome;
    use gtasks::model::{Item, ItemKind};
    let mut app = gtasks::app::App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("me/a").expect("valid slug"),
        gtasks::actions::RepoSlug::parse("me/b").expect("valid slug"),
    ]);
    let item = |repo: &str, number: u64| Item {
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
        kind: ItemKind::Pr,
    };
    app.apply_outcome(FetchOutcome {
        items: vec![item("me/a", 1), item("me/b", 2)],
        failures: Vec::new(),
    });
    app
}

#[test]
fn the_cross_tiles_without_gaps_or_overlaps() {
    let g = grid(area(), 30, 38);
    // Columns meet at the divider, rows meet at the divider.
    assert_eq!(g.repos.x + g.repos.width, g.div_x);
    assert_eq!(g.detail.x, g.div_x);
    assert_eq!(g.agent.x, g.div_x);
    assert_eq!(g.repos.y + g.repos.height, g.div_y);
    assert_eq!(g.list.y, g.div_y);
    assert_eq!(g.detail.y + g.detail.height, g.div_y);
    assert_eq!(g.agent.y, g.div_y);
    // Bottoms land on the body bottom, the status on the frame bottom.
    assert_eq!(g.list.y + g.list.height, g.body.y + g.body.height);
    assert_eq!(g.agent.y + g.agent.height, g.body.y + g.body.height);
    assert_eq!(
        g.status.y + g.status.height,
        39,
        "status above the frame border"
    );
    assert_eq!(g.status.x, 1);
    // Divider grabs sit on real border cells, inside the body.
    assert!(g.div_x > g.body.x && g.div_x < g.body.x + g.body.width);
    assert!(g.div_y > g.body.y && g.div_y < g.body.y + g.body.height);
}

#[test]
fn splits_move_the_dividers() {
    let narrow = grid(area(), 15, 38);
    let wide = grid(area(), 85, 38);
    assert!(narrow.div_x < wide.div_x, "columns follow col_pct");
    let top = grid(area(), 30, 15);
    let bottom = grid(area(), 30, 85);
    assert!(top.div_y < bottom.div_y, "rows follow row_pct");
    // Defaults keep the historical geometry: 30/70 columns.
    let g = grid(area(), 30, 38);
    let left_w = g.div_x - g.body.x;
    assert!(
        (left_w as i32 - 59).abs() <= 1,
        "30% of 198 ≈ 59, got {left_w}"
    );
}

#[test]
fn clicking_a_box_names_its_pane() {
    use gtasks::app::Pane;
    let g = grid(area(), 30, 38);
    assert_eq!(
        pane_at(&g, g.repos.x + 2, g.repos.y + 1, true),
        Some(Pane::Repos)
    );
    assert_eq!(
        pane_at(&g, g.list.x + 2, g.list.y + 1, true),
        Some(Pane::List)
    );
    assert_eq!(
        pane_at(&g, g.detail.x + 2, g.detail.y, true),
        Some(Pane::Detail)
    );
    assert_eq!(
        pane_at(&g, g.agent.x + 2, g.agent.y + 1, true),
        Some(Pane::Agent)
    );
    // Without the box the whole lower right is detail: a click there
    // focuses detail, never a hidden pane.
    assert_eq!(
        pane_at(&g, g.agent.x + 2, g.agent.y + 1, false),
        Some(Pane::Detail)
    );
    assert_eq!(
        pane_at(&g, 0, 0, true),
        None,
        "outside the boxes: the frame"
    );
    assert_eq!(
        pane_at(&g, g.status.x + 1, g.status.y, true),
        None,
        "the status row focuses nothing"
    );
}

#[test]
fn drag_math_refuses_slivers_and_tiny_screens() {
    let body = Rect::new(1, 1, 198, 37);
    assert_eq!(col_pct_for(body, 1 + 59), Some(29));
    assert_eq!(col_pct_for(body, 1 + 5), None, "left sliver refused");
    assert_eq!(col_pct_for(body, 1 + 190), None, "right sliver refused");
    assert_eq!(row_pct_for(body, 1 + 14), Some(37));
    assert_eq!(row_pct_for(body, 1 + 1), None, "top sliver refused");
    assert_eq!(row_pct_for(body, 1 + 36), None, "bottom sliver refused");
    let tiny = Rect::new(0, 0, 20, 8);
    assert_eq!(col_pct_for(tiny, 10), None, "too narrow to drag");
    assert_eq!(row_pct_for(tiny, 4), None, "too short to drag");
}

#[test]
fn clicking_a_list_row_selects_it() {
    let g = grid(area(), 30, 38);
    // First content row sits one below the top border.
    assert_eq!(list_row_at(g.list, g.list.y + 1, 2), Some(0));
    assert_eq!(list_row_at(g.list, g.list.y + 2, 2), Some(1));
    assert_eq!(
        list_row_at(g.list, g.list.y, 2),
        None,
        "border selects nothing"
    );
    assert_eq!(
        list_row_at(g.list, g.list.y + 3, 2),
        None,
        "past the last row selects nothing"
    );
    let mut app = two_item_app();
    app.click_list_row(1, std::time::Instant::now());
    assert_eq!(app.list_cursor, 1);
    assert_eq!(
        app.effective_item().map(|i| i.number),
        Some(2),
        "no tabs: the detail follows the clicked row"
    );
    app.click_list_row(9, std::time::Instant::now()); // overflow: ignored, cursor stays
    assert_eq!(app.list_cursor, 1);
}

#[test]
fn clicking_a_repos_row_moves_the_filter() {
    let g = grid(area(), 30, 38);
    assert_eq!(repos_row_at(g.repos, g.repos.y + 1, 3), Some(0));
    assert_eq!(repos_row_at(g.repos, g.repos.y + 2, 3), Some(1));
    assert_eq!(
        repos_row_at(g.repos, g.repos.y, 3),
        None,
        "border selects nothing"
    );
    let mut app = two_item_app();
    app.click_repos_row(2, std::time::Instant::now()); // me/b
    assert_eq!(
        app.visible_items()
            .iter()
            .map(|i| i.number)
            .collect::<Vec<_>>(),
        vec![2],
        "the list follows the clicked repo"
    );
}

#[test]
fn double_click_on_a_row_opens_its_tab() {
    use gtasks::app::Pane;
    let mut app = two_item_app();
    app.pane = Pane::List;
    let first = std::time::Instant::now();
    app.click_list_row(0, first);
    assert!(app.tabs.is_empty(), "one click only selects");
    app.click_list_row(0, first);
    assert_eq!(app.tabs.len(), 1, "two clicks open the tab");
    assert_eq!(app.pane, Pane::Detail, "the tab takes the focus right");
}

#[test]
fn two_slow_clicks_do_not_open_a_tab() {
    use gtasks::app::Pane;
    let mut app = two_item_app();
    app.pane = Pane::List;
    let first = std::time::Instant::now();
    app.click_list_row(0, first);
    app.click_list_row(0, first + std::time::Duration::from_secs(5));
    assert!(
        app.tabs.is_empty(),
        "clicks past the window are two single clicks"
    );
}

#[test]
fn double_click_in_repos_drills_in() {
    use gtasks::app::Pane;
    let mut app = two_item_app();
    app.pane = Pane::Repos;
    let at = std::time::Instant::now();
    app.click_repos_row(1, at);
    assert_eq!(app.pane, Pane::Repos, "one click only moves the filter");
    app.click_repos_row(1, at);
    assert_eq!(app.pane, Pane::List, "two clicks drill into the repo");
}
