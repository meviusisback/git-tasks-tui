//! "Put the terminal back": one owner, and **exactly once**.
//!
//! Each case here is one of the ways a TUI can leave a user without a usable terminal. They
//! are checked without a tty on purpose: the restore action is a parameter, so the test counts
//! calls instead of staring at a screen — and a count is what catches the defect that matters,
//! which is **two** mechanisms restoring the same terminal.
//!
//! Kept in its own file (its own process) because it installs a panic hook, and a hook is
//! process-wide: sharing a binary with other tests would let this one change their behaviour.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gtasks::tui::Tui;

fn counter() -> (Arc<AtomicUsize>, Box<dyn FnMut() + Send>) {
    let n = Arc::new(AtomicUsize::new(0));
    let for_restore = Arc::clone(&n);
    (
        n,
        Box::new(move || {
            for_restore.fetch_add(1, Ordering::SeqCst);
        }),
    )
}

#[test]
fn a_normal_exit_restores_once() {
    let (calls, restore) = counter();
    {
        let _tui = Tui::enter_with(|| Ok(()), || Ok(()), restore).expect("enter");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing to restore yet");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "dropping the guard must restore the terminal exactly once"
    );
}

#[test]
fn a_panic_inside_the_scope_restores_once() {
    let (calls, restore) = counter();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let _tui = Tui::enter_with(|| Ok(()), || Ok(()), restore).expect("enter");
        panic!("the body panicked");
    }));
    assert!(outcome.is_err(), "the panic must propagate");
    // One, not two: the hook runs first and Drop runs during unwinding, and only the first
    // of them may act. A count of two here means the two mechanisms are racing, which on a
    // real terminal shows up as the error message printed *after* leaving the screen.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the hook and Drop both restored: two mechanisms, one fact"
    );
}

#[test]
fn the_hook_is_not_left_behind() {
    let (calls, restore) = counter();
    {
        let _tui = Tui::enter_with(|| Ok(()), || Ok(()), restore).expect("enter");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // A panic *after* the guard is gone must not reach back into a terminal that is already
    // restored: the previous hook has to be back in place.
    let _ = panic::catch_unwind(AssertUnwindSafe(|| panic!("after the guard")));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the scope's hook survived its guard and restored a second time"
    );
}

#[test]
fn a_failed_first_step_restores_nothing() {
    // Nothing was changed, so nothing may be restored: a guard that restores what it never
    // changed is as wrong as one that leaves raw mode on, and it is the harder of the two to
    // notice on a real terminal (the screen flickers and looks fine).
    let (calls, restore) = counter();
    let err = Tui::enter_with(
        || Err(std::io::Error::other("enable_raw_mode failed")),
        || Ok(()),
        restore,
    );
    assert!(err.is_err(), "the failure must propagate");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the first step failed before changing anything: restoring would be a lie"
    );
}

#[test]
fn a_failed_enter_undoes_what_it_did() {
    // The case that `Drop` cannot cover, because `Self` is never constructed: the terminal is
    // enabled, the second step fails, and the error travels out of `enter_with` with raw mode
    // still on.
    let (calls, restore) = counter();
    let mut raw_on = false;
    let err = Tui::enter_with(
        || {
            raw_on = true;
            Ok(())
        },
        || Err(std::io::Error::other("EnterAlternateScreen failed")),
        restore,
    );
    assert!(err.is_err(), "the failure must propagate");
    assert!(raw_on, "the fixture must have reached the raw-mode step");
    // Raw mode is ON and `Self` was never built, so `Drop` cannot run: the only thing that can
    // put the terminal back is this call. Measured before the fix: it returned Err with raw
    // mode still on, and the user got a terminal with no echo and no line editing.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the half-entry was left in raw mode: nothing restored it"
    );
}
