//! After an action changes something, the screen must ask the **server** what is true.
//!
//! The contract now says it: *an action that changes something must read the state back and
//! show it, instead of trusting the return code*. Today the app does not: `y` renders the exit
//! status and stops.
//!
//! ```text
//! Some(pending) => { self.status = describe(execute(&pending.argv, self.write_timeout)); }
//! ```
//!
//! That is a claim about the server's state made from a local exit code. Measured twice today
//! in this project's own history: a write can return an error **and have happened** (a
//! `gh pr create` answered HTTP 504 while the pull request was created), and a write can
//! return 0 for something that did not take effect the way the user thinks (a merge queued
//! behind branch protection).
//!
//! So the observable is not the sentence: it is `App::refresh_requested`, the same flag `r`
//! sets and the main loop turns into a fetch. An action that reports an outcome without asking
//! the server leaves the screen describing a state nobody verified.
//!
//! Three cases, because the fix has two ways to be wrong:
//!
//!   1. a **successful** action must ask for the read-back (red today);
//!   2. an action that **may have been applied** (a timeout: `Executed::Unknown`) must ask for
//!      it too — ambiguity is exactly when a read-back is worth most (red today);
//!   3. **cancelling** must not ask for anything, so a fix cannot simply set the flag on every
//!      key (the hinge: this is the case that fails if the flag is set unconditionally).

use std::path::{Path, PathBuf};
use std::time::Duration;

use gtasks::actions::{Action, MergeMethod, RepoSlug, argv_for};
use gtasks::app::{App, Key, Modal, Msg, Pending};
use gtasks::binaries::Binaries;
use gtasks::gh::CheckedArgv;

/// A fake `gh` that exits `code` immediately (§1 and §3) — or hangs, for §2.
fn fake_gh(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("gh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake gh");
    let mut perms = std::fs::metadata(&path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

fn app_with_pending_merge(binaries: &Binaries, write_timeout: Duration) -> App {
    let action = Action::Merge {
        repo: RepoSlug::parse("me/one").expect("slug"),
        number: 7,
        method: MergeMethod::Squash,
        delete_branch: false,
    };
    let argv: CheckedArgv = argv_for(binaries, &action).expect("argv");
    App {
        modal: Some(Modal::Confirm("merge PR #7?".into())),
        pending: Some(Pending::Action(gtasks::app::PendingAction { action, argv })),
        write_timeout,
        ..App::default()
    }
}

#[test]
fn a_successful_action_asks_the_server() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path(), "exit 0"), "/usr/bin/git".into());
    let mut app = app_with_pending_merge(&binaries, Duration::from_secs(10));
    app.refresh_requested = false;

    app.update(Msg::Key(Key::Char('y')));

    println!(
        "status={:?} refresh_requested={}",
        app.status, app.refresh_requested
    );
    assert!(
        app.refresh_requested,
        "the action reported an outcome ({:?}) without asking the server: the screen is \
         describing a state nobody read back",
        app.status
    );
}

#[test]
fn an_action_that_may_have_been_applied_asks_the_server() {
    // A write that times out is the case where "did it happen?" is genuinely unknown, and the
    // app already says so ("it may have been applied") — which is a reason to look, not a
    // substitute for looking.
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(
        fake_gh(tmp.path(), "sleep 5\nexit 0"),
        "/usr/bin/git".into(),
    );
    let mut app = app_with_pending_merge(&binaries, Duration::from_millis(200));
    app.refresh_requested = false;

    app.update(Msg::Key(Key::Char('y')));

    println!(
        "status={:?} refresh_requested={}",
        app.status, app.refresh_requested
    );
    assert!(
        app.status.contains("may have been applied"),
        "this case is only meaningful if the action timed out: {:?}",
        app.status
    );
    assert!(
        app.refresh_requested,
        "an ambiguous write must be resolved by reading the state back"
    );
}

#[test]
fn cancelling_asks_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path(), "exit 0"), "/usr/bin/git".into());
    let mut app = app_with_pending_merge(&binaries, Duration::from_secs(10));
    app.refresh_requested = false;

    app.update(Msg::Key(Key::Char('n')));

    println!(
        "status={:?} refresh_requested={}",
        app.status, app.refresh_requested
    );
    assert!(
        !app.refresh_requested,
        "nothing was done, so there is nothing to read back: a fix that sets the flag on \
         every key would pass the other two tests and be wrong here"
    );
}
