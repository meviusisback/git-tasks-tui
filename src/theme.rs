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

/// Unfocused pane border.
pub fn border() -> Style {
    Style::default().fg(BORDER)
}

/// Focused pane border.
pub fn border_focused() -> Style {
    Style::default().fg(FOCUSED)
}

/// Pane titles.
pub fn title(focused: bool) -> Style {
    Style::default().fg(if focused { FOCUSED } else { SUBTEXT })
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
