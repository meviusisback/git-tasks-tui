//! Task 7a: one repo, one `gh` call, no surprises.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gtasks::fetch::{FIELDS_ISSUE, FIELDS_PR, FetchError, fetch_args, fetch_with_caps};
use gtasks::gh::{Argv, Caps};
use gtasks::model::ItemKind;

/// A fake `gh` that prints `body` and exits 0, so the real spawn path is used.
fn fake_gh(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("gh");
    std::fs::write(&path, format!("#!/bin/sh\ncat <<'JSON'\n{body}\nJSON\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn argv_for(program: &Path, repo: &str, kind: ItemKind) -> Argv {
    Argv {
        program: program.to_path_buf(),
        args: fetch_args(repo, kind, 5),
        env: vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))],
        cwd: std::env::temp_dir(),
    }
}

#[test]
fn the_field_lists_are_constants_and_differ_by_kind() {
    assert!(FIELDS_PR.contains("reviewDecision"));
    assert!(
        !FIELDS_ISSUE.contains("reviewDecision"),
        "issues have no review"
    );
    assert!(FIELDS_ISSUE.contains("labels"));
    // A field list is data, not a command: no shell metacharacters.
    for fields in [FIELDS_PR, FIELDS_ISSUE] {
        assert!(!fields.contains([';', '|', '&', '$', '`']));
    }
}

#[test]
fn args_pass_the_repo_as_a_value_never_as_a_flag() {
    let args = fetch_args("--evil/--also-evil", ItemKind::Pr, 7);
    let repo_at = args.iter().position(|a| a == "--repo").unwrap();
    assert_eq!(
        args[repo_at + 1],
        "--evil/--also-evil",
        "the value stays a value"
    );
    assert!(args.iter().any(|a| a == "pr"));
    assert!(args.iter().any(|a| a == "7"));
    assert!(
        args.iter().all(|a| a != "-c" && a != "--exec"),
        "no passthrough flags"
    );
}

#[test]
fn a_repo_name_with_spaces_stays_one_argument() {
    let args = fetch_args("acme/my repo", ItemKind::Issue, 3);
    assert!(args.iter().any(|a| a == "acme/my repo"));
}

#[test]
fn a_good_payload_is_parsed_and_stamped_with_origin() {
    let dir = tempfile::tempdir().unwrap();
    let body =
        r#"[{"number":12,"title":"Add widget","state":"OPEN","updatedAt":"2026-10-20T10:00:00Z"}]"#;
    let program = fake_gh(dir.path(), body);

    let items = fetch_with_caps(
        &argv_for(&program, "acme/widget", ItemKind::Pr),
        "acme/widget",
        ItemKind::Pr,
        Duration::from_secs(10),
        Caps::default(),
    )
    .unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].number, 12);
    assert_eq!(
        items[0].repo, "acme/widget",
        "origin comes from the fetcher"
    );
    assert_eq!(items[0].kind, ItemKind::Pr);
}

#[test]
fn a_payload_saying_it_belongs_elsewhere_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    // The payload tries to claim another repo and another kind.
    let body = r#"[{"number":1,"title":"x","state":"OPEN","repo":"evil/evil","kind":"Issue"}]"#;
    let program = fake_gh(dir.path(), body);

    let items = fetch_with_caps(
        &argv_for(&program, "acme/widget", ItemKind::Pr),
        "acme/widget",
        ItemKind::Pr,
        Duration::from_secs(10),
        Caps::default(),
    )
    .unwrap();

    assert_eq!(items[0].repo, "acme/widget");
    assert_eq!(items[0].kind, ItemKind::Pr);
}

#[test]
fn malformed_json_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_gh(dir.path(), "not json at all");

    let err = fetch_with_caps(
        &argv_for(&program, "acme/widget", ItemKind::Issue),
        "acme/widget",
        ItemKind::Issue,
        Duration::from_secs(10),
        Caps::default(),
    )
    .unwrap_err();

    assert!(matches!(err, FetchError::Parse { .. }), "got {err:?}");
    assert!(err.to_string().contains("acme/widget"));
}

#[test]
fn a_truncated_payload_is_refused_rather_than_half_parsed() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_gh(
        dir.path(),
        r#"[{"number":1,"title":"a long enough title"}]"#,
    );

    let err = fetch_with_caps(
        &argv_for(&program, "acme/widget", ItemKind::Pr),
        "acme/widget",
        ItemKind::Pr,
        Duration::from_secs(10),
        Caps {
            stdout: 8,
            stderr: 1024,
        },
    )
    .unwrap_err();

    assert!(matches!(err, FetchError::OutputCap), "got {err:?}");
}

#[test]
fn a_failing_gh_reports_the_exit_status_and_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("gh");
    std::fs::write(&program, "#!/bin/sh\necho 'no such repo' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    let err = fetch_with_caps(
        &argv_for(&program, "acme/missing", ItemKind::Pr),
        "acme/missing",
        ItemKind::Pr,
        Duration::from_secs(10),
        Caps::default(),
    )
    .unwrap_err();

    assert!(matches!(err, FetchError::Run { .. }), "got {err:?}");
    assert!(err.to_string().contains("no such repo"), "got {err}");
}

/// The auditor's 401 probe: an authenticated failure is a `Run` error carrying
/// stderr — which can contain a token. The log layer sanitises it (L-1); this
/// test pins that it stays a `Run` error and not a parse failure.
#[test]
fn a_401_is_a_run_error_carrying_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("gh");
    std::fs::write(
        &program,
        "#!/bin/sh\necho 'HTTP 401: Bad credentials' >&2\necho 'token: gho_16C7eABCdef' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    let err = fetch_with_caps(
        &argv_for(&program, "acme/widget", ItemKind::Pr),
        "acme/widget",
        ItemKind::Pr,
        Duration::from_secs(10),
        Caps::default(),
    )
    .unwrap_err();

    assert!(matches!(err, FetchError::Run { .. }), "got {err:?}");
    let text = err.to_string();
    assert!(text.contains("401"), "got {text}");

    // What the log would store is the sanitised form: no token survives.
    let logged = gtasks::sanitize::sanitize(&text);
    assert!(!logged.contains("gho_16C7e"), "got {logged}");
}
