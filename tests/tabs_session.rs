//! Tabs, the agent pane and the session: the split-column redesign.
//!
//! Left column is repos (top) + issues/PRs (bottom); the filter follows the
//! repos cursor. Right column is tabs + detail (top) + agent/worktree (bottom).
//! The session (`state.json`) keeps pane, cursors, filter, query and tabs so
//! a temporary exit resumes where the user left off.

use std::collections::HashMap;

use gtasks::app::{App, Key, Msg, Pane, Tab, TabAgent};
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
    // Scrolling the list never drags the detail off the tab: the open
    // tab owns the right column. Without tabs the detail follows the
    // cursor instead (covered where no tab is ever opened).
    press(&mut app, Key::Esc); // back to the list
    press(&mut app, Key::Down);
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/a", 1)),
        "the tab owns the detail while it lives"
    );
    press(&mut app, Key::Enter);
    assert_eq!(app.tabs.len(), 2, "Enter opens the row's tab");
    assert_eq!(app.pane, Pane::Detail, "Enter moves the focus right");
}

#[test]
fn switching_tabs_jumps_across_a_filter() {
    let mut app = app_with_items();
    press(&mut app, Key::Enter); // tab #1, pinned
    // Narrow the list to me/b, then bookmark #2 as well. Drilling in
    // lands on me/b's tab straight away.
    press(&mut app, Key::Esc); // back to the list
    press(&mut app, Key::Tab); // repos pane
    press(&mut app, Key::Down);
    press(&mut app, Key::Down); // me/b: the filter follows
    press(&mut app, Key::Enter); // drills to the list
    press(&mut app, Key::Enter); // tab #2, pinned
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/b", 2))
    );
    // The strip only shows me/b's tab now — and `t` stays inside it.
    assert_eq!(app.tab_infos().len(), 1, "only the repo's tab shows");
    // Switch back: clear the filter first (Esc to the list, Esc to the
    // repos pane, Esc on row 0), then `t` jumps across repos like before.
    press(&mut app, Key::Esc); // back to the list
    press(&mut app, Key::Esc); // back to the repos pane, filter kept
    press(&mut app, Key::Esc); // row 0 clears the filter
    assert_eq!(app.tab_infos().len(), 2, "all tabs show again");
    let cursor = app.repos_cursor;
    press(&mut app, Key::Char('t'));
    assert_eq!(app.active_tab, Some(0));
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/a", 1)),
        "switching tabs jumps across repos with no filter"
    );
    assert_eq!(app.repos_cursor, cursor, "the repo cursor stays");
    // Scrolling leaves the tab's detail alone: only another tab (or
    // closing this one) hands the detail back to the list.
    press(&mut app, Key::Esc); // back to the list
    press(&mut app, Key::Down);
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/a", 1)),
        "scrolling never drags the detail off a tab"
    );
}

#[test]
fn the_active_tab_carries_a_background() {
    use gtasks::ui::{draw, view_for};
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = app_with_items();
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/a".to_string(),
        number: 1,
    });
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/b".to_string(),
        number: 2,
    });
    app.active_tab = Some(1);
    let view = view_for(&app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let selected = ratatui::style::Color::Rgb(0x58, 0x5b, 0x70);
    let bg_of = |needle: &str| -> Vec<ratatui::style::Color> {
        // The tabs strip sits right under the outer border: row 1.
        let mut row = String::new();
        for x in 0..200 {
            row.push_str(buffer[(x, 1)].symbol());
        }
        let mut out = Vec::new();
        if let Some(start) = row.find(needle) {
            let start_char = row[..start].chars().count();
            for i in 0..needle.chars().count() {
                let x = (start_char + i) as u16;
                out.push(buffer[(x, 1)].bg);
            }
        }
        out
    };
    let active = bg_of("#2");
    assert!(!active.is_empty(), "the active tab is on screen");
    assert!(
        active.iter().all(|bg| *bg == selected),
        "the whole active tab stands out: {active:?}"
    );
    let idle = bg_of("#1");
    assert!(!idle.is_empty(), "the idle tab is on screen");
    assert!(
        idle.iter().all(|bg| *bg != selected),
        "the idle tab keeps no background: {idle:?}"
    );
}

