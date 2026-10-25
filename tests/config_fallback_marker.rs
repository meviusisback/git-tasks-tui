//! The status line carries the **state** of the program, not only a hint.
//!
//! A broken config is announced once, by a notice the user dismisses with `Esc`. After that the
//! program keeps running on the demo fallback — a repository the user never chose — and nothing on
//! screen remembers why. The decision "say it once, don't repeat it" is right in the moment and
//! wrong in the duration: whichever came next in the list looks like a normal first run.
//!
//! So the marker is not a second message: it is the same fact the notice carries, in the one place
//! that is allowed to describe the program's state for as long as it holds.

use gtasks::app::App;

/// While the config has not loaded, the legend says so — after the notice is gone, and for as long
/// as the fallback is in use.
#[test]
fn the_legend_keeps_saying_demo_fallback() {
    let app = App {
        config_fallback: true,
        ..App::default()
    };
    assert!(
        app.legend().contains("demo fallback"),
        "the legend is the state line: it must say the app is on the fallback. legend was {:?}",
        app.legend()
    );
}

/// And the hinge: an app whose config loaded says nothing about a fallback. A marker that is always
/// there is the same lie as a notice that never goes away — it teaches the eye to skip the row.
#[test]
fn a_loaded_config_says_nothing_about_a_fallback() {
    let app = App {
        config_fallback: false,
        ..App::default()
    };
    assert!(
        !app.legend().contains("demo fallback"),
        "with a loaded config the legend must stay quiet about fallbacks: {:?}",
        app.legend()
    );
}

/// The notice carries the full message; the marker carries the state. They must not be the same
/// string, or the row would repeat the message — the defect the box was built to avoid.
#[test]
fn the_marker_is_shorter_than_the_message() {
    let app = App {
        config_fallback: true,
        ..App::default()
    };
    let legend = app.legend();
    assert!(
        !legend.contains("cannot read config") && !legend.contains("invalid config"),
        "the marker is a state, not the message: {legend:?}"
    );
}

/// The flag follows the load, not the past: a clean load starts clear, so a
/// marker can never outlive the problem that raised it. The only writer is
/// startup (`config_fallback: problem.is_some()`), which is why this pins
/// both ends available to tests — the default and the clean load.
#[test]
fn the_flag_follows_the_load_not_the_past() {
    assert!(
        !App::default().config_fallback,
        "a fresh app never claims a fallback"
    );
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("gtasks/config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "repos = [\"o/r\"]\n").unwrap();
    let (_, problem) = gtasks::config::load_or_demo(&path);
    assert!(
        problem.is_none(),
        "a clean load carries no problem, so the flag starts clear: {problem:?}"
    );
}

/// The string test above says `legend()` carries the marker. It cannot say the marker is **on the
/// screen**: the row has a fixed width, and a legend that fills it truncates the *tail* — so an
/// appended marker is invisible while the string test stays green. Measured: at 100 columns the
/// legend already ends at `R repos`, and at 80 it ends sooner.
///
/// This is the test that would have caught the append version, so it looks at the drawn row.
#[test]
fn the_marker_survives_the_row_at_eighty_columns() {
    use gtasks::ui::{ViewModel, draw};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let app = App {
        config_fallback: true,
        ..App::default()
    };
    let view = ViewModel::default();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &app, &view)).unwrap();

    let buffer = terminal.backend().buffer().clone();
    let mut rows = Vec::new();
    for y in 0..24 {
        let mut line = String::new();
        for x in 0..80 {
            line.push_str(buffer[(x, y)].symbol());
        }
        rows.push(line);
    }
    // The legend shares the last inner row with the status text.
    let bottom = &rows[22];
    assert!(
        bottom.contains("demo fallback"),
        "the marker must survive the truncation at 80 columns; the row reads {bottom:?}"
    );
}
