//! Tabs, the agent pane and the session: the split-column redesign.
//!
//! Left column is repos (top) + issues/PRs (bottom); the filter follows the
//! repos cursor. Right column is tabs + detail (top) + agent/worktree (bottom).
//! The session (`state.json`) keeps pane, cursors, filter, query and tabs so
//! a temporary exit resumes where the user left off.

use gtasks::app::{App, Key, Msg, Pane};
use gtasks::model::{Item, ItemKind};

fn item(repo: &str, number: u64, title: &str) -> Item {
    Item {
        number,
        title: title.to_string(),
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
    }
}

fn app_with_items() -> App {
    let mut app = App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("me/a").expect("valid slug"),
        gtasks::actions::RepoSlug::parse("me/b").expect("valid slug"),
    ]);
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![item("me/a", 1, "one"), item("me/b", 2, "two")],
        failures: Vec::new(),
    });
    app
}

fn press(app: &mut App, key: Key) {
    app.update(Msg::Key(key));
}

#[test]
fn enter_in_the_list_opens_a_tab_and_the_detail_follows_it() {
    let mut app = app_with_items();
    assert!(app.tabs.is_empty());
    press(&mut app, Key::Enter);
    assert_eq!(app.tabs.len(), 1, "Enter opens one tab");
    assert_eq!(app.active_tab, Some(0));
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/a", 1))
    );
    // Moving the list cursor does not drag the detail away from the tab.
    press(&mut app, Key::Down);
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/a", 1)),
        "the tab keeps its issue while the list moves"
    );
}

#[test]
fn t_cycles_tabs_forward_and_back() {
    let mut app = app_with_items();
    press(&mut app, Key::Enter); // #1
    press(&mut app, Key::Down);
    press(&mut app, Key::Enter); // #2
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active_tab, Some(1));
    press(&mut app, Key::Char('t'));
    assert_eq!(app.active_tab, Some(0), "t wraps to the first tab");
    press(&mut app, Key::Char('T'));
    assert_eq!(app.active_tab, Some(1), "T goes back");
}

#[test]
fn x_closes_the_tab_first_then_the_record() {
    use gtasks::agent::{AgentOutcome, AgentRecord, AgentStatus};
    let mut app = app_with_items();
    press(&mut app, Key::Enter);
    assert_eq!(app.tabs.len(), 1);
    press(&mut app, Key::Char('x'));
    assert!(app.tabs.is_empty(), "x closes the tab first");
    assert_eq!(app.active_tab, None);
    // Without tabs, x forgets the agent record exactly as before.
    app.agent_record = Some(AgentRecord {
        repo: "me/a".to_string(),
        number: 1,
        profile: "default".to_string(),
        worktree: std::path::PathBuf::from("/tmp/wt"),
        seed_lines: 1,
        status: AgentStatus::Finished {
            outcome: AgentOutcome::Exit(0),
            secs: 1,
        },
    });
    assert!(app.visible_record().is_some());
    press(&mut app, Key::Char('x'));
    assert!(
        app.agent_record.is_none(),
        "x closes the record when no tab"
    );
}

#[test]
fn w_focuses_the_agent_pane_with_a_hint() {
    let mut app = app_with_items();
    press(&mut app, Key::Char('w'));
    assert_eq!(app.pane, Pane::Agent);
    assert!(
        app.status.contains("agent pane"),
        "w names where it went: {}",
        app.status
    );
}

#[test]
fn esc_climbs_from_the_agent_pane_to_the_detail() {
    let mut app = app_with_items();
    app.pane = Pane::Agent;
    press(&mut app, Key::Esc);
    assert_eq!(app.pane, Pane::Detail);
    press(&mut app, Key::Esc);
    assert_eq!(app.pane, Pane::List);
}

#[test]
fn the_agent_panel_names_the_worktree_steps() {
    let app = app_with_items();
    let text = app.agent_panel_text();
    for line in [
        "g create worktree + launch agent",
        "Enter opens a tab",
        "t/T next/prev tab",
        "profile ·",
        "clones ·",
    ] {
        assert!(text.contains(line), "agent pane says {line:?}: {text}");
    }
}

#[test]
fn the_session_roundtrip_keeps_tabs_filter_and_cursors() {
    let mut app = app_with_items();
    press(&mut app, Key::Enter); // tab #1
    press(&mut app, Key::Char('t')); // single tab: stays, but marks dirty
    // Simulate the loop: capture the session, save, load into a fresh app.
    let mut state = gtasks::state::State {
        selected_repos: Vec::new(),
        ..gtasks::state::State::default()
    };
    app.capture_session(&mut state);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    state.save(&path).unwrap();
    let back = gtasks::state::State::load(&path).unwrap();

    let mut fresh = App::default();
    fresh.set_repos(vec![
        gtasks::actions::RepoSlug::parse("me/a").expect("valid slug"),
        gtasks::actions::RepoSlug::parse("me/b").expect("valid slug"),
    ]);
    fresh.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![item("me/a", 1, "one"), item("me/b", 2, "two")],
        failures: Vec::new(),
    });
    fresh.restore_session(&back);
    assert_eq!(fresh.tabs.len(), 1, "tabs survive a restart");
    assert_eq!(fresh.active_tab, Some(0));
    assert_eq!(
        fresh.effective_item().map(|i| i.number),
        Some(1),
        "the detail is back on the tab"
    );
}

#[test]
fn a_stale_session_never_breaks_startup() {
    let state = gtasks::state::State {
        pane: "sidebar".to_string(), // unknown: falls back to list
        repo_filter: Some("not/a/valid/slug/at/all".to_string()),
        repos_cursor: 99,
        list_cursor: 99,
        active_tab: Some(7), // out of range
        ..gtasks::state::State::default()
    };
    let mut app = app_with_items();
    app.restore_session(&state);
    assert_eq!(app.pane, Pane::List);
    assert!(app.tabs.is_empty());
    assert_eq!(app.active_tab, None);
}

#[test]
fn the_view_carries_tabs_and_agent_to_the_screen() {
    let mut app = app_with_items();
    press(&mut app, Key::Enter);
    let view = gtasks::ui::view_for(&app);
    assert_eq!(view.tabs.len(), 1);
    assert_eq!(view.active_tab, Some(0));
    assert!(
        view.tabs[0].contains("#1"),
        "tab label names the issue: {:?}",
        view.tabs
    );
    assert!(
        view.agent.contains("g create worktree"),
        "agent pane reaches the screen: {}",
        view.agent
    );
}
