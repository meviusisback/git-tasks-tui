//! One owner for "put the terminal back".
//!
//! The terminal is the only thing a TUI can break that the user cannot fix by pressing a key,
//! and today three mechanisms restore it — the guard's `Drop`, a `panic::set_hook` in
//! `run_tui`, and the SIGHUP/SIGINT handlers — **none of them supervised**. Each looks like it
//! covers the others, which is the most polite form of a defect.
//!
//! This module makes the fact have one owner, and the property is **exactly once**:
//!
//! * **normal exit, `?` or unwinding** → the restore runs once, from `Drop`;
//! * **a panic inside the scope** → the hook runs it, `Drop` finds it done, still once;
//! * **a half-entry** (raw mode on, alternate screen failed) → `enter_with` runs it, because
//!   `Self` is never constructed and `Drop` will never exist;
//! * **a panic after the guard is gone** → nothing: the hook finds no active guard.
//!
//! ## The thing std does not allow, and what it forces
//!
//! Restoring the previous panic hook from `Drop` **cannot work**: a `Drop` that runs during
//! unwinding has `thread::panicking() == true`, and `panic::set_hook` panics in that state
//! ("cannot modify the panic hook from a panicking thread") — a panic inside unwinding, which
//! aborts the process. Measured: the first version of this module aborted with *"thread caused
//! non-unwinding panic"* on exactly the case the hook exists for.
//!
//! So the hook is installed **once**, permanently, and chains to the hook it replaced (a panic
//! must still print). It reaches the guard through a `Weak`, so when no guard is alive it does
//! nothing at all. The guard owns the **action**; the hook is a client of it, not a second
//! owner.
//!
//! ## Signals: already single-owner, and the claim that used to be here was wrong
//!
//! The first version of this note said the SIGHUP/SIGTERM/SIGINT handlers "belong to the same
//! `restore()` and must move here". Read against `main.rs`, that is **not** what they do and not
//! what they need: the handler stores into an atomic (`TERMINATE`) — the only async-signal-safe
//! thing it must do — and the event loop's condition (`while !app.quit && !TERMINATE.load(..)`)
//! leaves the loop, which drops this guard, which restores. There is no second restore path to
//! consolidate: the signal path already ends in `Drop`.
//!
//! What the handlers must **never** do is call the terminal functions themselves: `disable_raw_mode`
//! and `execute!` allocate and lock, and neither is async-signal-safe. A handler that "restored"
//! directly would look more robust and be the fragile version.
//!
//! So what remains unsaid is not ownership but **witness**: no test exercises the signal path,
//! because the loop lives in the binary. It is declared here rather than pretended.

use std::io;
use std::panic::{self, PanicHookInfo};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// The action that puts the terminal back: `disable_raw_mode` + `LeaveAlternateScreen` at the
/// real call site, a counter in a test.
type Restore = Box<dyn FnMut() + Send>;

type HookFn = dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static;

type Enable = Box<dyn FnMut() -> io::Result<()> + Send>;

struct Shared {
    enable_raw: Mutex<Enable>,
    enable_screen: Mutex<Enable>,
    restore: Mutex<Restore>,
    /// True while the terminal belongs to this guard. `restore_once` and `resume` swap it, so
    /// each ownership window restores exactly once — two windows, two restores, never three.
    live: AtomicBool,
}

impl Shared {
    /// Run the restore **once**. A panic runs the hook first and `Drop` during unwinding, and
    /// only the first of them may act — otherwise two mechanisms are ordering themselves
    /// against each other (the visible symptom is the error message printed *after* leaving the
    /// alternate screen, where nobody reads it).
    fn restore_once(&self) {
        if !self.live.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut restore) = self.restore.lock() {
            (restore)();
        }
    }
}

/// The guard currently in charge of the terminal, if any.
static ACTIVE: OnceLock<Mutex<Weak<Shared>>> = OnceLock::new();

/// The hook this module replaced, kept so a panic still prints.
static PREVIOUS: OnceLock<Mutex<Option<Arc<HookFn>>>> = OnceLock::new();

fn active() -> &'static Mutex<Weak<Shared>> {
    ACTIVE.get_or_init(|| Mutex::new(Weak::new()))
}

fn previous() -> &'static Mutex<Option<Arc<HookFn>>> {
    PREVIOUS.get_or_init(|| Mutex::new(None))
}

/// Install the chaining hook once, for the whole process.
fn install_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let replaced: Arc<HookFn> = Arc::from(panic::take_hook());
        if let Ok(mut slot) = previous().lock() {
            *slot = Some(Arc::clone(&replaced));
        }
        panic::set_hook(Box::new(move |info| {
            // 1. If a guard is alive, let it restore. `Weak` means a dropped guard is gone, so
            //    a panic after the TUI exits does not reach back into a finished terminal.
            let guard = active().lock().ok().and_then(|w| w.upgrade());
            if let Some(shared) = guard {
                shared.restore_once();
            }
            // 2. Then print, exactly as the replaced hook would have: a hook that swallows the
            //    message turns every panic into silence, which is how a crash becomes a mystery.
            if let Some(prev) = previous().lock().ok().and_then(|slot| slot.clone()) {
                prev(info);
            }
        }));
    });
}