#[test]
fn a_closed_tab_item_stays_in_the_detail() {
    let mut app = app_with_items();
    press(&mut app, Key::Enter); // tab #1, pinned
    let second = app.items[1].clone();
    // The next refresh drops #1: closed elsewhere. The list loses it,
    // the bookmarked detail keeps it.
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![second],
        failures: Vec::new(),
    });
    assert!(
        app.visible_items().iter().all(|i| i.number != 1),
        "a closed issue leaves the list"
    );
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/a", 1)),
        "but its tab keeps showing it"
    );
    // A repo that failed to fetch retains nothing: unknown is not closed.
    let mut app = app_with_items();
    press(&mut app, Key::Enter);
    let second = app.items[1].clone();
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![second],
        failures: vec![("me/a".to_string(), "boom".to_string())],
    });
    assert!(app.retained.is_empty(), "a failed repo keeps no one");
    // Reopened: back in the list, no copy kept aside.
    let mut app = app_with_items();
    press(&mut app, Key::Enter);
    let all = app.items.clone();
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![],
        failures: Vec::new(),
    });
    assert_eq!(app.retained.len(), 1, "closed while tabbed: retained");
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: all,
        failures: Vec::new(),
    });
    assert!(app.retained.is_empty(), "reopened: the list copy wins");
}

#[test]
fn the_body_fetch_uses_the_retained_kind() {
    use gtasks::model::ItemKind;
    let mut app = app_with_items();
    // Tab #1, then the refresh drops it: retained.
    press(&mut app, Key::Enter); // tab #1 (a PR here)
    let second = app.items[1].clone();
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![second],
        failures: Vec::new(),
    });
    assert_eq!(
        app.item_kind("me/a", 1),
        ItemKind::Pr,
        "retained items keep their kind, not the Pr default"
    );
    assert_eq!(
        app.item_kind("me/a", 9),
        ItemKind::Pr,
        "unknown items still default to Pr"
    );
}

#[test]
fn f_cycles_the_state_filter_on_a_single_repo_only() {
    let mut app = app_with_items();
    // All repos: closed history is per-repo, so `f` refuses.
    press(&mut app, Key::Char('f'));
    assert!(!app.refresh_requested, "no refetch without a single repo");
    assert!(
        app.status.contains("single repo"),
        "says why: {}",
        app.status
    );
    assert_eq!(app.state_filter, gtasks::app::StateFilter::Open);
    // Drill into me/a through keys, then cycle.
    press(&mut app, Key::Tab); // repos pane
    press(&mut app, Key::Down); // me/a: the filter follows
    press(&mut app, Key::Enter); // back to the list
    press(&mut app, Key::Char('f'));
    assert_eq!(app.state_filter, gtasks::app::StateFilter::Closed);
    assert!(app.refresh_requested, "the refetch carries the states");
    assert!(app.loading, "one fetch at a time");
    // The loop owns the fetch: simulate it landing before cycling on.
    app.loading = false;
    app.refresh_requested = false;
    press(&mut app, Key::Char('f'));
    assert_eq!(app.state_filter, gtasks::app::StateFilter::All);
    app.loading = false;
    app.refresh_requested = false;
    press(&mut app, Key::Char('f'));
    assert_eq!(app.state_filter, gtasks::app::StateFilter::Open);
}

#[test]
fn closed_items_show_oldest_first_on_a_single_repo() {
    let mut app = app_with_items();
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![
            item("me/a", 3, "three"),
            Item {
                number: 9,
                state: "CLOSED".into(),
                ..item("me/a", 9, "nine")
            },
            Item {
                number: 4,
                state: "CLOSED".into(),
                ..item("me/a", 4, "four")
            },
            item("me/b", 2, "two"),
        ],
        failures: Vec::new(),
    });
    // Drill into me/a through keys.
    press(&mut app, Key::Tab); // repos pane
    press(&mut app, Key::Down); // me/a
    press(&mut app, Key::Enter); // back to the list
    // Open by default: the closed ones stay hidden, no reorder.
    assert_eq!(
        app.visible_items()
            .iter()
            .map(|i| i.number)
            .collect::<Vec<_>>(),
        vec![3],
        "open-only, fetch order"
    );
    app.state_filter = gtasks::app::StateFilter::Closed;
    assert_eq!(
        app.visible_items()
            .iter()
            .map(|i| i.number)
            .collect::<Vec<_>>(),
        vec![4, 9],
        "closed only, numeric"
    );
    app.state_filter = gtasks::app::StateFilter::All;
    assert_eq!(
        app.visible_items()
            .iter()
            .map(|i| i.number)
            .collect::<Vec<_>>(),
        vec![3, 4, 9],
        "everything, numeric"
    );
}

