//! S5/S14 symlink hardening for the state dir and the log file (F-1/F-2).
//!
//! Each test builds a victim with known mode + content, runs the real init
//! helper through a symlink, and asserts the victim is byte- and
//! mode-identical afterwards and the helper returned `Err`.
//!
//! NOTE: `--version` exits inside clap before logging init, so these tests
//! call the init helpers directly instead of spawning the binary.

use std::os::unix::fs::{PermissionsExt, symlink};

use gtasks::logging::{ensure_state_dir, open_log_file};

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
