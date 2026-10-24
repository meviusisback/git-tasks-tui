//! R5-1: rotation happens **in session**, on the write that crosses the bound,
//! not only at startup.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;

use gtasks::logging::RotatingFile;

fn backup(path: &std::path::Path) -> std::path::PathBuf {
    path.with_extension("log.1")
}

#[test]
fn the_writer_rotates_when_the_bound_is_crossed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gtasks.log");

    let mut writer = RotatingFile::open(&path, 100).unwrap();
    writer.write_all(&[b'a'; 60]).unwrap();
    assert!(path.exists(), "the live file is created at open");
    assert!(!backup(&path).exists(), "60 bytes is under the bound");

    // This write would push the file to 120, so the writer rotates first.
    writer.write_all(&[b'b'; 60]).unwrap();
    assert!(backup(&path).exists(), "rotation must happen in session");

    let live = std::fs::metadata(&path).unwrap().len();
    assert!(
        live <= 100,
        "the live file restarts under the bound: {live}"
    );
    let saved = std::fs::metadata(backup(&path)).unwrap().len();
    assert_eq!(saved, 60, "the previous contents are in the backup");
}

#[test]
fn rotation_keeps_exactly_one_generation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gtasks.log");
    let mut writer = RotatingFile::open(&path, 50).unwrap();

    for _ in 0..3 {
        writer.write_all(&[b'x'; 60]).unwrap();
    }

    assert!(backup(&path).exists());
    assert!(
        !path.with_extension("log.2").exists(),
        "a single backup: no log.2"
    );
}

#[test]
fn the_rotated_file_keeps_mode_0600() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gtasks.log");
    let mut writer = RotatingFile::open(&path, 10).unwrap();
    writer.write_all(&[b'z'; 20]).unwrap();

    for candidate in [path.clone(), backup(&path)] {
        let mode = std::fs::metadata(&candidate).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode,
            0o600,
            "{} must stay private, found {mode:o}",
            candidate.display()
        );
    }
}
