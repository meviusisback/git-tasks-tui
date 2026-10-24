# audit-harness — executable fixtures for docs/security.md

Role: these are the **fixtures and the controls** from the r0/r1 audits, extracted from the
throwaway commands I ran by hand into something that runs on every commit. They are
API-independent on purpose, so they have teeth **before** the Rust code that will use them exists.

Author: @auditor. Nothing here is wired into `cargo test` yet — the wiring lands with `task/02`
(`gh` wrapper) and `task/10b` (worktree resolver), which is when the Rust sides start to exist.

## Layout

```
lib.sh          helpers: isolated git config, assertions, counters.  (no `set -e` — see note)
fixtures.sh     fixture builders (repo with committed symlink, repo hook, .git-file redirect,
                hostile XDG_STATE_HOME, hostile gtasks.log)
controls.sh     CONTROL CASES: prove the threat is real (git/fs behaviour, no app needed)
invariants.sh   ASSERTION SUITE against the gtasks binary (S7/S14 today; S1–S6, S16 wired later)
run_all.sh      entry point
fix-task01-hardening.patch   the F-1/F-2 fix, verified to apply to d96ed17 and to turn the suite green
```

## Run

```sh
bash run_all.sh                                        # controls only, no binary needed
bash run_all.sh --bin ./target/debug/gtasks            # controls + invariants
bash run_all.sh --suite invariants --bin ./target/debug/gtasks
```

Exit code 0 only if every executed case passed. Skips are allowed and printed loudly — a silent
pass is how a suite dies.

## Measured result (2026-10-24, this host)

| Build | Result |
|---|---|
| controls, no binary | **16 passed, 0 failed** — every threat reproduces |
| invariants vs `d96ed17` (as pushed) | **3 passed, 4 FAILED** — `inv_state_dir_symlink_not_followed` (victim dir 755 → 700, log written inside) and `inv_log_symlink_not_followed` (victim file 24 → 205 bytes, 644 → 600) |
| invariants vs `d96ed17` + `fix-task01-hardening.patch` | **7 passed, 0 failed, 7 skipped** (skips = not wired yet) |

That pair is the point: the same assertions fail on the current branch and pass on the fixed
build, so they are not decoration.

## Why the controls exist

An assertion that passes both with and without the bug proves nothing. Each `CTL` case pins the
platform behaviour the matching invariant depends on:

| Control | Pins |
|---|---|
| `CTL A1` | a committed symlink survives `git clone`; `git worktree add` refuses the path (`rc 128`, "already exists"); `worktree list --porcelain` does **not** list it |
| `CTL A2` | `mkdir` on an existing symlink → `EEXIST`; `metadata()` follows and reports a dir; `O_NOFOLLOW\|O_DIRECTORY` refuses |
| `CTL F-1/F-2` | `chmod` through a symlink hits the target — i.e. `set_permissions(path)` after `create_dir_all` is a chmod-clobber primitive |
| `CTL B1` | `GIT_CONFIG_COUNT/KEY_0/VALUE_0` injects arbitrary git config (`core.hooksPath` = code execution) |
| `CTL B2` | `GIT_DIR` overrides `git -C` (`-C` is not isolation) |
| `CTL C1` | `git worktree add` **runs** the repo's `post-checkout` hook; `-c core.hooksPath=/nonexistent-hooks` suppresses it |
| `CTL C2` | a `.git` **file** redirects the git dir; `--absolute-git-dir` follows it, `--show-toplevel` does not |

If a control ever fails, the platform changed and the invariant next to it is no longer testing
anything — fix the control first.

## What is NOT wired yet (and the API it needs)

| Skipped assertion | Needs |
|---|---|
| `argv_never_shell_metachars`, `argv_forbids_admin_force_auto`, `child_env_has_no_git_vars` | `task/02` — a `gh` wrapper exposing the constructed `Argv` and the child `env` for inspection |
| `reuse_only_from_worktree_list_porcelain`, `outside_root_symlink_blocks_creation`, `worktree_branch_mismatch_is_not_reused` | `task/10b` — the worktree resolver |
| `repo_hook_does_not_run_on_worktree_add`, `dot_git_file_redirect_is_rejected` | `task/10b` — the single `git` call site (must carry `-c core.hooksPath=…`) |

Until those exist, the fixtures still run: the controls prove the threat, and any Task-2/10b test
can call `fixtures.sh` directly instead of inventing its own setup.

## Design rules (learned the hard way)

- **No `set -e`.** Every assertion inspects a non-zero status; errexit would abort at the first
  expected failure. It cost one debugging round here, hence the comment in `lib.sh`.
- **Isolated git config** (`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`) and an explicit
  `-c user.email/name` on every fixture commit: a developer's `~/.gitconfig` must never change a
  result.
- **Pipes eat status.** `cmd | head` reports `head`'s exit code; the runner captures status into a
  variable and prints the true code (this bit me while verifying, ironically).
- **One temp dir per case**, removed on return; no fixture writes into `$HOME`.
- **Never `pip install` / `cargo add` to "check" something** — resolving is passive, installing runs
  scripts. Same rule as the registry-existence probe in the audit skill.
- **Assert the code path actually ran, not just the exit code.** `gtasks --version` exits inside clap
  *before* `init_file_logging`, so a repro driven by `--version` never reaches the code under test —
  that trap produced an invalid reproduction once. Every invariant therefore asserts a side
  artefact (non-empty log, victim mode unchanged), never just "the process exited 0".
- **Gate duplicates on the crates you care about, not on all of them.** `cargo tree -d` legitimately
  reports `syn 2/3`, `unicode-width 0.1/0.2`, `hashbrown 0.15/0.17`, `windows-sys 0.59/0.61` in a
  healthy tree; a CI step that fails on *any* duplicate would fail on day one. Check a critical set
  (`crossterm`, `ratatui`, `tokio`, `serde`, `serde_json`, `clap`, `tracing`, `anyhow`).
