//! Task 5: the key map is pure, so it is tested without a terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gtasks::app::{Key, Msg};
use gtasks::input::{map_key, map_key_code};

#[test]
fn printable_keys_become_text() {
    assert_eq!(
        map_key_code(KeyCode::Char('q'), KeyModifiers::NONE),
        Some(Msg::Key(Key::Char('q')))
    );
    assert_eq!(
        map_key_code(KeyCode::Char('/'), KeyModifiers::SHIFT),
        Some(Msg::Key(Key::Char('/')))
    );
}

#[test]
fn navigation_and_editing_keys_are_mapped() {
    assert_eq!(
        map_key_code(KeyCode::Esc, KeyModifiers::NONE),
        Some(Msg::Key(Key::Esc))
    );
    assert_eq!(
        map_key_code(KeyCode::Enter, KeyModifiers::NONE),
        Some(Msg::Key(Key::Enter))
    );
    assert_eq!(
        map_key_code(KeyCode::Up, KeyModifiers::NONE),
        Some(Msg::Key(Key::Up))
    );
    assert_eq!(
        map_key_code(KeyCode::Down, KeyModifiers::NONE),
        Some(Msg::Key(Key::Down))
    );
    assert_eq!(
        map_key_code(KeyCode::Tab, KeyModifiers::NONE),
        Some(Msg::Key(Key::Tab))
    );
    assert_eq!(
        map_key_code(KeyCode::BackTab, KeyModifiers::SHIFT),
        Some(Msg::Key(Key::BackTab))
    );
    assert_eq!(
        map_key_code(KeyCode::Backspace, KeyModifiers::NONE),
        Some(Msg::Key(Key::Backspace))
    );
}

#[test]
fn ctrl_c_is_a_quit_signal_whatever_else_is_held() {
    assert_eq!(
        map_key_code(KeyCode::Char('c'), KeyModifiers::CONTROL),
        Some(Msg::Key(Key::CtrlC))
    );
    assert_eq!(
        map_key(KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )),
        Some(Msg::Key(Key::CtrlC))
    );
}

#[test]
fn other_control_chords_and_unknown_keys_are_ignored() {
    assert_eq!(
        map_key_code(KeyCode::Char('a'), KeyModifiers::CONTROL),
        None
    );
    assert_eq!(map_key_code(KeyCode::F(5), KeyModifiers::NONE), None);
    assert_eq!(map_key_code(KeyCode::Insert, KeyModifiers::NONE), None);
}