#[test]
fn t_cycles_tabs_forward_and_back() {
    let mut app = app_with_items();
    press(&mut app, Key::Enter); // #1
    // The list narrowed to the first tab's repo: climb out to open #2.
    press(&mut app, Key::Esc);
    press(&mut app, Key::Up);
    press(&mut app, Key::Enter);
    press(&mut app, Key::Esc); // Enter focuses right: back to the list
    press(&mut app, Key::Down); // me/b #2
    press(&mut app, Key::Enter); // #2
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active_tab, Some(1));
    press(&mut app, Key::Char('t'));
    assert_eq!(app.active_tab, Some(0), "t wraps to the first tab");
    press(&mut app, Key::Char('T'));
    assert_eq!(app.active_tab, Some(1), "T goes back");
}

#[test]
fn activating_a_tab_keeps_the_repo_selection() {
    let mut app = app_with_items();
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/a".to_string(),
        number: 1,
    });
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/b".to_string(),
        number: 2,
    });
    // "All" selected: cursor 0, no filter.
    let cursor = app.repos_cursor;
    let filter = app.repo_filter.clone();
    app.activate_tab(1);
    assert_eq!(app.active_tab, Some(1));
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/b", 2)),
        "switching tabs jumps to the bookmark"
    );
    assert_eq!(app.repos_cursor, cursor, "the repo cursor stays");
    assert_eq!(
        app.repo_filter, filter,
        "the filter stays: an all-repos view stays all-repos"
    );
    app.activate_tab(0);
    assert_eq!(app.repos_cursor, cursor);
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/a", 1))
    );
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
    // A record filed under the open tab shows, then x closes the tab
    // (not the record) first.
    press(&mut app, Key::Esc); // closing keeps the pane: back to the list
    press(&mut app, Key::Enter);
    // The launch would have placed the bookmark on its tree.
    app.tabs[0].tree = "/tmp/wt".to_string();
    app.tab_agents.insert(
        ("me/a".to_string(), 1, "/tmp/wt".to_string()),
        TabAgent {
            record: Some(AgentRecord {
                repo: "me/a".to_string(),
                number: 1,
                profile: "default".to_string(),
                agent: "hermes".to_string(),
                worktree: std::path::PathBuf::from("/tmp/wt"),
                seed_lines: 1,
                status: AgentStatus::Finished {
                    outcome: AgentOutcome::Exit(0),
                    secs: 1,
                },
            }),
            ..Default::default()
        },
    );
    assert!(app.visible_record().is_some());
    press(&mut app, Key::Char('x'));
    assert!(app.tabs.is_empty(), "x closes the tab first");
    assert!(app.visible_record().is_none(), "the tab took its card");
    press(&mut app, Key::Char('x'));
    assert_eq!(app.status, "no agent record to close");
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
        "g chooses the agent + worktree, then launches",
        "task ·",
        "profile ·",
        "clones ·",
    ] {
        assert!(text.contains(line), "agent pane says {line:?}: {text}");
    }
    for line in ["switch", "close tab", "Enter opens a tab"] {
        assert!(
            !text.contains(line),
            "switch/close keys live in the legend only, not here: {text}"
        );
    }
}

