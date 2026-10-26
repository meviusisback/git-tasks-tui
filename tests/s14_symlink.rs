//! S5/S14 symlink hardening for the state dir and the log file (F-1/F-2).
//!
//! Each test builds a victim with known mode + content, runs the real init
//! helper through a symlink, and asserts the victim is byte- and
//! mode-identical afterwards and the helper returned `Err`.
//!
//! NOTE: `--version` exits inside clap before logging init, so these tests
//! call the init helpers directly instead of spawning the binary.

use std::os::unix::fs::{PermissionsExt, symlink};

use gtasks::logging::{ensure_state_dir, open_log_file, rotate_log};

fn mode_of(path: &std::path::Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

/// F-1: a symlinked state dir must be refused — the link target keeps its
/// mode and gains no log file.
#[test]
fn state_dir_symlink_is_not_followed() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let victim = root.join("victim");
    std::fs::create_dir(&victim).unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(victim.join("keep.txt"), "untouched").unwrap();

    let link = root.join("gtasks");
    symlink(&victim, &link).unwrap();

    let err = ensure_state_dir(&link).expect_err("symlinked state dir must be refused");
    assert!(
        format!("{err:#}").contains("symlink"),
        "error must name the symlink refusal, got: {err:#}"
    );

    assert_eq!(mode_of(&victim), 0o755, "victim dir mode must be unchanged");
    assert_eq!(
        std::fs::read_to_string(victim.join("keep.txt")).unwrap(),
        "untouched",
        "victim content must be unchanged"
    );
    assert!(
        !victim.join("gtasks.log").exists(),
        "no log file may appear through the link"
    );
}

/// F-1 (nested): a symlink in an intermediate component is refused too.
#[test]
fn state_dir_nested_symlink_is_not_followed() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let victim = root.join("victim");
    std::fs::create_dir(&victim).unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();

    let link = root.join("link");
    symlink(&victim, &link).unwrap();

    let err = ensure_state_dir(&link.join("sub"))
        .expect_err("nested symlinked component must be refused");
    assert!(
        format!("{err:#}").contains("symlink"),
        "error must name the symlink refusal, got: {err:#}"
    );

    assert_eq!(mode_of(&victim), 0o755, "victim dir mode must be unchanged");
    assert!(
        !victim.join("sub").exists(),
        "no subdirectory may be created through the link"
    );
}

/// F-2: a symlinked log file must be refused — the link target keeps its
/// mode and its bytes.
#[test]
fn log_file_symlink_is_not_followed() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let victim = root.join("victim.log");
    std::fs::write(&victim, "log-victim-bytes").unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();

    let link = root.join("gtasks.log");
    symlink(&victim, &link).unwrap();

    let err = open_log_file(&link).expect_err("symlinked log file must be refused");
    assert!(
        format!("{err:#}").to_lowercase().contains("symlink")
            || format!("{err:#}").contains("ELOOP")
            || format!("{err:#}").contains("loop"),
        "error must name the symlink refusal, got: {err:#}"
    );

    assert_eq!(
        mode_of(&victim),
        0o644,
        "victim file mode must be unchanged"
    );
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "log-victim-bytes",
        "victim content must be unchanged"
    );
}

/// F-5 (L1): at the threshold the live log becomes `.1`, keeping its bytes and
/// its mode. A tiny threshold keeps the test fast (never 5 MiB).
#[test]
fn log_rotation_keeps_one_backup() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let log = root.join("gtasks.log");
    let old = b"old-log-bytes".to_vec();
    std::fs::write(&log, &old).unwrap();
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o600)).unwrap();

    rotate_log(&log, old.len() as u64).unwrap();

    let backup = root.join("gtasks.log.1");
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        old,
        "the backup must carry the old bytes"
    );
    assert_eq!(
        mode_of(&backup),
        0o600,
        "the backup inherits the live log's mode"
    );
    assert!(
        !log.exists(),
        "rotation moves the file, it does not copy it"
    );

    drop(open_log_file(&log).unwrap());
    assert_eq!(
        std::fs::metadata(&log).unwrap().len(),
        0,
        "the fresh log starts empty"
    );
}

