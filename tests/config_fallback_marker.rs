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