#[test]
fn the_session_roundtrip_keeps_tabs_filter_and_cursors() {
    let mut app = app_with_items();
    press(&mut app, Key::Enter); // tab #1
    press(&mut app, Key::Char('t')); // single tab: stays, but marks dirty
    press(&mut app, Key::Esc); // back to the list
    press(&mut app, Key::Down); // cursor #2: the tab still owns the detail
    // Simulate the loop: capture the session, save, load into a fresh app.
    let mut state = gtasks::state::State {
        selected_repos: Vec::new(),
        ..gtasks::state::State::default()
    };
    app.capture_session(&mut state);
    let _guard = tempfile::tempdir().unwrap();
    let dir = _guard.path().canonicalize().unwrap();
    let path = dir.join("state.json");
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
fn opening_a_tab_leaves_the_left_column_alone() {
    // Opening a tab never reshapes the list: the repos cursor stays where
    // the user put it and the list cursor on its row.
    let mut app = app_with_items();
    app.repos_cursor = 0;
    app.list_cursor = 1; // looking at me/b #2, repos cursor elsewhere
    press(&mut app, Key::Enter);
    assert_eq!(app.active_tab, Some(0));
    assert!(
        matches!(&app.repo_filter, gtasks::app::RepoFilter::All),
        "no narrowing: {:?}",
        app.repo_filter
    );
    assert_eq!(app.repos_cursor, 0, "the repos cursor stays");
    assert_eq!(app.list_cursor, 1, "the list cursor stays on its row");
    assert_eq!(
        app.effective_item().map(|i| i.number),
        Some(2),
        "the detail is on the tab"
    );
}

#[test]
fn switching_tabs_leaves_the_left_column_alone() {
    let mut app = app_with_items();
    press(&mut app, Key::Enter); // me/a #1, the list is untouched
    press(&mut app, Key::Esc); // status only: Esc never leaves the list
    press(&mut app, Key::Up); // already row 0: stays
    press(&mut app, Key::Enter); // #1 already open and active: body no-op
    press(&mut app, Key::Esc); // Enter focuses right: back to the list
    press(&mut app, Key::Down); // me/b #2
    press(&mut app, Key::Enter); // second tab
    assert_eq!(app.active_tab, Some(1));
    press(&mut app, Key::Char('t')); // back to me/a #1
    assert_eq!(app.active_tab, Some(0));
    assert!(
        matches!(&app.repo_filter, gtasks::app::RepoFilter::All),
        "nothing touched the selection: {:?}",
        app.repo_filter
    );
    assert_eq!(
        app.effective_item().map(|i| i.number),
        Some(1),
        "switching tabs jumps to the bookmark"
    );
}

#[test]
fn the_agent_pane_shows_only_for_work_in_progress() {
    use gtasks::agent::{AgentOutcome, AgentRecord, AgentStatus};
    let mut app = app_with_items();
    assert!(!app.agent_active(), "idle: no pane");
    app.tab_agents = HashMap::from([(
        ("me/a".to_string(), 1, String::new()),
        TabAgent {
            running: true,
            record: Some(AgentRecord {
                repo: "me/a".to_string(),
                number: 1,
                profile: "default".to_string(),
                agent: "hermes".to_string(),
                worktree: std::path::PathBuf::from("/tmp/wt"),
                seed_lines: 1,
                status: AgentStatus::Running,
            }),
            ..Default::default()
        },
    )]);
    assert!(app.agent_active(), "running: pane shows");
    app.tab_agents = HashMap::from([(
        ("me/a".to_string(), 1, String::new()),
        TabAgent {
            record: Some(AgentRecord {
                repo: "me/a".to_string(),
                number: 1,
                profile: "default".to_string(),
                agent: "hermes".to_string(),
                worktree: std::path::PathBuf::from("/tmp/wt"),
                seed_lines: 1,
                status: AgentStatus::Finished {
                    outcome: AgentOutcome::Exit(0),
                    secs: 3,
                },
            }),
            ..Default::default()
        },
    )]);
    assert!(
        !app.agent_active(),
        "finished: the card stays in the detail, the pane hides"
    );
}

#[test]
fn nerd_icons_follow_the_override_not_the_host() {
    // Host fonts vary, so the override decides in tests: whatever the host
    // has, the env var wins.
    // SAFETY: `GTASKS_NERD` is process-global, but this env var is only read
    // by `icons::nerd()`, and no other test in this binary asserts on icon
    // output — the screen test only checks icon-independent strings. The
    // previous value is restored before returning.
    let previous = std::env::var_os("GTASKS_NERD");
    unsafe {
        std::env::set_var("GTASKS_NERD", "1");
    }
    assert!(gtasks::icons::nerd(), "GTASKS_NERD=1 forces icons on");
    assert!(!gtasks::icons::kind_pr().is_empty());
    unsafe {
        std::env::set_var("GTASKS_NERD", "0");
    }
    assert!(!gtasks::icons::nerd(), "GTASKS_NERD=0 forces icons off");
    assert!(gtasks::icons::kind_pr().is_empty());
    assert_eq!(gtasks::icons::repo(), "▸ ");
    unsafe {
        match previous {
            Some(value) => std::env::set_var("GTASKS_NERD", value),
            None => std::env::remove_var("GTASKS_NERD"),
        }
    }
}

#[test]
fn an_idle_tab_rents_no_agent_box() {
    use gtasks::ui::{draw, view_for};
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = app_with_items();
    press(&mut app, Key::Enter); // tab me/a#1
    press(&mut app, Key::Esc); // back to the list
    press(&mut app, Key::Down);
    press(&mut app, Key::Enter); // tab me/b#2, now active
    // An agent works in the other tab; the active one never ran.
    app.tab_agents.insert(
        ("me/a".to_string(), 1, String::new()),
        TabAgent {
            running: true,
            ..Default::default()
        },
    );
    assert!(
        !app.agent_box_shown(),
        "the box belongs to the active tab, not the app"
    );
    let view = view_for(&app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut screen = String::new();
    for y in 0..30 {
        for x in 0..200 {
            screen.push_str(buffer[(x, y)].symbol());
        }
        screen.push('\n');
    }
    assert!(
        !screen.contains("no run yet"),
        "no placeholder rents the idle tab's column: {screen}"
    );
    assert!(screen.contains("detail"), "the top box stays: {screen}");
}

#[test]
fn the_detail_keeps_the_pr_while_its_agent_runs() {
    use gtasks::agent::{AgentRecord, AgentStatus};
    use gtasks::ui::{draw, view_for};
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = app_with_items();
    press(&mut app, Key::Enter); // tab me/a#1, focus moves to the detail
    app.tab_agents.insert(
        ("me/a".to_string(), 1, String::new()),
        TabAgent {
            running: true,
            record: Some(AgentRecord {
                repo: "me/a".to_string(),
                number: 1,
                profile: "default".to_string(),
                agent: "omp".to_string(),
                worktree: std::path::PathBuf::from("/tmp/wt"),
                seed_lines: 4,
                status: AgentStatus::Running,
            }),
            ..Default::default()
        },
    );
    app.detail_for = Some(("me/a".to_string(), 1));
    app.detail = "Body line".to_string();
    assert!(app.agent_box_shown(), "the live tab keeps its box");
    let view = view_for(&app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut screen = String::new();
    for y in 0..30 {
        for x in 0..200 {
            screen.push_str(buffer[(x, y)].symbol());
        }
        screen.push('\n');
    }
    assert!(
        screen.contains("#1 one"),
        "the classic box stays up top: {screen}"
    );
    // The running card lives in the agent's box below (the instructions
    // say what runs there); the detail's own text carries no card.
    assert!(
        !view.detail.contains("seed"),
        "the detail text carries no card: {}",
        view.detail
    );
}

#[test]
fn a_tab_without_its_issue_names_it_on_an_empty_list() {
    use gtasks::ui::{draw, view_for};
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = app_with_items();
    // A tab for an issue the list does not have (stale fetch): with
    // nothing under the cursor the detail names it instead of a blank.
    app.items.clear();
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/a".to_string(),
        number: 11,
    });
    app.active_tab = Some(0);
    let view = view_for(&app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut screen = String::new();
    for y in 0..30 {
        for x in 0..200 {
            screen.push_str(buffer[(x, y)].symbol());
        }
        screen.push('\n');
    }
    assert!(
        screen.contains("#11 me/a"),
        "the tab names its own issue: {screen}"
    );
    assert!(
        screen.contains("not in the current list"),
        "honest instead of blank: {screen}"
    );
    assert!(
        !screen.contains("#1 one"),
        "the cleared list shows nothing else: {screen}"
    );
}

#[test]
fn the_screen_names_tab_and_repo_keys() {
    // Regression: the tab keys used to live on the tabs strip, which has no
    // room for prose. They now live in the bottom legend; the strip shows
    // only the tabs. This renders a real frame and reads it back.
    use gtasks::ui::{draw, view_for};
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = app_with_items();
    press(&mut app, Key::Enter);
    press(&mut app, Key::Esc); // Enter focuses right: back to the list
    assert!(!app.agent_active(), "idle: the agent pane stays hidden");
    let view = view_for(&app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..30 {
        for x in 0..200 {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    for needle in [
        "Tab repos", // bottom legend: how to reach the repos
        "←/→ tabs",  // bottom legend: how to move between tabs
        "x close",   // bottom legend: how to close one
        "●",         // the active tab marker on the strip
        "#1",        // the open tab itself
    ] {
        assert!(out.contains(needle), "screen names {needle:?}:\n{out}");
    }
    assert!(
        !out.contains("tabs:"),
        "the strip shows tabs, not prose:\n{out}"
    );
}

#[test]
fn the_live_agent_screen_reaches_the_box() {
    // A running session paints parsed cells, not instructions: render with a
    // fake screen and read the frame back.
    use gtasks::embedded::{AgentCell, Palette};
    use gtasks::ui::{draw, view_for};
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = app_with_items();
    press(&mut app, Key::Enter);
    app.pane = gtasks::app::Pane::Agent;
    app.tab_agents.insert(
        ("me/a".to_string(), 1, String::new()),
        TabAgent {
            running: true,
            screen: vec![vec![
                AgentCell {
                    text: "$".to_string(),
                    fg: Palette::Default,
                    bg: Palette::Default,
                    bold: true,
                    inverse: false,
                },
                AgentCell {
                    text: " hello-agent".to_string(),
                    fg: Palette::Idx(2),
                    bg: Palette::Default,
                    bold: false,
                    inverse: false,
                },
            ]],
            ..Default::default()
        },
    );
    let view = view_for(&app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..30 {
        for x in 0..200 {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    assert!(out.contains("hello-agent"), "live cells paint:\n{out}");
    assert!(
        out.contains("type to agent"),
        "the legend owns the focused-running keys:\n{out}"
    );
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
        view.agent.contains("g chooses the agent + worktree"),
        "agent pane reaches the screen: {}",
        view.agent
    );
}

#[test]
fn switching_tabs_refreshes_the_agent_line() {
    use gtasks::agent::{AgentRecord, AgentStatus};
    let mut app = app_with_items();
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/a".to_string(),
        number: 1,
    });
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/b".to_string(),
        number: 2,
    });
    app.tab_agents.insert(
        ("me/a".to_string(), 1, String::new()),
        TabAgent {
            running: true,
            screen: Vec::new(),
            scroll: 0,
            record: Some(AgentRecord {
                repo: "me/a".to_string(),
                number: 1,
                profile: "coder".to_string(),
                agent: "hermes".to_string(),
                worktree: std::path::PathBuf::from("/tmp/wt"),
                seed_lines: 4,
                status: AgentStatus::Running,
            }),
            dismissed_for: None,
            context: None,
            draft: None,
        },
    );
    app.activate_tab(0);
    assert!(
        app.status.contains("agent hermes running on me/a#1"),
        "the live tab names its agent: {}",
        app.status
    );
    app.activate_tab(1);
    assert!(
        !app.status.contains("me/a#1"),
        "the dead message does not stick to the new tab: {}",
        app.status
    );
    assert!(
        app.status.contains("from 2 repos") || app.status.contains("in me/"),
        "back to the idle summary: {}",
        app.status
    );
}

#[test]
fn switching_to_a_starting_agent_says_so() {
    let mut app = app_with_items();
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/a".to_string(),
        number: 1,
    });
    // Busy mark, no record yet: the Herdr thread has not answered.
    app.tab_agents.insert(
        ("me/a".to_string(), 1, String::new()),
        TabAgent {
            running: true,
            ..Default::default()
        },
    );
    app.activate_tab(0);
    assert_eq!(app.status, "starting the agent…");
}

fn herdr_tab_with_binding(app: &mut App) {
    use gtasks::agent::{AgentRecord, AgentStatus};
    use gtasks::herdr::HerdrBinding;
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/a".to_string(),
        number: 1,
    });
    app.active_tab = Some(0);
    app.tab_agents.insert(
        ("me/a".to_string(), 1, String::new()),
        TabAgent {
            running: true,
            record: Some(AgentRecord {
                repo: "me/a".to_string(),
                number: 1,
                profile: "coder".to_string(),
                agent: "hermes".to_string(),
                worktree: std::path::PathBuf::from("/tmp/wt"),
                seed_lines: 4,
                status: AgentStatus::Running,
            }),
            ..Default::default()
        },
    );
    app.herdr_bindings.insert(
        "/tmp/wt".to_string(),
        HerdrBinding {
            name: "g-a-1-hermes".to_string(),
            tab_id: "w3:t9".to_string(),
            pane_id: "w3:p9".to_string(),
            repo: "me/a".to_string(),
            number: 1,
            kind: "hermes".to_string(),
            profile: "coder".to_string(),
        },
    );
}

