//! Translating terminal events into [`Msg`]: a pure function, so the key map
//! is testable without a terminal.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Key, Msg};

/// `None` means "not a key we use" — the caller ignores it.
pub fn map_key(event: KeyEvent) -> Option<Msg> {
    map_key_code(event.code, event.modifiers)
}

pub fn map_key_code(code: KeyCode, modifiers: KeyModifiers) -> Option<Msg> {
    if modifiers.contains(KeyModifiers::CONTROL) {
        // Ctrl-C must always be able to leave the app.
        return match code {
            KeyCode::Char('c') | KeyCode::Char('C') => Some(Msg::Key(Key::CtrlC)),
            _ => None,
        };
    }

    // Shift+arrows move between boxes; plain arrows move inside them. Only
    // the arrows get the shift treatment: Shift+letter stays a letter, so
    // `T` (previous tab) keeps working however the terminal reports it.
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    let key = match code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Up if shift => Key::ShiftUp,
        KeyCode::Down if shift => Key::ShiftDown,
        KeyCode::Left if shift => Key::ShiftLeft,
        KeyCode::Right if shift => Key::ShiftRight,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Backspace => Key::Backspace,
        _ => return None,
    };
    Some(Msg::Key(key))
}
