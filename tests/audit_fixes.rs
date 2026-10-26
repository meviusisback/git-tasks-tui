//! Regression tests for the security-audit fix round (2026-10-25).
//!
//! Each test fails on the pre-fix code: the sanitize calls removed, the
//! untrusted transport used for deletes, the open allowlist, the raw
//! `fs::write` — and passes after. They are the teeth the audit asked for.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

// --- 1. Worktree deletion executes through the typed path ------------------

fn fake_git(dir: &Path) -> PathBuf {
    let path = dir.join("git");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}/argv.txt\nexit 0\n",
            dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn worktree_delete_executes_and_carries_force() {
    use gtasks::worktree::{WorktreeDelete, remove_tree};
    let tmp = tempfile::tempdir().unwrap();
    let git = fake_git(tmp.path());
    let binaries = gtasks::binaries::Binaries::new(PathBuf::from("/bin/false"), git);
    let target = tmp.path().join("tree-x");
    std::fs::create_dir_all(&target).unwrap();

    let order = WorktreeDelete {
        clone: tmp.path().to_path_buf(),
        branch: gtasks::worktree::BranchName::parse("fix-x").unwrap(),
        path: target,
    };
    // The production path: the same `remove_tree` the TUI calls.
    remove_tree(&binaries, &order).expect("delete runs");

    let seen = std::fs::read_to_string(tmp.path().join("argv.txt")).unwrap();
    assert!(
        seen.lines().any(|l| l == "--force"),
        "the --force the gate refuses on the untrusted path arrives here: {seen}"
    );
}

/// The untrusted path still refuses `--force`: the typed path is the only
/// door for it, not an excuse to loosen the gate.
#[test]
fn untrusted_path_still_refuses_force() {
    use gtasks::gh::run_pipe;
    use gtasks::worktree::remove_argv;
    let tmp = tempfile::tempdir().unwrap();
    let git = fake_git(tmp.path());
    let binaries = gtasks::binaries::Binaries::new(PathBuf::from("/bin/false"), git);
    let argv = remove_argv(&binaries, tmp.path(), tmp.path());
    let err = run_pipe(&argv, Duration::from_secs(5))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("refusing forbidden flag"),
        "untrusted delete still fails closed: {err}"
    );
}

// --- 2. The `gh` allowlist ---------------------------------------------------

fn gh_argv(args: &[&str]) -> gtasks::gh::Argv {
    use gtasks::gh::gh;
    // The fake is named `gh` so the gate keys on it; it exits 99 so that a
    // spawn (gate failure) is distinguishable from a refusal.
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("gh");
    std::fs::write(&fake, "#!/bin/sh\nexit 99\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Leak the dir: the argv only borrows the path at build time, but the
    // refusal happens before any spawn, so the file only needs to exist
    // until `run_pipe` returns — and keeping it alive avoids a racy unlink.
    let _dir = dir.keep();
    let binaries = gtasks::binaries::Binaries::new(fake, PathBuf::from("/bin/false"));
    gh(
        &binaries,
        Path::new("/tmp"),
        args.iter().map(OsString::from).collect(),
    )
}

#[test]
fn gh_allowlist_refuses_new_and_old_write_verbs() {
    use gtasks::gh::run_pipe;
    // A bare program is not `gh` on PATH here — but the gate fires before any
    // spawn, on the argv alone. Point at a missing binary: refusal must come
    // first, spawn never happens.
    for argv in [
        gh_argv(&["extension", "exec", "--", "id"]),
        gh_argv(&["auth", "status"]),
        gh_argv(&["config", "get", "x"]),
        gh_argv(&["secret", "list"]),
        gh_argv(&["pr", "comment", "1", "--body", "x"]),
        gh_argv(&["api", "/repos/cli/cli"]),
    ] {
        let err = run_pipe(&argv, Duration::from_secs(5))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("refusing gh subcommand"),
            "must fail closed before spawning: {err}"
        );
    }
}

