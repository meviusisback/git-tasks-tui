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
    // Narrow the list to me/b, then bookmark #2 as well.
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
    // Switch back: the detail jumps to the bookmark even though the
    // list only shows me/b — and the repo selection is untouched.
    let cursor = app.repos_cursor;
    press(&mut app, Key::Char('t'));
    assert_eq!(app.active_tab, Some(0));
    assert_eq!(
        app.effective_item().map(|i| (i.repo.as_str(), i.number)),
        Some(("me/a", 1)),
        "switching tabs jumps across the filter"
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
        repo: "me/a".to_string(),
        number: 1,
    });
    app.tabs.push(Tab {
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
        repo: "me/a".to_string(),
        number: 1,
    });
    app.tabs.push(Tab {
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
    app.tab_agents.insert(
        ("me/a".to_string(), 1),
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
        ("me/a".to_string(), 1),
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
        ("me/a".to_string(), 1),
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
        ("me/a".to_string(), 1),
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
        ("me/a".to_string(), 1),
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
        "t/T tabs",  // bottom legend: how to move between tabs
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
        ("me/a".to_string(), 1),
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
