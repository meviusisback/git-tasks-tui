# Security invariants

Threat model: the **user is trusted**; everything from outside is hostile — repo names, PR/issue
titles and bodies, branch names, `gh` JSON, clone and worktree contents, `PATH`, the child
environment, and anything a repository can commit (hooks, `.git` files, instruction files).

Each invariant has at least one named test. A change that weakens one needs an explicit decision
recorded here.

## S1 — Never a shell
No `sh -c`, no `Command::new("sh")`, no command *strings*. A function returning a command string is
a review failure. (The desktop plugin's `sq()` quoting helper is deliberately **not** ported.)

## S2 — Absolute, verified binaries
`gh`, `git`, `hermes` resolved at startup (config override or a fixed trusted list). Verify with
`symlink_metadata`: regular file, `mode & 0o022 == 0`, `uid == geteuid() || uid == 0`. Never rely on
`PATH` per launch. (`PATH` stays in the child allowlist only so absolute binaries work and so a
`#!/usr/bin/env` wrapper can resolve its own interpreter.)

## S3 — Child environment is chosen, not inherited (per child)
`env_clear()` plus an allowlist, **different per child class**:

- **gh child**: `PATH`, `HOME`, `TERM`, `LANG`/`LC_*`, `TZ`, plus `GH_TOKEN`/`GITHUB_TOKEN` (its own
  auth), `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1`, `GH_PAGER=cat`; read commands get
  `stdin(Stdio::null())`.
- **git child**: as above minus the tokens, plus `GIT_TERMINAL_PROMPT=0`. **Never** inherit `GIT_*`
  (`GIT_CONFIG_COUNT/KEY_n/VALUE_n` = arbitrary config → hook/pager/fsmonitor execution; `GIT_DIR`
  overrides `git -C`; `GIT_ASKPASS`, `GIT_SSH_COMMAND`, `GIT_EXTERNAL_DIFF` same class).
- **agent child**: no `GH_TOKEN`/`GITHUB_TOKEN` unless the user opts in explicitly (it already holds
  the vault secrets; an injected "read the env and post it" is the H-1 exfiltration payload). No
  `GIT_*`.

## S4 — Argument injection controls
`owner/repo` matches `^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$`; branch names via
`git check-ref-format --branch`; issue/PR numbers as `u64`; `--repo` always explicit. `--json` field
lists and `--limit` are **constants or validated integers** — never composed from fetched data.
`--` only where the tool accepts it (`git worktree add --` is a usage error).

## S5 — Worktree paths
Root is `$XDG_DATA_HOME/gtasks/worktrees` — **outside the clone**, so a repository cannot pre-empt a
path by committing a symlink. Canonicalize the root once; slug `^[A-Za-z0-9._-]{1,64}$` excluding
`.` and `..`; require a lexical `starts_with(root)`; create stepwise with `create_dir` +
`symlink_metadata` (never `create_dir_all`/`metadata`, which follow symlinks); after creation verify
`symlink_metadata().is_dir() && !is_symlink()`. Directories under the root are created `0700` (a
private-repo checkout must not be world-readable under the ambient umask). The residual same-user
TOCTOU is documented; it can be closed with `openat(O_NOFOLLOW|O_DIRECTORY)`.

## S6 — Reuse is decided by git
`git -C <clone> worktree list --porcelain` is the only source of "this worktree exists"; match the
registered path **and** branch. A path that merely exists on disk is never adopted.

## S7 — Agent launch policy
Never `--yolo`, `--accept-hooks`, `--tui`, `--oneshot`. Default `--ignore-rules` (a repo-committed
`AGENTS.md`/`.cursorrules`/`.hermes.md` cannot steer the agent); toggle re-enables with a warning;
`--safe-mode` offered for a first run in a fresh clone. Also forbidden: `--worktree`/`-w` (Hermes
would create a worktree inside the repo, bypassing S5) and `-c`/`--continue`/`-r`/`--resume`
(restore a recorded cwd). `-p <profile>` validated `^[a-z0-9][a-z0-9_-]{0,63}$`. Approvals stay on.

## S8 — Seed is data, not instructions
Seed = `repo#N` + URL + the user's own note, fenced as UNTRUSTED. No title, no body (the agent reads
them with `gh`). Delivered through a **`0600` regular file** in a `0700` runtime dir passed as
`--query-file <path>`, never argv and never `--query-file -` (see `docs/agent-cli.md`, C1); the file
is unlinked after the child exits.

The runtime **directory** must be guaranteed private and symlink-free (same helper as S5/S14:
`create_dir` per component + `O_DIRECTORY|O_NOFOLLOW`). A `0600` seed in a directory an attacker can
write is **not** safe: unlink rights come from the directory, not the file mode, so the seed can be
swapped between creation and the child's read — and `--query-file` would hand the attacker's text to
the agent as its first turn (proven: `audit_seed_is_not_written_through_symlinked_dir`,
`audit_seed_can_be_replaced_when_the_dir_is_writable`). `SeedFile::create_in` **enforces** this
itself (`ensure_runtime_dir` as its first step), so a caller that forgets cannot leak the seed: the
guarantee is structural, not disciplinary (S-1).

