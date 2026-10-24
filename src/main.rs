//! gtasks — multi-repo GitHub issues & PRs in the terminal.
//!
//! Thin wrapper: logging, terminal lifetime, the event loop, and nothing else.
//! All state lives in `gtasks::app`, all drawing in `gtasks::ui`.

#![deny(clippy::print_stdout)]

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::Parser;
use crossterm::event::{self, Event};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use gtasks::app::{App, Msg};
use gtasks::input::map_key;
use gtasks::logging::{init_file_logging, init_stderr_logging};
use gtasks::ui::{self, ViewModel};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// gtasks: GitHub issues & PRs across repos, in the terminal.
#[derive(Debug, Parser)]
#[command(name = "gtasks", version, about)]
struct Args {}

/// Set from a signal handler, so it must be a plain atomic store.
static TERMINATE: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_signal: libc::c_int) {
    TERMINATE.store(true, Ordering::SeqCst);
}

/// SIGTERM and SIGHUP would otherwise leave the terminal in raw mode.
fn install_signal_handlers() {
    // SAFETY: the handler only stores into an atomic, which is async-signal-safe.
    // The cast goes through a raw pointer because a bare function-item-to-integer
    // cast is deprecated (function_casts_as_integer).
    unsafe {
        let handler = on_signal as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGHUP, handler);
    }
}

/// Restores the terminal on drop: normal exit, `?` and panics all go through it.
struct TuiGuard;

impl TuiGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TuiGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
    }
}

fn main() -> anyhow::Result<()> {
    let _args = Args::parse();

    match init_file_logging() {
        Ok(path) => {
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                log = %path.display(),
                "gtasks starting"
            );
        }
        Err(err) => {
            eprintln!("gtasks: file logging unavailable ({err:#}); logging to stderr");
            init_stderr_logging();
        }
    }

    run_tui()
}

fn run_tui() -> anyhow::Result<()> {
    install_signal_handlers();

    // A panic must not leave the user with an unusable terminal.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
        previous_hook(info);
    }));

    let _guard = TuiGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let view = demo_view();
    let mut app = App::default();
    app.set_lengths(view.repos.len(), view.items.len());

    while !app.quit && !TERMINATE.load(Ordering::SeqCst) {
        terminal.draw(|frame| ui::draw(frame, &app, &view))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(msg) = map_key(key) {
                        app.update(msg);
                    }
                }
                // Resizes only need a redraw, which the loop does anyway.
                Event::Resize(_, _) => {}
                _ => {}
            }
        } else {
            app.update(Msg::Tick);
        }
    }

    Ok(())
}

/// Placeholder data until the fetch layer is wired: it proves the frame, the
/// key map and the terminal lifetime all work in a real terminal.
fn demo_view() -> ViewModel {
    ViewModel {
        repos: vec![
            "meviusisback/git-tasks-tui".to_string(),
            "meviusisback/omarchy-bitwarden-dropdown".to_string(),
        ],
        items: vec![
            "#12 Add the repository selector".to_string(),
            "#7 Fix the worktree path collision".to_string(),
            "#3 Sanitise titles before rendering".to_string(),
        ],
        detail: "Demo content.\n\nReal data arrives when the fetch layer lands.".to_string(),
    }
}
