//! Task 5: the pure state machine — no terminal, no I/O.

use gtasks::app::{App, Key, Modal, Msg, Pane};

fn press(app: &mut App, key: Key) {
    app.update(Msg::Key(key));
}

#[test]
fn tab_cycles_panes_both_ways() {
    // Idle: the agent pane stays out of the cycle — cycling into an empty
    // instructions box is a surprise. `w` reaches it on purpose.
    let mut app = App::default();
    assert_eq!(app.pane, Pane::List);
    press(&mut app, Key::Tab);
    assert_eq!(app.pane, Pane::Repos);
    press(&mut app, Key::Tab);
    assert_eq!(app.pane, Pane::Detail);
    press(&mut app, Key::Tab);
    assert_eq!(app.pane, Pane::List, "no agent: back to the list");
    press(&mut app, Key::BackTab);
    assert_eq!(app.pane, Pane::Detail);
    press(&mut app, Key::BackTab);
    assert_eq!(app.pane, Pane::Repos);
    press(&mut app, Key::BackTab);
    assert_eq!(app.pane, Pane::List);
}

#[test]
fn tab_cycle_includes_the_agent_while_one_runs() {
    let mut app = App {
        agent_running: true,
        ..App::default()
    };
    assert_eq!(app.pane, Pane::List);
    press(&mut app, Key::Tab);
    assert_eq!(app.pane, Pane::Repos);
    press(&mut app, Key::Tab);
    assert_eq!(app.pane, Pane::Detail);
    press(&mut app, Key::Tab);
    assert_eq!(app.pane, Pane::Agent, "running: the box joins the cycle");
    press(&mut app, Key::Tab);
    assert_eq!(app.pane, Pane::List);
    press(&mut app, Key::BackTab);
    assert_eq!(app.pane, Pane::Agent);
}

#[test]
fn cursors_clamp_to_the_available_rows() {
    let mut app = App::default();
    app.set_lengths(3, 5);

    for _ in 0..10 {
        press(&mut app, Key::Down);
    }
    assert_eq!(app.list_cursor, 4, "clamped at the last row");
    for _ in 0..10 {
        press(&mut app, Key::Up);
    }
    assert_eq!(app.list_cursor, 0, "clamped at the first row");

    // An empty list keeps the cursor at zero.
    app.set_lengths(0, 0);
    press(&mut app, Key::Down);
    assert_eq!(app.list_cursor, 0);
    assert_eq!(app.repos_cursor, 0);
}

#[test]
fn detail_scroll_moves_only_in_the_detail_pane() {
    let mut app = App::default();
    press(&mut app, Key::Down);
    assert_eq!(
        app.detail_scroll, 0,
        "list pane moves a cursor, not the scroll"
    );

    press(&mut app, Key::Tab); // Repos
    press(&mut app, Key::Tab); // Detail
    press(&mut app, Key::Down);
    press(&mut app, Key::Down);
    assert_eq!(app.detail_scroll, 2);
    press(&mut app, Key::Up);
    assert_eq!(app.detail_scroll, 1);
    for _ in 0..5 {
        press(&mut app, Key::Up);
    }
    assert_eq!(app.detail_scroll, 0, "never negative");
}

#[test]
fn legend_while_running_names_typing_not_app_keys() {
    let app = App {
        pane: Pane::Agent,
        agent_running: true,
        ..App::default()
    };
    let legend = app.legend();
    assert!(
        legend.contains("Esc/Tab panes"),
        "focused + running: Tab leaves the box too: {legend}"
    );
    assert!(
        legend.contains("wheel scrolls"),
        "the wheel reads history, not input: {legend}"
    );
    assert!(
        !legend.contains("q quit"),
        "`q` types into the agent, it must not be promised: {legend}"
    );
}

#[test]
fn typing_builds_and_erases_the_query() {
    let mut app = App::default();
    press(&mut app, Key::Char('/')); // enter search
    for c in "widget".chars() {
        press(&mut app, Key::Char(c));
    }
    assert_eq!(app.query, "widget");
    press(&mut app, Key::Backspace);
    assert_eq!(app.query, "widge");
    press(&mut app, Key::Enter); // leave search, keep the query
    assert!(!app.searching);
    assert_eq!(app.query, "widge");
}

#[test]
fn esc_leaves_the_search_then_stays() {
    let mut app = App::default();
    press(&mut app, Key::Char('/'));
    press(&mut app, Key::Char('a'));
    press(&mut app, Key::Esc);
    assert!(!app.quit, "first Esc only leaves the search");
    assert!(!app.searching);
    assert!(app.query.is_empty());
    press(&mut app, Key::Esc);
    assert!(!app.quit, "Esc is never an exit");
}

#[test]
fn esc_never_quits_in_any_state() {
    // The property from the spec: no Esc, in no state, sets quit. The
    // exhaustive match on Pane in `on_key` makes a new pane a compile
    // error, so whoever adds one must extend this sweep.
    for pane in [Pane::List, Pane::Repos, Pane::Detail, Pane::Agent] {
        for modal in [false, true] {
            for searching in [false, true] {
                for help in [false, true] {
                    let mut app = App {
                        pane,
                        modal: modal.then(|| Modal::Confirm("x?".to_string())),
                        searching,
                        query: if searching {
                            "ab".to_string()
                        } else {
                            String::new()
                        },
                        show_help: help,
                        ..App::default()
                    };
                    press(&mut app, Key::Esc);
                    assert!(
                        !app.quit,
                        "Esc must never quit (pane={pane:?}, modal={modal}, searching={searching}, help={help})"
                    );
                }
            }
        }
    }
}

#[test]
fn esc_climbs_one_level_and_stops() {
    let mut app = App {
        pane: Pane::Detail,
        ..App::default()
    };
    press(&mut app, Key::Esc);
    assert_eq!(app.pane, Pane::List, "Detail goes back to the list");
    press(&mut app, Key::Esc);
    assert_eq!(app.pane, Pane::List, "already at the first level: stays");
    assert!(!app.quit);
}

#[test]
fn q_is_text_while_searching_and_quits_outside() {
    let mut app = App::default();
    press(&mut app, Key::Char('/'));
    press(&mut app, Key::Char('q'));
    assert!(!app.quit, "q while searching is text");
    assert_eq!(app.query, "q");

    press(&mut app, Key::Esc); // leave the search
    press(&mut app, Key::Char('q'));
    assert!(app.quit);
}

#[test]
fn modal_swallows_keys_and_resolves() {
    let mut app = App {
        modal: Some(Modal::Confirm("close issue?".to_string())),
        ..App::default()
    };

    press(&mut app, Key::Char('x'));
    assert_eq!(app.query, "", "typing is swallowed by the modal");

    press(&mut app, Key::Esc);
    assert!(app.modal.is_none(), "Esc cancels");
    assert_eq!(app.status, "");

    app.modal = Some(Modal::Confirm("merge?".to_string()));
    press(&mut app, Key::Char('y'));
    assert!(app.modal.is_none());
    // X-1: `y` used to answer "confirmed" while no action ran — the sentence
    // said what did *not* happen. Until the modal is reachable and calls
    // `execute`, it must say exactly that.
    assert!(
        app.status.contains("nothing to run"),
        "y must not claim success: {:?}",
        app.status
    );
}
