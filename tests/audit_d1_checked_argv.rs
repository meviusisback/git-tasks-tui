//! Auditor probe for **D-1** — written *before* `CheckedArgv` exists, so the
//! first test below is **red on purpose**.
//!
//! The gap: `check_argv` sees every argv, typed or hand-built. To keep
//! `-done: looks good` legal as a *value* it only decomposes a token whose
//! letters are all in `SHORT_FLAGS = ['f','d']` — so a cluster carrying a
//! forbidden letter the alphabet does not know (`-fR`, `-fq`, `-fn`) slips
//! through and keeps `--force` inside.
//!
//! The shape that closes it: an argv type with a private field, constructed only
//! by `argv_for`/`gh`/`git`. `check_argv` then only ever sees hand-built argvs,
//! where fail-closed costs nothing — no legitimate value travels that path.
//!
//! When `CheckedArgv` lands, the first test must turn green; the three controls
//! must stay green throughout.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use gtasks::actions::{Action, MergeMethod, RepoSlug, argv_for};
use gtasks::binaries::Binaries;
use gtasks::env::git_env_from;
use gtasks::gh::{Argv, check_argv};

fn binaries() -> Binaries {
    Binaries::new(PathBuf::from("/bin/true"), PathBuf::from("/bin/true"))
}

/// A raw argv, the way hand-written code (or a future caller) would build one.
fn hand_built(arg: &str) -> Argv {
    Argv {
        program: PathBuf::from("/bin/true"),
        args: vec![OsString::from(arg)],
        env: git_env_from(&Default::default()),
        cwd: std::env::temp_dir(),
    }
}

fn slug() -> RepoSlug {
    RepoSlug::parse("acme/widget").expect("the slug is valid")
}

/// **D-1**: red today. Every one of these carries `-f` plus a letter the
/// alphabet ignores, so it is treated as a value and accepted.
#[test]
fn a_hand_built_argv_refuses_a_cluster_that_carries_a_forbidden_letter() {
    for cluster in ["-fR", "-fq", "-fn", "-fs", "-fC"] {
        assert!(
            check_argv(&hand_built(cluster)).is_err(),
            "{cluster} carries -f plus a letter outside SHORT_FLAGS: a hand-built \
             argv must refuse it (fail-closed) instead of treating it as a value"
        );
    }
}

/// Control: the plain forbidden flags are refused today and must stay refused.
#[test]
fn a_hand_built_argv_refuses_the_plain_forbidden_flags() {
    for flag in ["-f", "--force", "--admin", "--auto"] {
        assert!(
            check_argv(&hand_built(flag)).is_err(),
            "{flag} must stay refused"
        );
    }
}

/// Control: a **value** coming from a typed action that happens to start with a
/// dash must survive. This is the case that makes fail-closed impossible while
/// `check_argv` also sees action argvs.
#[test]
fn a_typed_action_value_that_starts_with_a_dash_is_still_a_value() {
    let action = Action::Comment {
        repo: slug(),
        number: 7,
        body: "-done: looks good".to_string(),
    };
    let argv = argv_for(&binaries(), &action).expect("a dash-leading body is legal");
    check_argv(&argv).expect("a typed value is not a flag, whatever it starts with");
}

/// Control: nothing hostile can enter through the typed path.
#[test]
fn a_typed_action_cannot_carry_a_forbidden_flag() {
    let action = Action::Merge {
        repo: slug(),
        number: 7,
        method: MergeMethod::Squash,
        delete_branch: true,
    };
    let argv = argv_for(&binaries(), &action).expect("a squash merge deleting the branch is legal");
    check_argv(&argv).expect("the builder emits only allowlisted flags");
    for arg in &argv.args {
        assert!(
            !arg.to_string_lossy().contains("--force")
                && !arg.to_string_lossy().contains("--admin"),
            "the typed path must never emit a forbidden flag: {arg:?}"
        );
    }
}

#[allow(dead_code)]
fn _unused(_: &Path) {}
