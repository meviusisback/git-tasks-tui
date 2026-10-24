//! P2: a **failed** second init must not rotate the live log.
//!
//! In its own integration-test binary: it installs a global `tracing`
//! subscriber, which would affect any other test in the same process.

use std::os::unix::fs::MetadataExt;

use gtasks::logging::{MAX_LOG_BYTES, init_file_logging_at};

#[test]
fn failed_second_init_does_not_rotate() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state/gtasks.log");

    // First init succeeds and installs the global subscriber.
    init_file_logging_at(&path).expect("first init must succeed");

    // Grow the live log past the rotation threshold, then try to init again.
    let big = vec![b'y'; (MAX_LOG_BYTES + 10) as usize];
    std::fs::write(&path, &big).expect("write live log");
    let before = std::fs::metadata(&path).expect("stat live log");

    let second = init_file_logging_at(&path);
    assert!(
        second.is_err(),
        "second init must be refused, got: {second:?}"
    );

    let after = std::fs::metadata(&path).expect("stat live log after");
    assert_eq!(
        after.ino(),
        before.ino(),
        "the live log must not be replaced by a fresh file"
    );
    assert_eq!(
        after.len(),
        before.len(),
        "the live log must not be rotated away"
    );
    assert!(
        !root.path().join("state/gtasks.log.1").exists(),
        "a refused init must not create a backup"
    );
}
