//! The detail pane while an agent works a tree with no issue/PR under the
//! cursor: repo, branch and path instead of a blank pane.

use gtasks::app::{AgentContext, App};
use gtasks::ui::view_for;

fn render(app: &App, view: &gtasks::ui::ViewModel) -> String {
    use gtasks::ui::draw;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let backend = TestBackend::new(200, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app, view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..40 {
        for x in 0..200 {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn detail_shows_the_agent_tree_when_no_item_is_selected() {
    let app = App {
        items: Vec::new(),
        agent_running: true,
        agent_context: Some(AgentContext {
            repo: "meviusisback/omarchy-ente-auth-dropdown".to_string(),
            branch: "omarchy-ente-auth-dropdown-dev".to_string(),
            path: std::path::PathBuf::from(
                "/home/user/worktrees/meviusisback/omarchy-ente-auth-dropdown/omarchy--ente-auth-dropdown--dev",
            ),
        }),
        ..App::default()
    };
    let view = view_for(&app);
    let screen = render(&app, &view);
    assert!(
        screen.contains("meviusisback/omarchy-ente-auth-dropdown"),
        "the repo is named: {screen}"
    );
    assert!(
        screen.contains("omarchy-ente-auth-dropdown-dev"),
        "the branch is named: {screen}"
    );
    assert!(
        screen.contains("working on"),
        "the pane says what it is: {screen}"
    );
}

#[test]
fn detail_stays_blank_without_an_agent_tree() {
    let app = App::default();
    let view = view_for(&app);
    let screen = render(&app, &view);
    assert!(
        !screen.contains("working on"),
        "no agent, no context: {screen}"
    );
}
