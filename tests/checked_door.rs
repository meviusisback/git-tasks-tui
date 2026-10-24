//! W-1: the test-only door must be **loud** when misused.
//!
//! `CheckedArgv::unchecked_for_tests` exists because integration tests link the
//! crate normally and cannot reach `from_checked`. `#[doc(hidden)]` hides it from
//! the docs, not from the compiler, so the guard is a `debug_assert`: a misuse
//! fails where it is written instead of quietly widening the door.

use std::ffi::OsString;
use std::path::PathBuf;

use gtasks::env::git_env_from;
use gtasks::gh::{Argv, CheckedArgv};

fn raw(arg: &str) -> Argv {
    Argv {
        program: PathBuf::from("/bin/true"),
        args: vec![OsString::from(arg)],
        env: git_env_from(&Default::default()),
        cwd: std::env::temp_dir(),
    }
}

#[test]
#[should_panic(expected = "this door is for probes")]
fn w1_a_bypass_through_the_test_door_is_loud() {
    let _ = CheckedArgv::unchecked_for_tests(raw("--admin"));
}

#[test]
fn w1_a_legit_argv_passes_the_door() {
    for arg in ["-d", "-done"] {
        let _ = CheckedArgv::unchecked_for_tests(raw(arg));
    }
}
