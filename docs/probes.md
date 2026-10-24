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

## The docs-only exemption in `pre-push`: the matrix

The guard refuses a push to `main` that carries no `review/*` tag, with one
exemption: a push whose changes are all under `docs/`. "Docs-only" means **not
compiled** — not "not executable by a person". Nothing builds or CI-invokes
`docs/` (no `build.rs`, and `Cargo.toml` has zero references to it), which is the
property that makes the exemption safe. If that ever changes, the prefix must grow
to cover the invoking path too.

Measured adversarially — nine cases, no findings:

| case | expected | measured |
|---|---|---|
| `docs/` only | exempt | `rc=0` · "touches docs/ only — no review tag needed" |
| `docs/` + code in one push | strict | `rc=1` |
| path `src/docs/x.rs` (prefix is anchored `^docs/`) | strict | `rc=1` |
| deleting a code file | strict | `rc=1` |
| executable script created under `docs/` | exempt | `rc=0` (informed acceptance: see above) |
| rename inside `docs/` | exempt | `rc=0` |
| filename containing a **newline** under `docs/` | strict | `rc=1` |
| pushing a change to `.githooks/pre-push` itself | strict | `rc=1` — an exemption covering the guard would be a door that opens from inside |
| new branch (zero remote sha) | strict | `rc=1` |

To reproduce: in a scratch clone, make each change and run
`git push origin HEAD:main` with no `review/*` tag on the tip, then read the exit
code. The newline case holds in both `core.quotePath` settings: with quoting on,
git prints the path in quotes, so it does not start with `docs/`; with quoting
off, the second line (`README.md`) does not match anyway.

### A published tag is not a local tag (H-4)

`git tag --points-at` sees this clone only, so a tag that never left the machine
used to be enough. The hook now asks the remote when it answers:

| case | measured |
|---|---|
| local tag, remote reachable, tag not pushed | `rc=1` · "the remote has no such tag" · "push the tag first" |
| same commit after `git push origin <tag>` | `rc=0` · "the tag is published" |
| remote unreachable | `rc=0` with the warning "the tag was not verified against it" — failing **open**, loudly |

The first version of this check used `case "$remote_tags" in *"refs/tags/$tag"`;
on a multi-line string that pattern only ever looks at the end, so a commit with
one published and one unpublished tag was refused. The case that was *supposed*
to pass found it, not the one supposed to fail.

## Doctests: `no_run`, never `ignore`

`no_run` compiles the example and skips executing it; `ignore` skips *compiling*
too, so the example can rot for months and nothing says so. Measured here:
`src/gh/mod.rs` carried a ```ignore``` block calling `gh(cwd, &args)` — three
arguments, by reference — while the real signature is
`gh(&Binaries, &Path, Vec<OsString>)`: two shapes that cannot both be true.

Switching the same block to `no_run` produced, in order:

```
ignore   → 0 errors (never compiled)
no_run   → E0425: `cwd`, `args`, `gh`, `binaries` not in scope
+ preamble → E0061: "takes 3 arguments but 2 were supplied"
```

The third line is the finding: the defect existed and only compiling the example
surfaced it. A compiled example also catches what a review cannot — `let binaries
= Binaries::detect()?;` shadows the module name, so `binaries::resolve("gh")`
stops resolving; the example needs `gtasks::binaries::resolve`.

Rule: **`no_run` always, `ignore` never.** `ignore` does not fail even when the
symbol it mentions does not exist: it is not a postponed proof, it is an
absent one. It is also the only form in which an example can **describe a
signature that does not exist** — `gh(cwd, &args)` with two arguments while
the real function takes three — without anyone noticing. An example that cannot be made compilable is prose — take it out
of the code block.