// --- 3. Detail pane: no bidi/Cf reaches the screen --------------------------

fn hostile_item() -> gtasks::model::Item {
    use gtasks::model::{ClosingRef, Item, ItemKind, Label};
    Item {
        number: 7,
        title: "fix \u{202e}reversed".to_string(),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: vec![Label {
            name: "x\u{2028}y".to_string(),
            color: None,
        }],
        assignees: Vec::new(),
        updated_at: "2026-10-20T10:00:00Z".into(),
        created_at: String::new(),
        url: "https://github.com/acme/one/pull/7".into(),
        head_ref_name: Some("feat-\u{202e}spoof".to_string()),
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        closing_issues_references: vec![ClosingRef {
            number: Some(1),
            title: Some("#1 \u{202e}linked".to_string()),
        }],
        repo: "acme/one".to_string(),
        kind: ItemKind::Pr,
    }
}

fn render_screen(app: &gtasks::app::App, view: &gtasks::ui::ViewModel) -> String {
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
fn detail_title_fields_and_links_carry_no_bidi() {
    use gtasks::app::App;
    use gtasks::fetch::FetchOutcome;
    use gtasks::ui::view_for;
    let mut app = App::default();
    app.apply_outcome(FetchOutcome {
        items: vec![hostile_item()],
        failures: Vec::new(),
    });
    let view = view_for(&app);
    let screen = render_screen(&app, &view);
    for bad in ['\u{202e}', '\u{2028}', '\u{2029}', '\u{feff}', '\u{2066}'] {
        assert!(
            !screen.contains(bad),
            "U+{:04X} reached the terminal: {screen}",
            bad as u32
        );
    }
    assert!(screen.contains("#7"), "the item still renders: {screen}");
}

// --- 4. Markdown link URLs are sanitised ------------------------------------

#[test]
fn markdown_link_url_cannot_reorder_the_line() {
    use gtasks::markdown::{line_text, render};
    let lines = render("See [docs](https://example.com/o/r\u{202e}).", 80);
    let joined = line_text(&lines[0]);
    assert!(joined.contains("docs"), "text stays: {joined}");
    assert!(!joined.contains('\u{202e}'), "RLO scrubbed: {joined}");
    assert!(
        joined.contains("https://example.com/"),
        "url shown: {joined}"
    );
}

// --- 5. Garbage dates map to None, never panic -------------------------------

#[test]
fn a_huge_year_is_none_not_a_panic() {
    assert_eq!(
        gtasks::model::relative_time(1_791_000_000, "99999999999999999-01-01T00:00:00Z"),
        None,
        "era*146097 overflows i64: must be None in every profile"
    );
    assert!(
        gtasks::model::relative_time(1_791_000_000, "2026-01-01T00:00:00Z").is_some(),
        "sane dates still work"
    );
}

// --- 6. The cache boundary validates the repo --------------------------------

#[test]
fn pane_cache_rejects_traversal_repos() {
    use gtasks::cache::{load_detail, save_detail};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    for evil in ["../../evil", "/tmp/evil", ""] {
        let err = save_detail(&root, evil, 1, "t", "x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid repo"), "save refuses {evil:?}: {err}");
        assert!(
            load_detail(&root, evil, 1).is_none(),
            "load refuses {evil:?}"
        );
    }
    assert!(
        !tmp.path().join("evil").exists(),
        "nothing escaped the cache dir"
    );
}

// --- 7. Foot writes: validated, atomic, private -------------------------------

#[test]
fn foot_hostile_family_is_refused() {
    use gtasks::config::set_foot_font;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("foot.ini");
    for evil in ["x\n[main]\nfont=evil", "x\ry", "x\0y", "  "] {
        let err = set_foot_font(&path, evil, 11).unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("control") || err.to_lowercase().contains("empty"),
            "family {evil:?} refused: {err}"
        );
    }
    assert!(!path.exists(), "a refused write leaves no file");
}

