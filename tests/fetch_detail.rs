//! Detail body: `gh pr view <n> --json body`, argv only.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gtasks::fetch::{FetchError, detail_args, fetch_detail_with_caps};
use gtasks::gh::{Argv, Caps};
use gtasks::model::ItemKind;

fn fake_gh(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("gh");
    std::fs::write(&path, format!("#!/bin/sh\ncat <<'JSON'\n{body}\nJSON\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn argv_for(program: &Path, repo: &str, kind: ItemKind, number: u64) -> Argv {
    Argv {
        program: program.to_path_buf(),
        args: detail_args(repo, kind, number),
        env: vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))],
        cwd: std::env::temp_dir(),
    }
}

#[test]
fn the_argv_asks_for_the_body_of_that_number_only() {
    let args = detail_args("acme/widget", ItemKind::Pr, 42);
    assert_eq!(args[0], "pr");
    assert_eq!(args[1], "view");
    assert_eq!(args[2], "42");
    assert!(
        args.windows(2)
            .any(|w| w[0] == "--repo" && w[1] == "acme/widget")
    );
    assert!(args.windows(2).any(|w| w[0] == "--json" && w[1] == "body"));
    assert!(!args.iter().any(|a| a == "--comments"), "no extra payload");

    let issue = detail_args("acme/widget", ItemKind::Issue, 7);
    assert_eq!(issue[0], "issue");
}

#[test]
fn the_body_comes_back_as_markdown_text() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_gh(dir.path(), "{\"body\":\"## Fix\\n\\nUse **this**.\"}");

    let body = fetch_detail_with_caps(
        &argv_for(&program, "acme/widget", ItemKind::Pr, 12),
        "acme/widget",
        Duration::from_secs(10),
        Caps::default(),
    )
    .unwrap();

    assert_eq!(body, "## Fix\n\nUse **this**.");
}

#[test]
fn an_empty_body_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_gh(dir.path(), "{\"body\":\"\"}");

    let body = fetch_detail_with_caps(
        &argv_for(&program, "acme/widget", ItemKind::Pr, 1),
        "acme/widget",
        Duration::from_secs(10),
        Caps::default(),
    )
    .unwrap();

    assert_eq!(body, "");
}

#[test]
fn a_malformed_body_payload_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_gh(dir.path(), "not json");

    let err = fetch_detail_with_caps(
        &argv_for(&program, "acme/widget", ItemKind::Issue, 3),
        "acme/widget",
        Duration::from_secs(10),
        Caps::default(),
    )
    .unwrap_err();

    assert!(matches!(err, FetchError::Parse { .. }), "got {err:?}");
}

#[test]
fn a_conversation_flood_is_refused_by_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_gh(dir.path(), "{\"body\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}");

    let err = fetch_detail_with_caps(
        &argv_for(&program, "acme/widget", ItemKind::Pr, 5),
        "acme/widget",
        Duration::from_secs(10),
        Caps {
            stdout: 16,
            stderr: 1024,
        },
    )
    .unwrap_err();

    assert!(matches!(err, FetchError::OutputCap), "got {err:?}");
}