#[test]
fn k_asks_before_stopping_the_herdr_agent() {
    use gtasks::app::Modal;
    let mut app = app_with_items();
    herdr_tab_with_binding(&mut app);
    press(&mut app, Key::Char('K'));
    match &app.modal {
        Some(Modal::Confirm(question)) => assert!(
            question.contains("g-a-1-hermes"),
            "the question names the agent: {question}"
        ),
        other => panic!("K must ask first, got: {other:?}"),
    }
    // `y` does not stop anything itself — it hands the agreed stop to the
    // loop, which owns the Herdr CLI call.
    press(&mut app, Key::Char('y'));
    assert!(app.modal.is_none());
    let stop = app
        .stop_herdr_go
        .as_ref()
        .expect("the stop waits for the loop");
    assert_eq!(stop.tab_id, "w3:t9");
    assert_eq!(stop.tree, "/tmp/wt");
    assert_eq!(stop.key, ("me/a".to_string(), 1, String::new()));
}

#[test]
fn k_without_a_binding_says_so() {
    let mut app = app_with_items();
    app.tabs.push(Tab {
        tree: String::new(),
        repo: "me/a".to_string(),
        number: 1,
    });
    app.active_tab = Some(0);
    press(&mut app, Key::Char('K'));
    assert!(app.modal.is_none(), "nothing to confirm without a binding");
    assert!(
        app.status.contains("no Herdr agent"),
        "the status says so: {}",
        app.status
    );
}

