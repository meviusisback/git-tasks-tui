//! The config notice at the width and height the project says it defends: 80x24.
//!
//! `draw_notice` is a centered box for a stated reason — "at 80 columns a long path would push
//! the remedy (Esc dismiss) off the edge, which is exactly the silence this notice replaces".
//! A centered box protects the remedy from a long *path*; it does not protect it from a long
//! *message*, because the message fills the box vertically and the remedy is the last line.
//!
//! The longest message this program can produce is a TOML parse error: `toml` repeats the
//! offending line, so a 400-character token gives a 997-character notice. At 80x24 the box
//! interior is 54x17, which fits about 17 lines of 54 columns — the remedy does not survive.

use gtasks::app::{App, Modal};
use gtasks::config::load_or_demo;
use gtasks::ui::{ViewModel, draw};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn screen_with_notice(notice: &str, w: u16, h: u16) -> String {
    let app = App {
        modal: Some(Modal::Notice(notice.to_string())),
        ..App::default()
    };
    let view = ViewModel::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// The property the box exists for, stated as a test: whatever the message, the way out is
/// on screen.
#[test]
fn the_remedy_survives_a_message_that_does_not_fit() {
    let longest = format!(
        "cannot read config {}: {}",
        "/tmp/x/config.toml",
        "x".repeat(950)
    );
    // Not an exact count: the point is that it cannot fit, and the count is measured below.
    println!("the notice is {} chars", longest.chars().count());
    assert!(
        longest.chars().count() > 54 * 17,
        "the fixture must be longer than the box interior (54x17 at 80x24)"
    );

    let screen = screen_with_notice(&longest, 80, 24);
    assert!(
        screen.contains("Esc dismiss"),
        "at 80x24 the remedy must stay on screen. screen was:\n{screen}"
    );
}

/// The other half: the message itself must be bounded at its source, so the box has something
/// to fit. A TOML parse error's head carries the file, the line and the column; the tail is the
/// offending line repeated — the head is what a person needs.
#[test]
fn a_toml_parse_error_is_bounded_and_keeps_the_diagnosis() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, format!("repos = [{}", "x".repeat(400))).expect("write");

    let (_config, problem) = load_or_demo(&path);
    let message = problem.expect("must be named").message();
    println!("message is {} chars: {message}", message.chars().count());

    assert!(
        message.chars().count() <= 220,
        "a notice that cannot fit must be elided, got {} chars",
        message.chars().count()
    );
    for needle in ["config.toml", "line", "column"] {
        assert!(
            message.contains(needle),
            "the elided message must keep {needle:?}: {message}"
        );
    }
    assert!(
        !message.contains("xxxx"),
        "the repeated offending line is the part to drop: {message}"
    );
}
