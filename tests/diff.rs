//! `d` on a PR: the diff overlay, fetched once and cached.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gtasks::app::{App, Key, Modal, Msg};
use gtasks::fetch::{FetchError, diff_args, fetch_diff_with_caps};
use gtasks::gh::{Argv, Caps};
use gtasks::model::{Item, ItemKind};

fn item(repo: &str, number: u64, kind: ItemKind, head: Option<&str>) -> Item {
    Item {
        number,
        title: "t".to_string(),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        updated_at: "2026-10-20T10:00:00Z".into(),
        created_at: String::new(),
        url: format!("https://github.com/{repo}/pull/{number}"),
        head_ref_name: head.map(str::to_string),
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        closing_issues_references: Vec::new(),
        repo: repo.to_string(),
        kind,
    }
}

fn pr_app() -> App {
    let mut app = App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("o/r").expect("valid slug"),
    ]);
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![item("o/r", 7, ItemKind::Pr, Some("fix-foo"))],
        failures: Vec::new(),
    });
    app
}

fn press(app: &mut App, key: Key) {
    app.update(Msg::Key(key));
}

fn fake_gh(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("gh");
    std::fs::write(&path, format!("#!/bin/sh\ncat <<'DIFF'\n{body}\nDIFF\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn d_opens_the_overlay_and_queues_one_fetch() {
    let mut app = pr_app();
    press(&mut app, Key::Char('d'));
    assert!(
        matches!(app.modal, Some(Modal::Diff)),
        "d opens the diff, got {:?}",
        app.modal
    );
    assert_eq!(
        app.diff_requested,
        Some(("o/r".to_string(), 7)),
        "the loop fetches once"
    );
    // Reopening the same PR hits the cache: no second fetch.
    app.apply_diff("o/r", 7, Ok("+added\n context".to_string()));
    assert!(app.diff_requested.is_none());
    press(&mut app, Key::Esc);
    press(&mut app, Key::Char('d'));
    assert!(app.diff_requested.is_none(), "cached: no refetch");
    assert!(matches!(app.modal, Some(Modal::Diff)));
}

#[test]
fn d_refuses_issues_with_the_reason() {
    let mut app = pr_app();
    app.apply_outcome(gtasks::fetch::FetchOutcome {
        items: vec![item("o/r", 9, ItemKind::Issue, None)],
        failures: Vec::new(),
    });
    press(&mut app, Key::Char('d'));
    assert!(app.modal.is_none(), "no overlay without a diff");
    assert!(
        app.status.contains("no diff"),
        "the reason is on screen: {}",
        app.status
    );
}

#[test]
fn esc_enter_close_and_arrows_scroll() {
    let mut app = pr_app();
    app.apply_diff(
        "o/r",
        7,
        Ok((0..10)
            .map(|i| format!(" line {i}"))
            .collect::<Vec<_>>()
            .join("\n")),
    );
    press(&mut app, Key::Char('d'));
    press(&mut app, Key::Down);
    press(&mut app, Key::Down);
    assert_eq!(app.diff_scroll, 2);
    press(&mut app, Key::Up);
    assert_eq!(app.diff_scroll, 1);
    press(&mut app, Key::Up);
    press(&mut app, Key::Up);
    assert_eq!(app.diff_scroll, 0, "scrolling clamps at the top");
    press(&mut app, Key::Enter);
    assert!(app.modal.is_none(), "Enter closes too");
    press(&mut app, Key::Char('d'));
    press(&mut app, Key::Esc);
    assert!(app.modal.is_none(), "Esc closes");
}

#[test]
fn long_diffs_are_capped_with_the_total_kept() {
    let mut app = pr_app();
    let text = (0..250)
        .map(|i| format!("+line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.apply_diff("o/r", 7, Ok(text));
    assert_eq!(app.diff_total, 250);
    assert!(app.diff_truncated, "the overlay says what it hides");
    assert_eq!(app.diff_text.lines().count(), App::DIFF_MAX_LINES);
    assert!(app.diff_text.contains("+line 0"));
    assert!(!app.diff_text.contains("+line 249"));
}

#[test]
fn empty_and_failed_diffs_say_so() {
    let mut app = pr_app();
    app.apply_diff("o/r", 7, Ok(String::new()));
    assert_eq!(app.diff_text, "(no changes)");

    app.apply_diff("o/r", 7, Err(FetchError::Timeout));
    assert!(
        app.diff_text.contains("diff failed"),
        "got: {}",
        app.diff_text
    );
    assert!(app.diff_for.is_none(), "a failure never caches");
}

#[test]
fn a_late_answer_for_another_item_is_dropped() {
    let mut app = pr_app();
    app.diff_requested = Some(("o/r".to_string(), 7));
    app.apply_diff("o/r", 8, Ok("+other".to_string()));
    assert_ne!(
        app.diff_for,
        Some(("o/r".to_string(), 8)),
        "the late answer must not overwrite"
    );
    assert_eq!(
        app.diff_requested,
        Some(("o/r".to_string(), 7)),
        "the real request stays pending"
    );
}

#[test]
fn the_argv_asks_for_the_diff_of_that_number_only() {
    let args = diff_args("acme/widget", 42);
    assert_eq!(args[0], "pr");
    assert_eq!(args[1], "diff");
    assert_eq!(args[2], "42");
    assert!(
        args.windows(2)
            .any(|w| w[0] == "--repo" && w[1] == "acme/widget"),
        "scoped to the repo: {args:?}"
    );
}

#[test]
fn the_diff_comes_back_as_plain_text() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_gh(dir.path(), "diff --git a/f b/f\n+added\n-removed");
    let argv = Argv {
        program: program.clone(),
        args: diff_args("acme/widget", 12),
        env: vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))],
        cwd: std::env::temp_dir(),
    };
    let text = fetch_diff_with_caps(
        &argv,
        "acme/widget",
        Duration::from_secs(10),
        Caps::default(),
    )
    .expect("the fake gh answers");
    assert!(text.contains("+added"));
    assert!(text.contains("-removed"));
}

fn render(app: &App) -> (String, Vec<Vec<ratatui::style::Style>>) {
    use gtasks::ui::{draw, view_for};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let view = view_for(app);
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    let mut styles = Vec::new();
    for y in 0..30 {
        let mut row = Vec::new();
        for x in 0..100 {
            out.push_str(buffer[(x, y)].symbol());
            row.push(buffer[(x, y)].style());
        }
        styles.push(row);
        out.push('\n');
    }
    (out, styles)
}

#[test]
fn the_overlay_colours_additions_and_removals() {
    use ratatui::style::Color;
    let mut app = pr_app();
    app.apply_diff("o/r", 7, Ok("+added\n context\n-removed".to_string()));
    press(&mut app, Key::Char('d'));
    let (screen, styles) = render(&app);
    assert!(screen.contains("o/r#7"), "title names the PR: {screen}");
    assert!(screen.contains("+added"));
    assert!(screen.contains("-removed"));
    let green = styles
        .iter()
        .flatten()
        .any(|s| s.fg == Some(Color::Rgb(0xa6, 0xe3, 0xa1)));
    let red = styles
        .iter()
        .flatten()
        .any(|s| s.fg == Some(Color::Rgb(0xf3, 0x8b, 0xa8)));
    assert!(green, "added lines are green");
    assert!(red, "removed lines are red");
}