fn placed_tab(app: &mut App, repo: &str, number: u64, tree: &str) {
    app.tabs.push(Tab {
        repo: repo.to_string(),
        number,
        tree: tree.to_string(),
    });
}

#[test]
fn two_trees_on_the_same_issue_are_two_tabs() {
    let mut app = app_with_items();
    placed_tab(&mut app, "me/a", 1, "/wt/a1");
    placed_tab(&mut app, "me/a", 1, "/wt/a2");
    for (tree, agent) in [("/wt/a1", "omp"), ("/wt/a2", "hermes")] {
        app.tab_agents.insert(
            ("me/a".to_string(), 1, tree.to_string()),
            TabAgent {
                running: true,
                record: Some(gtasks::agent::AgentRecord {
                    repo: "me/a".to_string(),
                    number: 1,
                    profile: "coder".to_string(),
                    agent: agent.to_string(),
                    worktree: std::path::PathBuf::from(tree),
                    seed_lines: 1,
                    status: gtasks::agent::AgentStatus::Running,
                }),
                ..Default::default()
            },
        );
    }
    app.active_tab = Some(0);
    let labels: Vec<String> = app.tab_infos().into_iter().map(|i| i.label).collect();
    assert_eq!(labels.len(), 2, "both trees show: {labels:?}");
    assert!(
        labels[0].contains("a1") && labels[0].contains("omp"),
        "first tab names its tree and agent: {}",
        labels[0]
    );
    assert!(
        labels[1].contains("a2") && labels[1].contains("hermes"),
        "second tab names its tree and agent: {}",
        labels[1]
    );
    // Switching tabs switches boxes.
    app.activate_tab(1);
    assert_eq!(
        app.active_key(),
        Some(("me/a".to_string(), 1, "/wt/a2".to_string()))
    );
    assert!(
        app.status.contains("agent hermes running on me/a#1"),
        "the status follows the tab: {}",
        app.status
    );
}