#[test]
fn foot_symlink_target_is_replaced_never_followed() {
    use gtasks::config::set_foot_font;
    let tmp = tempfile::tempdir().unwrap();
    let victim = tmp.path().join("victim.ini");
    std::fs::write(&victim, "PRECIOUS\n").unwrap();
    let link = tmp.path().join("foot.ini");
    std::os::unix::fs::symlink(&victim, &link).unwrap();

    let err = set_foot_font(&link, "monospace", 11)
        .unwrap_err()
        .to_string();
    assert!(err.contains("symlink"), "got {err}");
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "PRECIOUS\n",
        "the victim is untouched"
    );
}

#[test]
fn foot_new_file_lands_0600_with_content() {
    use gtasks::config::set_foot_font;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sub").join("foot.ini");
    assert!(set_foot_font(&path, "monospace", 13).unwrap(), "changed");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[main]\nfont=monospace:size=13\n"
    );
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "private, not umask: {mode:o}");
    assert!(
        !set_foot_font(&path, "monospace", 13).unwrap(),
        "same content: no-op"
    );
}

// --- 8. Exact token values are redacted ---------------------------------------

#[test]
fn token_value_in_env_is_redacted() {
    // GITHUB_TOKEN here so this never races a GH_TOKEN test elsewhere.
    let secret = "auditfix-token-0123456789abcdef";
    unsafe { std::env::set_var("GITHUB_TOKEN", secret) };
    let out = gtasks::sanitize::redact_credentials(&format!("oops {secret} leaked"));
    unsafe { std::env::remove_var("GITHUB_TOKEN") };
    assert!(!out.contains(secret), "value gone: {out}");
    assert!(out.contains("[REDACTED]"), "marked: {out}");
}

// --- 9. Launch paths re-validated ---------------------------------------------

#[test]
fn placed_ok_rejects_symlinks_and_escapes() {
    use gtasks::agent::placed_ok_in;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let tree = root.join("t");
    std::fs::create_dir_all(tree.join(".git")).unwrap();
    assert!(
        placed_ok_in(&tree, Some(&root)),
        "a real tree under root launches"
    );

    let link = root.join("linked");
    std::os::unix::fs::symlink(&tree, &link).unwrap();
    assert!(
        !placed_ok_in(&link, Some(&root)),
        "a swapped-in symlink never launches"
    );

    let outside = tmp.path().join("elsewhere");
    std::fs::create_dir_all(outside.join(".git")).unwrap();
    assert!(
        !placed_ok_in(&outside, Some(&root)),
        "outside the root asks instead of launching"
    );
    assert!(
        !placed_ok_in(&root.join("missing"), Some(&root)),
        "a vanished tree asks instead of failing"
    );
}

// --- 10. Browser child inherits no secrets -------------------------------------

#[test]
fn browser_child_sees_no_token() {
    use std::time::Instant;
    let secret = "auditfix-browser-0123456789abcdef";
    unsafe { std::env::set_var("GH_TOKEN", secret) };
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("xdg-open");
    let out = dir.path().join("env.txt");
    std::fs::write(&script, format!("#!/bin/sh\nenv > {}\n", out.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    gtasks::actions::open_in_browser(&script, "https://github.com/o/r/pull/1")
        .expect("launcher starts");
    unsafe { std::env::remove_var("GH_TOKEN") };

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = String::new();
    while Instant::now() < deadline {
        seen = std::fs::read_to_string(&out).unwrap_or_default();
        if !seen.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!seen.is_empty(), "the stub ran");
    assert!(!seen.contains(secret), "no token in the child env");
    assert!(
        seen.lines()
            .all(|l| !l.starts_with("GH_TOKEN=") && !l.starts_with("GIT_")),
        "no GH_TOKEN/GIT_* at all:\n{seen}"
    );
}
