//! Task 5: the frame is drawn from state + view data, and untrusted text is
//! sanitised on the way in (S13). Assertions read the real `TestBackend` buffer.

use gtasks::app::{App, Pane};
use gtasks::ui::{ViewModel, draw};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

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
fn frame_shows_header_panels_and_footer() {
    let (app, view) = sample();
    let screen = render(&app, &view);

    assert!(screen.contains("gtasks"), "header: {screen}");
    assert!(screen.contains("List"));
    assert!(screen.contains("repos"), "panel titles are visible");
    assert!(screen.contains("items"));
    assert!(screen.contains("detail"));
    assert!(screen.contains("owner/repo"));
    assert!(screen.contains("#12 Add widget"));
    assert!(
        screen.contains("/ search"),
        "the key hints live in the footer"
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

    // Row 1 is the header; the first item sits at y=2, the second at y=3.
    // The repos column is 30% of 80 = 24 cells, so x=25 is inside the list.
    let first = terminal.backend().buffer()[(25, 2)].modifier;
    let second = terminal.backend().buffer()[(25, 3)].modifier;
    assert!(
        second.contains(Modifier::REVERSED),
        "selected row is reversed"
    );
    assert!(!first.contains(Modifier::REVERSED), "other rows are not");
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
