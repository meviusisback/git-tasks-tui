//! A failed action must not be reported as **done**.
//!
//! `Executed::Finished(ExitStatus)` is returned for *any* completed process, and the status
//! line renders it as `done ({status})`. So a merge that `gh` refused reads
//!
//! ```text
//! done (exit status: 1)
//! ```
//!
//! which says the opposite of what happened. The distinction that already exists elsewhere in
//! this codebase — `Refused` for "we did not run it", `Unknown` for "it may have been applied" —
//! is exactly the right one, and it stops one step short: **a completed process with a non-zero
//! exit is a failure, and the word for it is not "done".**
//!
//! The assertions are tolerant about wording on purpose: they fix the **property** (the first
//! word must not claim success, the exit status must be visible), not the sentence, so a future
//! rewording does not turn the test into a maintenance chore.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gtasks::actions::{argv_for, Action, MergeMethod, RepoSlug};
use gtasks::app::{App, Key, Modal, Msg, PendingAction};
use gtasks::binaries::Binaries;
use gtasks::gh::CheckedArgv;

/// A fake `gh` that exits with `code` and says something on stderr.
fn fake_gh(dir: &Path, code: i32) -> PathBuf {
    let path = dir.join("gh");
    std::fs::write(
        &path,
        format!("#!/bin/sh\necho 'gh: branch is protected' >&2\nexit {code}\n"),
    )
    .expect("write fake gh");
    let mut perms = std::fs::metadata(&path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

fn git_binary() -> PathBuf {
    "/usr/bin/git".into()
}

/// Build the app state a user reaches after pressing `a` on a PR: a confirmation modal and the
/// exact argv that would run.
fn pending_merge(binaries: &Binaries) -> (App, Action) {
    let action = Action::Merge {
        repo: RepoSlug::parse("me/one").expect("slug"),
        number: 7,
        method: MergeMethod::Squash,
        delete_branch: false,
    };
    let argv: CheckedArgv = argv_for(binaries, &action).expect("argv");
    let app = App {
        modal: Some(Modal::Confirm("merge PR #7?".into())),
        pending_action: Some(PendingAction {
            action: action.clone(),
            argv,
        }),
        ..App::default()
    };
    (app, action)
}

#[test]
fn a_refused_merge_is_not_reported_as_done() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path(), 1), git_binary());
    let (mut app, _action) = pending_merge(&binaries);

    app.update(Msg::Key(Key::Char('y')));

    println!("status after a non-zero exit: {:?}", app.status);
    assert!(
        !app.status.starts_with("done"),
        "the action failed (exit 1) and the line says it was done: {:?}",
        app.status
    );
    assert!(
        app.status.contains('1'),
        "the exit status must stay visible, so the user can tell a refusal from a network \
         problem: {:?}",
        app.status
    );
}

#[test]
fn a_successful_merge_still_reads_as_done() {
    // The hinge: without it, a fix that renames every outcome to "failed" would pass the test
    // above and be just as wrong in the other direction.
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path(), 0), git_binary());
    let (mut app, _action) = pending_merge(&binaries);

    app.update(Msg::Key(Key::Char('y')));

    println!("status after exit 0: {:?}", app.status);
    assert!(
        app.status.starts_with("done"),
        "a process that exited 0 is done: {:?}",
        app.status
    );
}

/// `execute` itself is not the problem — it reports the exit status faithfully. This pins the
/// half that is right, so a fix cannot make `Finished` disappear instead of renaming it.
#[test]
fn execute_reports_the_exit_status_faithfully() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path(), 3), git_binary());
    let action = Action::Merge {
        repo: RepoSlug::parse("me/one").expect("slug"),
        number: 7,
        method: MergeMethod::Squash,
        delete_branch: false,
    };
    let argv = argv_for(&binaries, &action).expect("argv");

    let outcome = gtasks::actions_ui::execute(&argv, Duration::from_secs(10));
    println!("outcome: {outcome:?}");
    match outcome {
        gtasks::actions_ui::Executed::Finished(status) => {
            assert_eq!(status.code(), Some(3), "the exit code must survive");
        }
        other => panic!("a process that ran to completion is Finished, got {other:?}"),
    }
}
