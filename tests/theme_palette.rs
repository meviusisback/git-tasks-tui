//! The embedded box must show the agent's real colours: mapping every
//! extended index to one grey painted Pi's empty rows light grey instead
//! of their real colour.

use gtasks::theme::{SUBTEXT, term_idx};
use ratatui::style::Color;

#[test]
fn extended_indices_map_to_the_real_xterm_palette() {
    // Pi's backgrounds, from its own output: navy and slate, both dark.
    assert_eq!(term_idx(17), Color::Rgb(0, 0, 95));
    assert_eq!(term_idx(67), Color::Rgb(95, 135, 175));
    // Cube corners and the greyscale ramp ends.
    assert_eq!(term_idx(16), Color::Rgb(0, 0, 0));
    assert_eq!(term_idx(231), Color::Rgb(255, 255, 255));
    assert_eq!(term_idx(232), Color::Rgb(8, 8, 8));
    assert_eq!(term_idx(255), Color::Rgb(238, 238, 238));
}

#[test]
fn black_stays_readable_instead_of_panicking() {
    // 0 used to share the fallback arm with the ramp: a bare `index - 232`
    // would underflow on it in debug builds.
    assert_eq!(term_idx(0), SUBTEXT);
    assert_eq!(term_idx(8), SUBTEXT);
}
