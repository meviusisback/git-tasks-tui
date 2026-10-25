//! Task 17: a broken config shows on screen, with the remedy named.
//!
//! Two different failures, two different sentences: *unreadable* (the file
//! itself is broken) versus *invalid* (readable text, rejected value). The
//! demo fallback stays, but never silently — the notice modal stays until
//! dismissed, and quitting always works through it.

use gtasks::app::{App, Key, Modal, Msg};
use gtasks::config::{ConfigProblem, demo_config, load_or_demo};

fn write_config(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("gtasks/config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn a_missing_file_is_a_first_run_not_a_problem() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, problem) = load_or_demo(&tmp.path().join("nope/config.toml"));
    assert!(problem.is_none(), "nothing present, nothing to report");
    assert_eq!(config.repos, vec!["meviusisback/git-tasks-tui"]);
}

#[test]
fn garbage_is_unreadable_with_the_file_named() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_config(tmp.path(), "this is not toml = [");
    let (config, problem) = load_or_demo(&path);
    assert_eq!(
        config.repos,
        vec!["meviusisback/git-tasks-tui"],
        "demo fallback"
    );
    let problem = problem.expect("garbage must be reported");
    assert!(matches!(problem, ConfigProblem::Unreadable { .. }));
    let message = problem.message();
    assert!(message.starts_with("cannot read config"), "got: {message}");
    assert!(
        message.contains(&path.display().to_string()),
        "the file is named: {message}"
    );
}

#[test]
fn a_bad_value_is_invalid_with_value_and_range() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_config(tmp.path(), "repos = [\"o/r\"]\nparallel = 99\n");
    let (_, problem) = load_or_demo(&path);
    let problem = problem.expect("a bad value must be reported");
    assert!(matches!(problem, ConfigProblem::Invalid { .. }));
    let message = problem.message();
    assert!(message.starts_with("invalid config"), "got: {message}");
    assert!(
        message.contains("parallel must be 1..=8, got 99"),
        "value and accepted range: {message}"
    );
    assert!(
        message.contains(&path.display().to_string()),
        "the file is named: {message}"
    );
}

#[test]
fn an_unknown_key_is_invalid_not_unreadable() {
    // Readable TOML with a rejected meaning: the fix is the key, not the file.
    let tmp = tempfile::tempdir().unwrap();
    let path = write_config(tmp.path(), "repos = [\"o/r\"]\nclon_roots = []\n");
    let (_, problem) = load_or_demo(&path);
    let problem = problem.expect("an unknown key must be reported");
    assert!(
        matches!(problem, ConfigProblem::Invalid { .. }),
        "unknown keys are rejected content, not broken syntax: {problem:?}"
    );
}

#[test]
fn an_empty_repo_list_is_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_config(tmp.path(), "repos = []\n");
    let (_, problem) = load_or_demo(&path);
    let problem = problem.expect("an empty list must be reported");
    assert!(
        matches!(problem, ConfigProblem::Invalid { .. }),
        "an empty list reads fine and means nothing: {problem:?}"
    );
}

#[test]
fn a_good_file_reports_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_config(tmp.path(), "repos = [\"o/r\"]\n");
    let (config, problem) = load_or_demo(&path);
    assert!(problem.is_none());
    assert_eq!(config.repos, vec!["o/r"]);
}

#[test]
fn the_demo_fallback_is_valid_by_construction() {
    let demo = demo_config();
    assert!(demo.validate().is_ok(), "the fallback must load clean");
    assert!(
        !demo.repos.is_empty(),
        "a fallback with no repos shows nothing"
    );
}

#[test]
fn esc_dismisses_the_notice_and_q_quits_through_it() {
    let mut app = App {
        modal: Some(Modal::Notice("invalid config /tmp/x: nope".into())),
        ..App::default()
    };
    app.update(Msg::Key(Key::Char('r')));
    assert!(app.modal.is_some(), "other keys wait their turn");
    assert!(!app.refresh_requested, "nothing runs behind a notice");
    app.update(Msg::Key(Key::Esc));
    assert!(app.modal.is_none(), "Esc puts the notice away");
    assert!(!app.quit, "dismissing is not quitting");

    let mut app = App {
        modal: Some(Modal::Notice("invalid config /tmp/x: nope".into())),
        ..App::default()
    };
    app.update(Msg::Key(Key::Char('q')));
    assert!(app.quit, "quitting works even on top of bad news");
}