#[test]
fn the_strip_shows_only_the_filtered_repo() {
    let mut app = app_with_items();
    placed_tab(&mut app, "me/a", 1, "/wt/a");
    placed_tab(&mut app, "me/b", 2, "/wt/b");
    assert_eq!(app.tab_infos().len(), 2);
    app.repo_filter =
        gtasks::app::RepoFilter::One(gtasks::actions::RepoSlug::parse("me/a").expect("valid slug"));
    let infos = app.tab_infos();
    assert_eq!(infos.len(), 1, "only the repo's tab shows");
    assert_eq!(infos[0].index, 0, "the click still lands on the real tab");
    assert!(infos[0].label.contains('1'));
    // Cycling stays inside the filter.
    app.active_tab = Some(0);
    app.next_tab();
    assert_eq!(app.active_tab, Some(0), "nowhere else to go in this repo");
}

#[test]
fn drilling_into_a_repo_lands_on_its_tab() {
    let mut app = app_with_items();
    placed_tab(&mut app, "me/b", 2, "/wt/b");
    app.active_tab = None;
    // Down to me/b (row 0 is All, the filter follows the cursor),
    // drilled in with Enter.
    app.pane = gtasks::app::Pane::Repos;
    press(&mut app, Key::Down);
    press(&mut app, Key::Down);
    press(&mut app, Key::Enter);
    assert_eq!(app.pane, gtasks::app::Pane::List);
    assert_eq!(app.active_tab, Some(0), "the repo's tab activates");
}