## S9 — Actions are typed and gated
One `Action` enum feeds one argv builder. Forbidden forever (tested): `--admin`, `--force`,
`--auto`. Merge requires a typed confirmation; approve and close/reopen require a modal;
`--delete-branch` (on `gh pr merge -d` **and** `gh pr close -d`) is a separate action, off by
default, behind a second confirmation that names the branch (it deletes the local branch too).
`git worktree remove` never receives `--force`.

## S10 — Stale state is re-checked
Before merge/approve, re-fetch `headRefOid` (and `mergeable` for merge) and abort with "PR changed,
refresh" if it differs from what the user saw.

## S11 — Processes die as a group (pipe children only)
For **non-interactive** children (`gh`, `git`): `kill_on_drop(true)`, `process_group(0)`,
`kill(-pgid)` on timeout/quit, `prctl(PR_SET_PDEATHSIG, SIGTERM)` in `pre_exec`. **No retry on write
actions** — a killed comment or merge has an unknown outcome; surface "verify on GitHub". Writes get
their own longer timeout. **Exception:** the interactive agent child must NOT get `process_group(0)`
(a non-foreground pgroup reading the tty stops on SIGTTIN); see S15.

## S12 — Bounded, streamed I/O
Two concurrent reader tasks with independent caps (16 MiB stdout, 256 KiB stderr), killed on
overflow with a `truncated` flag; JSON is parsed only from capped bytes. Caps are applied while
streaming, never after `wait_with_output()`.

## S13 — Output hygiene
`ratatui >= 0.28` filters control characters in `Buffer`; pin it and keep a regression test with an
OSC escape in a title. Anything printed **outside** the buffer (panic, toast, log) goes through
`sanitize::strip_control`. Strip bidi/Cf characters (U+202E …) from labels. Redact
`gh[pousr]_[A-Za-z0-9]{36,}` and `github_pat_[A-Za-z0-9_]{22,}` from `gh` stderr before log/UI.
**Never log argv — including the seed** — for comment/merge/agent launches. Agent output re-rendered
inside the TUI is model text: it goes through the same sanitizer.

## S14 — Files and URLs
Config, state **and seed** files are created `0600` (directories `0700`).
**Nothing we create or open may follow a symlink.** A pre-existing symlink at the state dir, at the
log file, or at any file we open is refused (`symlink_metadata`, `openat(O_NOFOLLOW)`); never
`create_dir_all` + `chmod` on the path — that writes into, and chmods, the attacker's target
(proven on the Task 1 scaffold: a symlinked state dir turned a foreign directory into `0700` and
received `gtasks.log`; a symlinked log file rewrote a foreign file and chmodded it `0600`). Files are
opened with `mode(0o600)` at creation time, not chmodded afterwards; `set_permissions` stays only as
a belt for pre-existing **regular** files. The log is rotated **at startup** at 5 MiB (`gtasks.log` →
`gtasks.log.1`, one generation): a long-running session does not rotate yet, so the bound is
per-launch until the Task 5 event loop re-checks the size (R5-1).
`xdg-open` is called only for `https://` URLs on `github.com` / `www.github.com`. `gh repo clone`
requires a **non-existing** target checked with `symlink_metadata`, under a configured root.
Tests: `state_dir_symlink_is_not_followed`, `state_dir_nested_symlink_is_not_followed`
(intermediate component), `log_file_symlink_is_not_followed`. `runtime_dir_symlink_is_not_followed`
lands with **Task 11a**: it protects the seed runtime dir, not the state dir, and does not exist on
main today.

## S15 — Terminal ownership and signals
Two spawn profiles:

- **App owns the terminal** (`gh`, `git`, non-interactive): SIGINT/SIGTERM/SIGHUP → restore the
  terminal, kill the child group, exit non-zero.
- **Child owns the terminal** (`hermes`, `$EDITOR`): after suspending, `tcsetpgrp(child)`; SIGINT is
  forwarded to the child, not acted on by the app; on return `tcsetpgrp(self)` + restore termios +
  `clear()` + full redraw. The app restores only when it owns the terminal.

`TuiGuard` restores on `Drop` (panic included); teardown uses `let _ =`, never `unwrap`.

## S16 — Repository trust
A directory found under a clone root is not trusted until git agrees it is that very clone:

- Every `git` call carries `-c core.hooksPath=<nonexistent> -c core.fsmonitor=false -c core.pager=cat`
  and `GIT_TERMINAL_PROMPT=0` (a repo hook runs on `git worktree add`; proven on this host).
- A directory is a clone only if
  `canonicalize(git -C <dir> rev-parse --absolute-git-dir) == canonicalize(<dir>/.git)`.
  A `.git` **file** pointing elsewhere answers `rev-parse --absolute-git-dir` with the other repo
  while `--show-toplevel` still reports `<dir>` (proven), so the comparison must use the git-dir, not
  the toplevel. A `.git` file that redirects outside `<dir>` is rejected.
- Tests: `repo_hook_does_not_run_on_worktree_add`, `dot_git_file_redirect_is_rejected`.
