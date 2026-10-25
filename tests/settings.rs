//! `s`: the settings menu. Toggles flip now, the loop persists to
//! `config.toml` — the key handler never touches the disk itself.

use gtasks::app::{App, Key, Modal, Msg, Setting};

fn settings_app() -> App {
    App::default()
}

#[test]
fn s_opens_the_settings_menu_with_the_sandbox_row() {
    let mut app = settings_app();
    app.update(Msg::Key(Key::Char('s')));
    let Some(Modal::Settings(menu)) = app.modal.as_ref() else {
        panic!("s opens settings, got {:?}", app.modal);
    };
    assert_eq!(menu.rows, vec![Setting::SandboxAgents]);
    assert_eq!(menu.cursor, 0);
}

#[test]
fn enter_flips_the_flag_and_asks_the_loop_to_save() {
    let mut app = settings_app();
    app.update(Msg::Key(Key::Char('s')));
    app.update(Msg::Key(Key::Enter));
    assert!(app.sandbox_agents);
    assert!(app.config_save_requested, "the loop owns the write");
    assert_eq!(app.status, "sandboxed agents on");
    // The menu stays open on the new state: Esc closes it.
    assert!(matches!(app.modal, Some(Modal::Settings(_))));
    app.update(Msg::Key(Key::Enter));
    assert!(!app.sandbox_agents);
    assert_eq!(app.status, "sandboxed agents off");
    app.update(Msg::Key(Key::Esc));
    assert!(app.modal.is_none());
}

#[test]
fn space_flips_too_and_cursor_stays_inside() {
    let mut app = settings_app();
    app.update(Msg::Key(Key::Char('s')));
    app.update(Msg::Key(Key::Char(' ')));
    assert!(app.sandbox_agents);
    app.update(Msg::Key(Key::Down));
    app.update(Msg::Key(Key::Up));
    let Some(Modal::Settings(menu)) = app.modal.as_ref() else {
        panic!("still open");
    };
    assert_eq!(menu.cursor, 0, "one row: the cursor clamps, never leaves");
}

#[test]
fn q_quits_through_the_menu() {
    let mut app = settings_app();
    app.update(Msg::Key(Key::Char('s')));
    app.update(Msg::Key(Key::Char('q')));
    assert!(app.quit);
}

#[test]
fn the_toggle_roundtrips_through_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "repos = [\"o/r\"]\nsandbox_agents = true\n").unwrap();
    let mut config = gtasks::config::Config::load(&path).unwrap();
    assert!(config.sandbox_agents);
    config.sandbox_agents = false;
    config.save(&path).unwrap();
    assert!(!gtasks::config::Config::load(&path).unwrap().sandbox_agents);
}

fn render(app: &App) -> String {
    use gtasks::ui::{draw, view_for};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let view = view_for(app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..30 {
        for x in 0..200 {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_menu_shows_the_state_on_screen() {
    let mut app = settings_app();
    app.update(Msg::Key(Key::Char('s')));
    let off = render(&app);
    assert!(
        off.contains("sandboxed agents [off]"),
        "state on the row: {off}"
    );
    assert!(
        off.contains("sandboxed` wrappers"),
        "the description explains: {off}"
    );

    app.update(Msg::Key(Key::Enter));
    let on = render(&app);
    assert!(on.contains("sandboxed agents [on]"), "flipped: {on}");
}
