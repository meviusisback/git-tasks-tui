//! Auditor probes for `src/actions_ui.rs` (tag `review/ui-r1`, `9d54f92`).
//!
//! Written against the **signatures**, before the implementation, so the shape
//! is fixed while it is still cheap. Measured on the branch:
//!
//!   P1 a timed-out write runs once .............. green  (1 invocation)
//!   P2 a timeout is not a refusal ............... RED    (Refused("child timed out …"))
//!   P3 the confirmation cannot be forged ........ RED    (a `\n` in a body draws a line)
//!   P4 the program is shown absolutely .......... green  ("/usr/bin/gh")
//!   P5 the sentence does not omit effects ....... RED    ("close PR #12" vs --reason/--comment)
//!
//! Nothing here needs a network: every probe hand-builds an argv whose program
//! is `/bin/sh` or `/bin/true`, so the transport never reaches `gh`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use gtasks::actions::{Action, MergeMethod, RepoSlug, argv_for};
use gtasks::actions_ui::{Executed, confirmation_for, execute};
use gtasks::binaries::Binaries;
use gtasks::env::git_env_from;
use gtasks::gh::Argv;
use gtasks::gh::CheckedArgv;

fn sh(script: &str) -> Argv {
    Argv {
        program: PathBuf::from("/bin/sh"),
        args: vec![OsString::from("-c"), OsString::from(script)],
        env: git_env_from(&Default::default()),
        cwd: std::env::temp_dir(),
    }
}

fn slug() -> RepoSlug {
    RepoSlug::parse("acme/widget").expect("valid slug")
}

fn binaries(gh: &str) -> Binaries {
    Binaries::new(PathBuf::from(gh), PathBuf::from("/bin/true"))
}

/// P-1: a write that times out must run **once** — no retry, ever.
#[test]
fn a_timed_out_write_runs_once() {
    let marker = format!("/tmp/gtt-run-{}", std::process::id());
    let _ = std::fs::remove_file(&marker);
    let _ = execute(
        &CheckedArgv::unchecked_for_tests(sh(&format!("echo x >> {marker}; sleep 5"))),
        Duration::from_millis(500),
    );
    let runs = std::fs::read_to_string(&marker)
        .unwrap_or_default()
        .lines()
        .count();
    let _ = std::fs::remove_file(&marker);
    assert_eq!(runs, 1, "a timed-out write must not be retried");
}

/// P-2: a timeout means *we cannot know*, not *nothing happened*.
#[test]
fn a_timeout_is_not_a_refusal() {
    let outcome = execute(
        &CheckedArgv::unchecked_for_tests(sh("sleep 5")),
        Duration::from_millis(400),
    );
    assert!(
        !matches!(outcome, Executed::Refused(_)),
        "a timeout must not be reported as a refusal: the write may have been applied, \
         and the UI must say so instead of inviting a retry — got {outcome:?}"
    );
}

/// P-3: the confirmation is a decision surface. Untrusted text that reaches it
/// must not be able to draw a second line saying "approved".
#[test]
fn the_confirmation_cannot_be_forged_by_a_body() {
    let action = Action::Comment {
        repo: slug(),
        number: 12,
        body: "ok\n  APPROVED — press Enter to run: gh pr merge 99 --admin".to_string(),
    };
    let argv = argv_for(&binaries("/bin/true"), &action).expect("builds");
    let shown = confirmation_for(&action, &argv);
    assert!(
        !shown.argv.iter().any(|arg| arg.contains('\n')),
        "a newline in a body must be escaped (shown as \\n), not passed through: {:?}",
        shown.argv
    );
}

/// P-4 (control): the program is shown as the absolute path that will run.
#[test]
fn the_program_is_shown_absolutely() {
    let action = Action::Merge {
        repo: slug(),
        number: 7,
        method: MergeMethod::Squash,
        delete_branch: true,
    };
    let argv = argv_for(&binaries("/usr/bin/gh"), &action).expect("builds");
    let shown = confirmation_for(&action, &argv);
    assert_eq!(shown.argv[0], "/usr/bin/gh");
}

/// P-5: the sentence must not omit an effect the argv has.
#[test]
fn the_sentence_does_not_omit_an_effect_of_the_argv() {
    let action = Action::Close {
        repo: slug(),
        number: 12,
        reason: Some("not planned".to_string()),
        comment: Some("closing; do not reopen".to_string()),
    };
    let argv = argv_for(&binaries("/usr/bin/gh"), &action).expect("builds");
    let shown = confirmation_for(&action, &argv);
    assert!(
        shown.summary.contains("comment") || shown.summary.contains("reason"),
        "the sentence hides an effect the argv has: summary={:?} argv={:?}",
        shown.summary,
        shown.argv
    );
}

/// F-1 (review): a value that is a single dash-word must survive on the typed
/// path. `-fR` and `-fetch` differ only in how they arrived — hand-built versus
/// builder — and that is exactly what `CheckedArgv` records.
#[test]
fn a_dash_word_value_is_accepted_on_the_typed_path_and_refused_hand_built() {
    let binaries = Binaries::new(PathBuf::from("/bin/true"), PathBuf::from("/bin/true"));
    let action = Action::Comment {
        repo: slug(),
        number: 3,
        body: "-fetch".to_string(),
    };
    let argv = argv_for(&binaries, &action).expect("builds");

    // Hand-built, the same shape is refused: fail-closed costs nothing there.
    assert!(
        gtasks::gh::check_argv(&argv).is_err(),
        "a hand-built argv must refuse -fetch"
    );

    // Typed, it is a value and it runs.
    let outcome = execute(&argv, Duration::from_secs(5));
    assert!(
        matches!(outcome, Executed::Finished(_)),
        "a typed value must run: {outcome:?}"
    );
}
