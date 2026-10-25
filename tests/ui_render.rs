//! Task 10 shell: two columns, legend line, `?` overlay. Assertions read the
//! real `TestBackend` buffer; untrusted text is sanitised on the way in.

use gtasks::app::{App, Key, Msg, Pane};
use gtasks::theme;
use gtasks::ui::{ViewModel, draw, view_for};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

fn render(app: &App, view: &ViewModel) -> String {
    render_sized(app, view, 80, 12)
}

fn render_sized(app: &App, view: &ViewModel, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app, view)).unwrap();
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
        tabs: Vec::new(),
        active_tab: None,
        agent: "g create worktree + launch agent".into(),
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

    // Left column is split: repos on top (3 rows: border, 1 row, border),
    // issues below starting at y=4. Cursor 1 in the issues box means the
    // second issue row (y=6). Only that row carries the selection
    // background; the repos box shows no highlight while the list is
    // focused.
    let buffer = terminal.backend().buffer();
    let mut highlighted = Vec::new();
    for y in 2..11u16 {
        if buffer[(2, y)].bg == theme::SELECTED_BG {
            highlighted.push(y);
        }
    }
    assert_eq!(
        highlighted,
        vec![6],
        "exactly the selected issue row: {highlighted:?}"
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
    assert!(screen.contains("q          quit"), "quit row: {screen}");
    assert!(
        screen.contains("Ctrl-C"),
        "the escape hatch is documented: {screen}"
    );
    assert!(
        screen.contains("coming in the next stages:"),
        "future keys get their own section: {screen}"
    );
    assert!(
        screen.contains("m          actions menu"),
        "m is available today: {screen}"
    );
    assert!(
        !screen.contains("actions menu (stage"),
        "m is not promised for later anymore: {screen}"
    );
    assert!(
        screen.contains("R          repo selection menu"),
        "R is available today: {screen}"
    );
    assert!(
        !screen.contains("repo selection menu (stage"),
        "R is not promised for later anymore: {screen}"
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
    // The note is no longer in the legend (S-6a: 24 characters there pushed
    // every status message off the edge below ~110 columns). It appears as a
    // status when Esc has nothing to close.
    let (mut app, view) = sample();
    app.update(Msg::Key(Key::Esc));
    let wide = render_sized(&app, &view, 100, 24);
    assert!(
        wide.contains("Esc — nothing to close"),
        "full width: the Esc note is in the status panel: {wide}"
    );
}

#[test]
fn status_messages_survive_at_80_columns() {
    // S-6a repro: at 80 columns the status used to drown behind a ~100-cell
    // legend. `r` is the honest path — it sets a real message, no fields.
    let (mut app, view) = sample();
    app.update(Msg::Key(Key::Char('r')));
    assert_eq!(app.status, "refreshing…");
    let screen = render(&app, &view);
    assert!(
        screen.contains("refreshing…"),
        "80 cols must show the status: {screen}"
    );
}

#[test]
fn detail_box_shows_structured_fields_at_80_columns() {
    // 80 columns is the decisive width: a field the test only renders at 120
    // certifies a screen that was never broken. Stable lines only — the
    // relative clock is pinned in `detail_fields` unit tests, not here.
    use gtasks::model::{Author, Check, Item, ItemKind, Label};
    let slug = gtasks::actions::RepoSlug::parse("o/r").expect("valid slug");
    let mut app = App {
        repos: vec![gtasks::app::RepoRow::All, gtasks::app::RepoRow::Repo(slug)],
        ..App::default()
    };
    app.items = vec![Item {
        number: 12,
        title: "Add widget".to_string(),
        state: "OPEN".to_string(),
        is_draft: None,
        author: Some(Author {
            login: "alberto".to_string(),
        }),
        labels: vec![Label {
            name: "bug".to_string(),
            color: None,
        }],
        assignees: vec![],
        created_at: String::new(),
        updated_at: String::new(),
        url: "https://github.com/o/r/pull/12".to_string(),
        head_ref_name: Some("feat/x".to_string()),
        base_ref_name: Some("main".to_string()),
        review_decision: None,
        mergeable: Some("MERGEABLE".to_string()),
        status_check_rollup: vec![Check {
            name: Some("ci".to_string()),
            status: Some("COMPLETED".to_string()),
            conclusion: Some("FAILURE".to_string()),
        }],
        closing_issues_references: vec![],
        repo: "o/r".to_string(),
        kind: ItemKind::Pr,
    }];
    let view = view_for(&app);
    // One size: 80x24. 80 columns is the decisive width (a field rendered
    // only at 120 certifies a screen that was never broken); 24 rows hold
    // the whole box, so this test watches the detail pane, all of it.
    // Stable lines only — the relative clock is pinned in `detail_fields`
    // unit tests, not here. No body is loaded, so the pane must say how to
    // fetch it instead of leaving a blank zone.
    let screen = render_sized(&app, &view, 80, 24);
    for line in [
        "state: OPEN",
        "author: alberto",
        "labels: bug",
        "feat/x → main",
        "checks: 0 passed, 1 failed (ci)",
        "mergeable: MERGEABLE",
        "url: https://github.com/o/r/pull/12",
        "Enter — body",
    ] {
        assert!(screen.contains(line), "80x24 shows {line}: {screen}");
    }
}

#[test]
fn a_body_belonging_to_another_item_is_not_shown() {
    // Moving the cursor must not drag the old body under the new fields:
    // the pane shows this item's body, or how to fetch it — never another
    // item's text.
    use gtasks::model::{Author, Item, ItemKind};
    let slug = gtasks::actions::RepoSlug::parse("o/r").expect("valid slug");
    let mut app = App {
        repos: vec![gtasks::app::RepoRow::All, gtasks::app::RepoRow::Repo(slug)],
        ..App::default()
    };
    let mk = |number: u64| Item {
        number,
        title: format!("t{number}"),
        state: "OPEN".to_string(),
        is_draft: None,
        author: Some(Author {
            login: "alberto".to_string(),
        }),
        labels: vec![],
        assignees: vec![],
        created_at: String::new(),
        updated_at: String::new(),
        url: String::new(),
        head_ref_name: None,
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: vec![],
        closing_issues_references: vec![],
        repo: "o/r".to_string(),
        kind: ItemKind::Pr,
    };
    app.items = vec![mk(1), mk(2)];
    app.detail = "stale body".to_string();
    app.detail_for = Some(("o/r".to_string(), 1));
    app.list_cursor = 1; // looking at #2, whose body was never fetched
    let view = view_for(&app);
    assert!(
        !view.detail.contains("stale body"),
        "another item's body leaked: {}",
        view.detail
    );
    assert!(view.detail.contains("Enter — body"));
}

#[test]
fn the_action_menu_fits_at_80_columns() {
    // The menu is a choice, not a surprise: all six rows must be visible at
    // the decisive width, with the method and the branch flag readable.
    use gtasks::actions::RepoSlug;
    use gtasks::model::{Item, ItemKind};
    let slug = RepoSlug::parse("o/r").expect("valid slug");
    let mut app = App {
        repos: vec![gtasks::app::RepoRow::All, gtasks::app::RepoRow::Repo(slug)],
        ..App::default()
    };
    app.items = vec![Item {
        number: 7,
        title: "seven".to_string(),
        state: "OPEN".to_string(),
        is_draft: None,
        author: None,
        labels: vec![],
        assignees: vec![],
        created_at: String::new(),
        updated_at: String::new(),
        url: String::new(),
        head_ref_name: None,
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: vec![],
        closing_issues_references: vec![],
        repo: "o/r".to_string(),
        kind: ItemKind::Pr,
    }];
    app.update(Msg::Key(Key::Char('m')));
    assert!(matches!(app.modal, Some(gtasks::app::Modal::ActionMenu(_))));
    let view = view_for(&app);
    let screen = render_sized(&app, &view, 80, 24);
    for line in [
        "merge — squash",
        "merge — rebase + delete branch",
        "↑/↓ choose · Enter confirm · Esc cancel",
        "▸ merge — squash",
    ] {
        assert!(screen.contains(line), "80x24 shows {line}: {screen}");
    }
}

#[test]
fn the_agent_record_shows_at_80_columns() {
    // The record card replaces the detail for its own item — finished and
    // running alike. "Running" can only ever be seen here in a test: on a
    // real terminal the agent owns the screen while it is alive.
    use gtasks::actions::RepoSlug;
    use gtasks::agent::{AgentOutcome, AgentRecord, AgentStatus};
    use gtasks::model::{Item, ItemKind};
    let slug = RepoSlug::parse("o/r").expect("valid slug");
    let mut app = App {
        repos: vec![gtasks::app::RepoRow::All, gtasks::app::RepoRow::Repo(slug)],
        ..App::default()
    };
    app.items = vec![Item {
        number: 7,
        title: "seven".to_string(),
        state: "OPEN".to_string(),
        is_draft: None,
        author: None,
        labels: vec![],
        assignees: vec![],
        created_at: String::new(),
        updated_at: String::new(),
        url: String::new(),
        head_ref_name: None,
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: vec![],
        closing_issues_references: vec![],
        repo: "o/r".to_string(),
        kind: ItemKind::Pr,
    }];
    app.agent_record = Some(AgentRecord {
        repo: "o/r".to_string(),
        number: 7,
        profile: "default".to_string(),
        worktree: std::path::PathBuf::from("/tmp/wt/o/r/fix-foo"),
        seed_lines: 4,
        status: AgentStatus::Finished {
            outcome: AgentOutcome::Exit(0),
            secs: 42,
        },
    });
    let view = view_for(&app);
    let screen = render_sized(&app, &view, 80, 24);
    for line in [
        "agent · o/r#7",
        "profile default · seed 4 lines",
        "worktree /tmp/wt/o/r/fix-foo",
        "exit 0 · 42 s",
    ] {
        assert!(screen.contains(line), "80x24 shows {line}: {screen}");
    }
    app.agent_record = Some(AgentRecord {
        repo: "o/r".to_string(),
        number: 7,
        profile: "default".to_string(),
        worktree: std::path::PathBuf::from("/tmp/wt/o/r/fix-foo"),
        seed_lines: 4,
        status: AgentStatus::Running,
    });
    let view = view_for(&app);
    let screen = render_sized(&app, &view, 80, 24);
    assert!(
        screen.contains("running…"),
        "the live state has its own word: {screen}"
    );
}

#[test]
fn the_config_notice_shows_on_screen_until_dismissed() {
    // The fallback must never be silent: an unreadable and an invalid file
    // show different sentences in the legend row, at the decisive width.
    for (notice, first_words) in [
        (
            "cannot read config /tmp/x/config.toml: expected `.`, `=`",
            "cannot read",
        ),
        (
            "invalid config /tmp/x/config.toml: parallel must be 1..=8, got 99",
            "invalid config",
        ),
    ] {
        let mut app = App::default();
        app.modal = Some(gtasks::app::Modal::Notice(notice.to_string()));
        let view = view_for(&app);
        let screen = render_sized(&app, &view, 80, 24);
        assert!(
            screen.contains(first_words) && screen.contains("Esc dismiss"),
            "the notice stays on screen with its remedy: {screen}"
        );
        assert!(
            screen.contains("config problem"),
            "the blocker is a box, not just a legend row: {screen}"
        );
        // The legend row (second to last) keeps only the remedy: the message
        // lives in the box, and a second copy would truncate mid-word and
        // add nothing.
        let rows: Vec<&str> = screen.lines().collect();
        let legend = rows.get(rows.len() - 2).expect("legend row");
        assert!(
            !legend.contains(first_words),
            "the legend row must not repeat the message: {legend}"
        );
        assert!(
            legend.contains("Esc dismiss"),
            "the remedy stays visible: {legend}"
        );
    }
}

#[test]
fn coming_next_keys_produce_no_screen_change() {
    // The auditor's proof: every key in the overlay's "coming in the next
    // stages" section must be a no-op today. The keys come from the same
    // PLANNED list the overlay formats — a fourth promise is watched from
    // the day it is added, not from the day someone remembers this test.
    // When a later stage binds one of them, this test fails — the overlay
    // section must shrink then, not keep promising what the screen already
    // does. (`R` left this list the day the repo menu landed.)
    for (key, _) in gtasks::ui::PLANNED {
        let key = Key::Char(*key);
        let (mut app, view) = sample();
        let before = render(&app, &view);
        let before_wide = render_sized(&app, &view, 120, 30);
        let before_status = app.status.clone();
        app.update(Msg::Key(key));
        assert!(!app.quit, "{key:?} must not quit");
        assert!(app.modal.is_none(), "{key:?} must not open anything");
        assert!(!app.show_help, "{key:?} must not open the overlay");
        assert_eq!(
            app.status, before_status,
            "{key:?} must not touch the status"
        );
        let after = render(&app, &view);
        assert_eq!(before, after, "{key:?} must not change the screen");
        let after_wide = render_sized(&app, &view, 120, 30);
        assert_eq!(
            before_wide, after_wide,
            "{key:?} must not change the screen at full width either"
        );
    }
}

#[test]
fn no_colour_literal_outside_the_theme_module() {
    // Task 10 invariant: no `Color::` literal outside `theme.rs`.
    //
    // One pattern, used by the scan AND by the anti-vacuity guard: if the
    // pattern is renamed or the theme switches to another colour type, the scan
    // finds nothing everywhere and would happily report "all clear". The guard
    // requires the scan to find the literals where they must live first.
    const PATTERN: &str = "Color::";
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut in_theme = 0usize;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            if !text.contains(PATTERN) {
                continue;
            }
            if path.file_name().is_some_and(|n| n == "theme.rs") {
                in_theme += 1;
            } else {
                offenders.push(path);
            }
        }
    }
    assert!(
        in_theme > 0,
        "the scan found no {PATTERN} even in theme.rs — the pattern is wrong, not the tree"
    );
    assert!(
        offenders.is_empty(),
        "colour literals outside the theme module: {offenders:?}"
    );
    // The token resolves to the Mocha base colour: the theme is not just
    // present, it carries the value the design names.
    assert_eq!(theme::BASE, Color::Rgb(0x1e, 0x1e, 0x2e));
}

#[test]
fn the_status_is_visible_at_eighty_columns() {
    // S-6a: the status used to be a suffix of the legend string, rendered into
    // one row: at 80 columns it fell past the right edge, so every honest
    // message of the app ("nothing selected", "it may have been applied") was
    // invisible on an ordinary terminal. It has its own panel now.
    let (mut app, view) = sample();
    app.update(Msg::Key(Key::Char('a')));
    let screen = render_sized(&app, &view, 80, 24);
    assert!(
        screen.contains("nothing selected"),
        "the status must be readable at 80 columns: {screen}"
    );
}