/// F-5 (L1): a symlinked log path is refused before any rename.
#[test]
fn rotation_refuses_symlinked_log() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let victim = root.join("victim.log");
    let bytes = b"victim-log-bytes".to_vec();
    std::fs::write(&victim, &bytes).unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    let link = root.join("gtasks.log");
    symlink(&victim, &link).unwrap();

    let err = rotate_log(&link, 1).expect_err("symlinked log must not rotate");
    assert!(
        format!("{err:#}").to_lowercase().contains("symlink"),
        "error must name the symlink refusal, got: {err:#}"
    );
    assert_eq!(
        std::fs::read(&victim).unwrap(),
        bytes,
        "the victim keeps its bytes"
    );
    assert!(
        !root.join("gtasks.log.1").exists(),
        "no backup may be created through a symlink"
    );
}

/// F-5 (L1): exactly one generation — a previous `.1` is replaced, never appended.
#[test]
fn rotation_replaces_previous_backup() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let log = root.join("gtasks.log");
    let fresh = b"new-log-bytes".to_vec();
    std::fs::write(&log, &fresh).unwrap();
    let backup = root.join("gtasks.log.1");
    std::fs::write(&backup, "stale-backup").unwrap();

    rotate_log(&log, fresh.len() as u64).unwrap();

    let bytes = std::fs::read(&backup).unwrap();
    assert_eq!(
        bytes, fresh,
        "the old backup must be replaced, not appended to"
    );
    assert!(
        !bytes.windows(6).any(|w| w == b"stale-"),
        "no byte of the stale backup may survive"
    );
}

/// S14 (L3): `..` is refused in the middle of a path, and nothing is created.
#[test]
fn state_dir_rejects_parent_dir_component_in_the_middle() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let target = root.join("gtasks-should-not-exist");
    let err = ensure_state_dir(&root.join("..").join("gtasks-should-not-exist"))
        .expect_err("a `..` component must be refused");
    assert!(
        format!("{err:#}").contains(".."),
        "error must name the `..` refusal, got: {err:#}"
    );
    assert!(
        !target.exists(),
        "nothing may be created under the refused path"
    );
}

/// S14 (L3): a trailing `..` is refused **before** creating the missing
/// component that precedes it — no half-built tree.
#[test]
fn state_dir_rejects_trailing_parent_dir_without_side_effects() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let missing = root.join("missing");
    let err = ensure_state_dir(&missing.join(".."))
        .expect_err("a trailing `..` component must be refused");
    assert!(
        format!("{err:#}").contains(".."),
        "error must name the `..` refusal, got: {err:#}"
    );
    assert!(
        !missing.exists(),
        "the missing component must not be created before the refusal"
    );
}

/// S14 happy path: real directories end up 0700, a fresh log is 0600.
#[test]
fn private_dir_and_log_modes_are_0700_and_0600() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let dir = root.join("nested/state");
    ensure_state_dir(&dir).unwrap();
    assert_eq!(mode_of(&dir), 0o700, "final dir must be 0700");
    assert_eq!(
        mode_of(&root.join("nested")),
        0o700,
        "intermediate dirs are created 0700 too"
    );

    let log = dir.join("gtasks.log");
    drop(open_log_file(&log).unwrap());
    assert_eq!(mode_of(&log), 0o600, "a fresh log must be 0600");
}

/// S5/S14: the agent runtime dir follows the same rule as the state dir.
#[test]
fn runtime_dir_symlink_is_not_followed() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let victim = root.join("victim");
    std::fs::create_dir(&victim).unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();

    let link = root.join("gtasks");
    symlink(&victim, &link).unwrap();

    let err = gtasks::agent::ensure_runtime_dir(&link)
        .expect_err("symlinked runtime dir must be refused");
    assert!(
        format!("{err:#}").contains("symlink"),
        "error must name the symlink refusal, got: {err:#}"
    );
    assert_eq!(mode_of(&victim), 0o755, "victim dir mode must be unchanged");
}
