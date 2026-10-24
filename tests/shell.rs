//! Task 10 shell behaviour: arrows move between columns, `?` toggles the
//! overlay, the legend always shows the keys available right now.

use gtasks::app::{App, Key, Modal, Msg, Pane};

fn press(app: &mut App, key: Key) {
    app.update(Msg::Key(key));
}

#[test]
fn arrows_move_between_the_two_columns() {
    let mut app = App::default();
    assert_eq!(app.pane, Pane::List);

    press(&mut app, Key::Right);
    assert_eq!(app.pane, Pane::Detail, "right enters the detail column");
    press(&mut app, Key::Right);
    assert_eq!(app.pane, Pane::Detail, "right at the edge stays put");

    press(&mut app, Key::Left);
    assert_eq!(app.pane, Pane::List, "left goes back to the list column");
    press(&mut app, Key::Left);
    assert_eq!(app.pane, Pane::List, "left at the edge stays put");
}

#[test]
fn esc_in_detail_goes_back_to_the_list() {
    let mut app = App::default();
    press(&mut app, Key::Right);
    assert_eq!(app.pane, Pane::Detail);
    press(&mut app, Key::Esc);
    assert!(!app.quit, "Esc in detail goes back, it does not quit");
    assert_eq!(app.pane, Pane::List);
}

#[test]
fn question_mark_toggles_the_overlay_and_any_key_closes_it() {
    let mut app = App::default();
    press(&mut app, Key::Char('?'));
    assert!(app.show_help, "? opens the overlay");
    press(&mut app, Key::Char('?'));
    assert!(!app.show_help, "? again closes it");

    press(&mut app, Key::Char('?'));
    press(&mut app, Key::Down);
    assert!(!app.show_help, "any other key closes it");
    assert_eq!(app.list_cursor, 0, "and the key itself is swallowed");
}

#[test]
fn legend_names_the_keys_available_right_now() {
    let app = App::default();
    let legend = app.legend();
    for key in ["←/→", "↑/↓", "Enter", "R repos", "m actions", "? help", "q"] {
        assert!(legend.contains(key), "legend names {key}: {legend}");
    }
}

#[test]
fn esc_at_the_top_level_says_nothing_is_closed() {
    // It used to live in the legend. There it was 24 characters that pushed
    // every real status message off the right edge below ~110 columns (S-6a),
    // so it moved to the status panel, where the text may be as long as needed.
    let mut top = App::default();
    press(&mut top, Key::Esc);
    let status = top.status_text().unwrap_or_default();
    assert!(
        status.contains("Esc — nothing to close"),
        "top level says Esc has nothing to close: {status}"
    );
    assert!(
        !top.legend().contains("nothing to close"),
        "and the legend no longer carries it: {}",
        top.legend()
    );
    assert!(!top.quit, "Esc is never an exit");
    for state in [
        App {
            pane: Pane::Detail,
            ..App::default()
        },
        App {
            modal: Some(Modal::Confirm("merge PR #7?".into())),
            ..App::default()
        },
        App {
            searching: true,
            ..App::default()
        },
    ] {
        assert!(
            !state.legend().contains("nothing to close"),
            "no note where Esc does something: {}",
            state.legend()
        );
    }
}
