# Probes for the execution block (auditor)

Five files, in the order of importance you set. All of them pass on `main@4481268`
(the tree where `CheckedArgv` and the UI are in), and **each one has been seen red**
under its own mutation — that is the only reason to trust the green.

| # | file | vendored as | what it asserts |
|---|---|---|---|
| 1 | `gtt-probe-1-confirmation.rs` | `tests/probe_confirmation_matches_argv.rs` | **weak version**: the confirmation, as a data structure, carries the argv and the sentence names every effect. It compares what the function returns — it would pass even if the *process* received something else |
| 2 | `gtt-probe-2-no-retry.rs` | `tests/probe_no_retry.rs` | a timed-out write runs **exactly once**, and the outcome is `Unknown`, not `Refused` |
| 3 | `gtt-probe-3-terminal.py` | `scripts/probe-terminal.py` | after exit the terminal is **byte-identical** to before (termios), and raw mode was actually observable |
| 1b | `gtt-probe-1b-shown-argv-is-spawned.rs` | `tests/probe_shown_argv_is_the_spawned_argv.rs` | **the strong version of 1**: the argv the process *receives* (a fake `gh` records it) is byte-identical to the one shown, so the test cannot pass by copying a vector |
| 4 | `gtt-probe-4-process-group.rs` | `tests/timeout_group.rs` (already in) | a timeout leaves **no grandchild** alive, and does not stretch the wait |

## Running them

```sh
cargo test --test probe_confirmation_matches_argv
cargo test --test probe_shown_argv_is_the_spawned_argv
cargo test --test probe_no_retry
cargo test --test timeout_group
cargo build && GTASKS_BIN=./target/debug/gtasks python3 scripts/probe-terminal.py
```

Probe 3 is a pty probe on purpose: the terminal state is not observable from
inside the process that changes it, and "I restored the terminal" is exactly the
kind of statement a program makes while the shell has no echo. It exits 2 when it
cannot observe raw mode — **2 is not a pass**, it means the probe could not run.

## The mutations that prove each one bites

**1 is the weak version of 1b**: it verifies the pair `(sentence, argv)` the code
produces, not that the process receives that argv. Keep both — 1 costs nothing —
but when you want to know whether what the user approved is what runs, 1b is the
one that answers, and it is the one that was mutated below.

| probe | mutation | observed |
|---|---|---|
| 1 | drop `", then delete the branch"` from `summarise` | `FAILED — the sentence hides an effect of the argv (--delete-branch)` |
| 1b | make the confirmation hide `--squash` | `FAILED — the process received a different argv from the one shown to the user` |
| 2 | wrap `run_pipe_with_caps` in a retry loop | (to apply) — the probe counts invocations through a file, so a second process is visible |
| 3 | empty `impl Drop for TuiGuard` + neutralise every `disable_raw_mode()` | `after exit: ECHO False, ICANON False` → `FAIL`, exit 1 |
| 4 | `kill(-pid, …)` → `kill(pid, …)` | `left the grandchild alive`, and the timeout goes 1.0 s → 3.0 s |

Probe 2's mutation is the only one not yet executed by me: the current code has no
retry to remove, so the probe is a guard against the future. That is stated rather
than hidden — and it is why probe 2 counts **processes** instead of asserting on
`Executed`: a retry added later would still return `Unknown`.

## Two notes that matter more than the probes

1. **Route the executor through `run_pipe`.** It already spawns with
   `process_group(0)`, sets `PR_SET_PDEATHSIG` and kills the group on timeout. A
   second, private spawn for the agent loses all three at once, and none of them
   is noticed until someone sends SIGTERM. Probe 4 is exactly the guard for that.
2. **Probe 3 tests the exit paths that exist today** (Ctrl-C and the guard's
   `Drop`). When the action runs *inside* the TUI, the property to add is the one
   that is not yet written: while the action runs the terminal must be cooked (or
   the TUI must not be reading keys), and after it returns the TUI must re-enter
   raw mode. That one needs the code to exist; the harness is already pty-based,
   so it is a second scenario in the same script, not a second script.
# z
# z
