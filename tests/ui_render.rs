//! Task 10 shell: two columns, legend line, `?` overlay. Assertions read the
//! real `TestBackend` buffer; untrusted text is sanitised on the way in.

use gtasks::app::{App, Pane};
use gtasks::theme;
use gtasks::ui::{ViewModel, draw};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

fn render(app: &App, view: &ViewModel) -> String {
    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app, view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..12 {
        for x in 0..80 {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn sample() -> (App, ViewModel) {
    let app = App {
        pane: Pane::List,
        repos_len: 1,
        list_len: 2,
        ..App::default()
    };
    let view = ViewModel {
        repos: vec!["owner/repo".into()],
        items: vec!["#12 Add widget".into(), "#7 Fix crash".into()],
        detail: "Body line one\nBody line two".into(),
    };
    (app, view)
}

#[test]
fn frame_shows_two_columns_and_legend() {
    let (app, view) = sample();
    let screen = render(&app, &view);

    assert!(screen.contains("gtasks"), "outer title: {screen}");
    assert!(screen.contains("list"), "left box");
    assert!(screen.contains("detail"), "right box");
    assert!(screen.contains("owner/repo"), "repo row");
    assert!(screen.contains("(0)"), "repo row carries its count");
    assert!(screen.contains("#12 Add widget"), "item row");
    assert!(
        screen.contains("←/→ panes"),
        "the legend names the column keys: {screen}"
    );
    assert!(
        screen.contains("? help"),
        "the legend points at the overlay"
    );
}

#[test]
fn search_mode_shows_the_typed_query() {
    let (mut app, view) = sample();
    app.searching = true;
    app.query = "widget".into();
    let screen = render(&app, &view);
    assert!(
        screen.contains("/widget"),
        "query is shown while searching: {screen}"
    );
}

#[test]
fn untrusted_text_is_sanitised_before_display() {
    let (app, mut view) = sample();
    // The same bytes the model probe uses: an OSC-52 clipboard write and a
    // right-to-left override.
    view.items[0] = "evil\u{1b}]52;c;Zm9v\u{07} title\u{202e}".into();
    let screen = render(&app, &view);
    assert!(
        !screen.contains('\u{1b}'),
        "escape must never reach the buffer"
    );
    assert!(
        !screen.contains('\u{202e}'),
        "bidi override must never reach it"
    );
    assert!(
        screen.contains("evil]52;c;Zm9v title"),
        "the visible text survives; the escape and the bidi override do not: {screen}"
    );
}

#[test]
fn the_focused_list_highlights_its_selected_row() {
    let (mut app, view) = sample();
    app.list_cursor = 1;
    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();

    // One repo row, then the items: cursor 1 in List means the third row.
    // Repos box removed: the left box holds rows at y=2 (repo), y=3 (#12),
    // y=4 (#7). Only the selected row carries the selection background.
    let buffer = terminal.backend().buffer();
    let mut highlighted = Vec::new();
    for y in 2..11u16 {
        if buffer[(2, y)].bg == theme::SELECTED_BG {
            highlighted.push(y);
        }
    }
    assert_eq!(
        highlighted,
        vec![4],
        "exactly the selected row: {highlighted:?}"
    );
}

#[test]
fn the_detail_renders_markdown_instead_of_raw_markers() {
    let (app, mut view) = sample();
    view.detail = "## Summary\n\nUse **bold** and `code`.".to_string();

    let screen = render(&app, &view);
    assert!(screen.contains("Summary"), "heading text is shown");
    assert!(
        !screen.contains("##"),
        "markdown markers are gone: {screen}"
    );
    assert!(screen.contains("bold"));
    assert!(screen.contains("code"));
}

#[test]
fn help_overlay_lists_the_full_keymap() {
    // The overlay needs room: 14 keymap rows plus its border.
    let render_tall = |app: &App, view: &ViewModel| -> String {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app, view)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..24 {
            for x in 0..100 {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    };
    let (mut app, view) = sample();
    app.show_help = true;
    let plain = render(&App::default(), &view);
    assert!(!plain.contains("open the detail box"));

    let screen = render_tall(&app, &view);
    assert!(screen.contains("keys"), "overlay title: {screen}");
    assert!(
        screen.contains("open the detail box"),
        "full keymap: {screen}"
    );
    assert!(
        screen.contains("quit (with a confirmation"),
        "quit row: {screen}"
    );
    assert!(
        screen.contains("coming in the next stages:"),
        "future keys get their own section: {screen}"
    );
    assert!(
        screen.contains("repo selection menu (stage 11)"),
        "future keys name their stage: {screen}"
    );
    assert!(
        !screen.contains("drill into the repo"),
        "no claims about unimplemented drill-down: {screen}"
    );
}

#[test]
fn recovery_keys_survive_narrow_widths() {
    // S-1b: at 40 and 20 columns the legend must still show `q` and `?`,
    // because they are the two keys that explain and exit. They lead the
    // line, so truncation from the right takes the middle first.
    let render_narrow = |w: u16, h: u16| {
        let (app, view) = sample();
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
    };
    let legend = App::default().legend();
    assert!(
        legend.starts_with("q quit · ? help"),
        "recovery keys lead: {legend}"
    );
    for (w, h) in [(40, 12), (20, 8)] {
        let screen = render_narrow(w, h);
        assert!(screen.contains("q quit"), "{w}x{h}: q visible: {screen}");
        assert!(screen.contains("? help"), "{w}x{h}: ? visible: {screen}");
    }
}

#[test]
fn no_colour_literal_outside_the_theme_module() {
    // Task 10 invariant: `grep -rn 'Color::' src/ | grep -v theme.rs` is empty.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().unwrap() != "theme.rs"
            {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains("Color::") {
                    offenders.push(path);
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "colour literals must live in src/theme.rs only: {offenders:?}"
    );
    // The token itself resolves to the Mocha base colour.
    assert_eq!(theme::BASE, Color::Rgb(0x1e, 0x1e, 0x2e));
}
