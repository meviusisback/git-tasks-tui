//! L2: a second logging initialisation in the same process must not panic.
//!
//! Kept in its own integration-test binary: it installs a global `tracing`
//! subscriber, which would affect any other test in the same process.

use gtasks::logging::init_file_logging_at;

#[test]
fn second_logging_init_does_not_panic() {
    let _guard = tempfile::tempdir().unwrap();
    let root = _guard.path().canonicalize().unwrap();
    let path = root.join("state/gtasks.log");

    init_file_logging_at(&path).expect("first init must succeed");

    let second = init_file_logging_at(&path);
    let err = second.expect_err("second init must return Err, not panic");
    assert!(
        format!("{err:#}").to_lowercase().contains("already"),
        "error must explain the duplicate initialisation, got: {err:#}"
    );
}