/// The one implementation of "take the terminal back": entry and resume share it, so the
/// half-entry rule cannot drift between them.
fn resume_shared(shared: &Shared) -> io::Result<()> {
    let raw = {
        let mut f = shared
            .enable_raw
            .lock()
            .map_err(|_| io::Error::other("enable_raw lock poisoned"))?;
        (f)()
    };
    raw?;
    let screen = {
        let mut f = shared
            .enable_screen
            .lock()
            .map_err(|_| io::Error::other("enable_screen lock poisoned"))?;
        (f)()
    };
    match screen {
        Ok(()) => {
            shared.live.store(true, Ordering::SeqCst);
            Ok(())
        }
        Err(err) => {
            // Undo the raw-mode step we just did, and stay not-live.
            if let Ok(mut restore) = shared.restore.lock() {
                (restore)();
            }
            Err(err)
        }
    }
}

/// Suspend the terminal **through the active guard**, for a call site that cannot thread a
/// `&Tui` (the guard lives in the event loop; the agent handoff is two levels below it).
///
/// `false` means "no guard owns the terminal" — an honest answer, so the caller can say there is
/// nothing to hand over instead of pretending.
pub fn suspend_active() -> bool {
    let guard = active().lock().ok().and_then(|w| w.upgrade());
    match guard {
        Some(shared) => {
            shared.restore_once();
            true
        }
        None => false,
    }
}

/// Take the terminal back through the active guard. Same code as [`Tui::resume`].
pub fn resume_active() -> io::Result<()> {
    let guard = active().lock().ok().and_then(|w| w.upgrade());
    match guard {
        Some(shared) => resume_shared(&shared),
        None => Err(io::Error::other("no guard owns the terminal")),
    }
}

/// Owns the terminal for as long as it lives.
pub struct Tui {
    shared: Arc<Shared>,
}

impl Tui {
    /// Enter the real terminal: raw mode, alternate screen, mouse capture,
    /// and the chaining panic hook.
    pub fn enter() -> io::Result<Self> {
        Self::enter_with(
            crossterm::terminal::enable_raw_mode,
            || {
                crossterm::execute!(
                    io::stdout(),
                    crossterm::terminal::EnterAlternateScreen,
                    crossterm::event::EnableMouseCapture
                )
            },
            Box::new(|| {
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = crossterm::execute!(
                    io::stdout(),
                    crossterm::terminal::LeaveAlternateScreen,
                    crossterm::event::DisableMouseCapture,
                    crossterm::cursor::Show
                );
            }),
        )
    }

    /// Enter with the actions supplied by the caller — the shape that makes the guard testable,
    /// and the only difference between this and [`Tui::enter`].
    pub fn enter_with<A, B>(
        mut enable_raw: A,
        mut enable_screen: B,
        restore: Restore,
    ) -> io::Result<Self>
    where
        A: FnMut() -> io::Result<()> + Send + 'static,
        B: FnMut() -> io::Result<()> + Send + 'static,
    {
        // 1. Nothing changed yet, so a failure here restores nothing.
        enable_raw()?;

        // 2. The terminal IS in raw mode now. A failure here must undo it: `Self` is never
        //    constructed, so `Drop` never runs, and without this the user is left with no echo
        //    and no line editing and no message saying why.
        let mut restore = restore;
        if let Err(err) = enable_screen() {
            (restore)();
            return Err(err);
        }

        // 3. Only now is there something to guard.
        install_hook();
        let shared = Arc::new(Shared {
            enable_raw: Mutex::new(Box::new(enable_raw)),
            enable_screen: Mutex::new(Box::new(enable_screen)),
            restore: Mutex::new(restore),
            live: AtomicBool::new(true),
        });
        if let Ok(mut slot) = active().lock() {
            *slot = Arc::downgrade(&shared);
        }
        Ok(Self { shared })
    }
}

impl Tui {
    /// Hand the terminal over: leave raw mode and the alternate screen, **once**. The guard
    /// keeps nothing and claims nothing until [`Tui::resume`]; a panic in between restores
    /// nothing, because there is nothing of ours to restore.
    pub fn suspend(&self) {
        self.shared.restore_once();
    }

    /// Take the terminal back after [`Tui::suspend`]. The half-entry rule applies again: if the
    /// second step fails, the first is undone before the error travels, and the guard stays
    /// not-live — so a later `drop` does not restore a terminal it does not own.
    pub fn resume(&self) -> io::Result<()> {
        resume_shared(&self.shared)
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // No `set_hook` here on purpose: during unwinding it would panic and abort (see the
        // module docs). And no explicit clearing of the active slot either: dropping
        // `self.shared` kills the last `Arc`, so the hook's `Weak` stops upgrading by
        // construction. Measured — a version that cleared the slot in `Drop` passed every test
        // exactly like this one, because the clear changed nothing. An explicit step that
        // cannot be missed because it cannot matter is the `test -x` class in miniature.
        self.shared.restore_once();
    }
}
