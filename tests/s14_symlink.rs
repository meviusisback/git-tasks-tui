//! S5/S14 symlink hardening for the state dir and the log file (F-1/F-2).
//!
//! Each test builds a victim with known mode + content, runs the real init
//! helper through a symlink, and asserts the victim is byte- and
//! mode-identical afterwards and the helper returned `Err`.
//!
//! NOTE: `--version` exits inside clap before logging init, so these tests
//! call the init helpers directly instead of spawning the binary.

use std::os::unix::fs::{PermissionsExt, symlink};

use gtasks::logging::{MAX_LOG_BYTES, ensure_state_dir, open_log_file, rotate_log};

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
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("victim");
    std::fs::create_dir(&victim).unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(victim.join("keep.txt"), "untouched").unwrap();

    let link = root.path().join("gtasks");
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
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("victim");
    std::fs::create_dir(&victim).unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();

    let link = root.path().join("link");
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
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("victim.log");
    std::fs::write(&victim, "log-victim-bytes").unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();

    let link = root.path().join("gtasks.log");
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

/// F-5 (L1): at the threshold the live log becomes `.1`, keeping its bytes.
#[test]
fn log_rotation_keeps_one_backup() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("gtasks.log");
    std::fs::write(&log, vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();

    rotate_log(&log).unwrap();

    let backup = root.path().join("gtasks.log.1");
    assert_eq!(
        std::fs::metadata(&backup).unwrap().len(),
        MAX_LOG_BYTES,
        "the backup must carry the old bytes"
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
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("victim.log");
    std::fs::write(&victim, vec![b'v'; MAX_LOG_BYTES as usize]).unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    let link = root.path().join("gtasks.log");
    symlink(&victim, &link).unwrap();

    let err = rotate_log(&link).expect_err("symlinked log must not rotate");
    assert!(
        format!("{err:#}").to_lowercase().contains("symlink"),
        "error must name the symlink refusal, got: {err:#}"
    );
    assert_eq!(
        std::fs::metadata(&victim).unwrap().len(),
        MAX_LOG_BYTES,
        "the victim keeps its size"
    );
    assert!(
        !root.path().join("gtasks.log.1").exists(),
        "no backup may be created through a symlink"
    );
}

/// F-5 (L1): exactly one generation — a previous `.1` is replaced, never appended.
#[test]
fn rotation_replaces_previous_backup() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("gtasks.log");
    std::fs::write(&log, vec![b'n'; MAX_LOG_BYTES as usize]).unwrap();
    let backup = root.path().join("gtasks.log.1");
    std::fs::write(&backup, "stale-backup").unwrap();

    rotate_log(&log).unwrap();

    let bytes = std::fs::read(&backup).unwrap();
    assert_eq!(
        bytes.len() as u64,
        MAX_LOG_BYTES,
        "the old backup must be replaced, not appended to"
    );
    assert!(
        bytes.starts_with(b"nnn"),
        "the new backup is the live log's content"
    );
    assert!(
        !bytes.windows(6).any(|w| w == b"stale-"),
        "no byte of the stale backup may survive"
    );
}

/// S14 (L3): a `..` component is refused outright, before any filesystem work.
#[test]
fn state_dir_rejects_parent_dir_component() {
    let err = ensure_state_dir(std::path::Path::new("/tmp/../tmp/gtasks-should-not-exist"))
        .expect_err("a `..` component must be refused");
    assert!(
        format!("{err:#}").contains(".."),
        "error must name the `..` refusal, got: {err:#}"
    );
}

/// S14 happy path: real directories end up 0700, a fresh log is 0600.
#[test]
fn private_dir_and_log_modes_are_0700_and_0600() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("nested/state");
    ensure_state_dir(&dir).unwrap();
    assert_eq!(mode_of(&dir), 0o700, "final dir must be 0700");
    assert_eq!(
        mode_of(&root.path().join("nested")),
        0o700,
        "intermediate dirs are created 0700 too"
    );

    let log = dir.join("gtasks.log");
    drop(open_log_file(&log).unwrap());
    assert_eq!(mode_of(&log), 0o600, "a fresh log must be 0600");
}
