//! Catppuccin Mocha tokens in one module (catppuccin-mocha).
//!
//! Every colour in the interface comes from here; the rest of `src/` must
//! not name a colour literally (there is a test that greps for it).

use ratatui::style::{Color, Modifier, Style};

/// #1e1e2e — the app background and the base of unfocused chrome.
pub const BASE: Color = Color::Rgb(0x1e, 0x1e, 0x2e);
/// #181825 — inset panels and the help overlay.
pub const MANTLE: Color = Color::Rgb(0x18, 0x18, 0x25);
/// #cdd6f4 — primary text.
pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
/// #a6adc8 — secondary text: counts, legend, unfocused titles.
pub const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
/// #585b70 — the selected row.
pub const SELECTED_BG: Color = Color::Rgb(0x58, 0x5b, 0x70);
/// #b4befe — focused pane borders and titles.
pub const FOCUSED: Color = Color::Rgb(0xb4, 0xbe, 0xfe);
/// #45475a — unfocused pane borders.
pub const BORDER: Color = Color::Rgb(0x45, 0x47, 0x5a);
/// #a6e3a1 — ok / passing.
pub const OK: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
/// #f38ba8 — errors and destructive hints.
pub const ALERT: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
/// #f9e2af — warnings, pending states, the help overlay title.
pub const WARN: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
/// #cba6f7 — mauve: magenta terminal output in the embedded agent box.
pub const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
/// #89dceb — sky: cyan terminal output in the embedded agent box.
pub const SKY: Color = Color::Rgb(0x89, 0xdc, 0xeb);

/// Map an embedded-terminal colour index to the Mocha palette. Index 0
/// (black) reads as the secondary text instead of near-black: the agent box
/// sits on a dark background, and an unreadable colour is a lie about what
/// the agent printed. Indices 16-255 are the real xterm palette (6×6×6 cube
/// plus greyscale ramp): mapping them all to one grey painted Pi's empty
/// rows light grey instead of their real colour.
pub fn term_idx(index: u8) -> Color {
    match index {
        1 | 9 => ALERT,
        2 | 10 => OK,
        3 | 11 => WARN,
        4 | 12 => FOCUSED,
        5 | 13 => MAUVE,
        6 | 14 => SKY,
        7 | 15 => TEXT,
        // Black reads as secondary text (see above); bright black is its
        // natural grey partner.
        0 | 8 => SUBTEXT,
        16..=231 => {
            // 6×6×6 cube, levels 0, 95, 135, 175, 215, 255.
            const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let cube = index - 16;
            let r = LEVELS[(cube / 36) as usize];
            let g = LEVELS[((cube / 6) % 6) as usize];
            let b = LEVELS[(cube % 6) as usize];
            Color::Rgb(r, g, b)
        }
        // Greyscale ramp: 8, 18, …, 238.
        _ => {
            let level = 8 + 10 * (index - 232);
            Color::Rgb(level, level, level)
        }
    }
}

/// Map an embedded-terminal cell colour: `None` means the terminal default
/// (transparent here — the box keeps the app background).
pub fn term_color(color: crate::embedded::Palette) -> Option<Color> {
    match color {
        crate::embedded::Palette::Default => None,
        crate::embedded::Palette::Idx(index) => Some(term_idx(index)),
        crate::embedded::Palette::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

/// One embedded-terminal cell as a style. Reverse video renders as the
/// selection style: the box has no other way to say "inverted".
pub fn agent_cell(
    fg: crate::embedded::Palette,
    bg: crate::embedded::Palette,
    bold: bool,
    inverse: bool,
) -> Style {
    if inverse {
        return highlight();
    }
    let mut style = Style::default().fg(TEXT);
    if let Some(color) = term_color(fg) {
        style = style.fg(color);
    }
    if let Some(color) = term_color(bg) {
        style = style.bg(color);
    }
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

/// Unfocused pane border.
pub fn border() -> Style {
    Style::default().fg(BORDER)
}

/// Focused pane border.
pub fn border_focused() -> Style {
    Style::default().fg(FOCUSED)
}

/// Pane titles: brighter when focused, and bold so the title reads as a
/// title and the rows below as content.
pub fn title(focused: bool) -> Style {
    let base = Style::default().fg(if focused { FOCUSED } else { SUBTEXT });
    if focused {
        base.add_modifier(Modifier::BOLD)
    } else {
        base
    }
}

/// The selected row.
pub fn highlight() -> Style {
    Style::default()
        .fg(TEXT)
        .bg(SELECTED_BG)
        .add_modifier(Modifier::BOLD)
}

/// The bottom legend line.
pub fn legend() -> Style {
    Style::default().fg(SUBTEXT)
}

/// The status panel: brighter than the legend, because it is the app talking.
pub fn status() -> Style {
    Style::default().fg(TEXT)
}

/// Help overlay frame.
pub fn overlay() -> Style {
    Style::default().fg(WARN)
}

/// The worktree marker in the issues list: dimmer than the kind colour,
/// because it is metadata about the row, not the row itself.
pub fn worktree_marker() -> Style {
    Style::default().fg(SUBTEXT)
}

/// Error overlay frame: the config problem box. Same shape as help, the
/// error colour instead of the warning one — the kind is visible before
/// a word is read.
pub fn alert() -> Style {
    Style::default().fg(ALERT)
}

/// Help overlay body.
pub fn overlay_text() -> Style {
    Style::default().fg(TEXT).bg(MANTLE)
}