#[test]
fn moving_to_an_empty_repo_never_shows_another_repos_issue() {
    // The screenshot: #1's tab open on me/a, cursor moved down to me/b.
    // The hidden tab must not own the detail from off-screen — the
    // filter's first tab (none here) takes over.
    let mut app = app_with_items();
    placed_tab(&mut app, "me/a", 1, "/wt/a1");
    app.active_tab = Some(0);
    // Empty me/b out, like the screenshot's repo with zero rows.
    app.items.retain(|i| i.repo != "me/b");
    // Down to me/a, down to me/b (row 0 is All): the filter follows.
    app.pane = gtasks::app::Pane::Repos;
    press(&mut app, Key::Down);
    press(&mut app, Key::Down);
    assert_eq!(app.active_tab, None, "no tab of me/b: none active");
    assert!(
        app.effective_item().is_none(),
        "the detail must go blank, not linger on #1"
    );
    assert!(app.tab_infos().is_empty());
    // And back up: me/a's tab takes over again by itself.
    press(&mut app, Key::Up);
    assert_eq!(app.active_tab, Some(0));
    assert_eq!(
        app.effective_item().map(|i| i.number),
        Some(1),
        "back on me/a, its tab owns the detail again"
    );
}

#[test]
fn closing_names_the_tab_you_land_on() {
    let mut app = app_with_items();
    placed_tab(&mut app, "me/a", 1, "/wt/a1");
    placed_tab(&mut app, "me/b", 2, "/wt/b");
    app.active_tab = Some(1);
    // Drilled into me/a: closing the hidden #2 lands on #1, and says so.
    app.repo_filter =
        gtasks::app::RepoFilter::One(gtasks::actions::RepoSlug::parse("me/a").expect("valid slug"));
    app.close_tab_or_record();
    assert_eq!(app.active_tab, Some(0));
    assert!(
        app.status.starts_with("closed tab — now on #1"),
        "status names the landing tab: {}",
        app.status
    );
}

#[test]
fn an_open_tab_marks_its_row_with_a_peach_t() {
    use gtasks::ui::{draw, view_for};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = app_with_items();
    press(&mut app, Key::Enter); // tab #1 opens
    let view = view_for(&app);
    assert_eq!(
        view.item_tabs,
        vec![true, false],
        "only #1 carries the mark"
    );
    // Plain icons: with a Nerd Font a glyph sits between the mark and the
    // number, without one the row reads exactly `T #1 one`.
    let previous = std::env::var_os("GTASKS_NERD");
    unsafe {
        std::env::set_var("GTASKS_NERD", "0");
    }
    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();
    unsafe {
        match previous {
            Some(value) => std::env::set_var("GTASKS_NERD", value),
            None => std::env::remove_var("GTASKS_NERD"),
        }
    }
    let buffer = terminal.backend().buffer().clone();
    let mut screen = String::new();
    for y in 0..12 {
        for x in 0..80 {
            screen.push_str(buffer[(x, y)].symbol());
        }
    }
    assert!(
        screen.contains("T #1 one"),
        "the mark reads in front of the issue: {screen}"
    );
    // The second row has no tab: no mark before its number.
    assert!(
        !screen.contains("T #2 two"),
        "rows without a tab stay plain: {screen}"
    );
}
