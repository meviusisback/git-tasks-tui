# Security invariants

Threat model: the **user is trusted**; everything from outside is hostile — repo names, PR/issue
titles and bodies, branch names, `gh` JSON, clone and worktree contents, `PATH`, the child
environment. Attacker: anyone who can open an issue/PR on a followed repo, or land code in a repo
that gets cloned.

Each invariant has at least one named test. A change that weakens one needs an explicit decision
recorded here.

## S1 — Never a shell
No `sh -c`, no `Command::new("sh")`, no command *strings*. A function returning a command string is
a review failure. (The desktop plugin's `sq()` quoting helper is deliberately **not** ported.)

## S2 — Absolute, verified binaries
`gh`, `git`, `hermes` resolved at startup (config override or a fixed trusted list). Verify with
`symlink_metadata`: regular file, `mode & 0o022 == 0`, `uid == geteuid() || uid == 0`. Never rely on
`PATH` per launch.

## S3 — Child environment is chosen, not inherited
`env_clear()` plus an allowlist (`PATH`, `HOME`, `TERM`, `LANG`/`LC_*`, `TZ`, intentional `GH_*`).
**Never** pass `GIT_*`: `GIT_CONFIG_COUNT/KEY_n/VALUE_n` injects arbitrary git config (hooks,
fsmonitor, pager → code execution), `GIT_DIR` overrides `git -C`, and `GIT_ASKPASS`,
`GIT_SSH_COMMAND`, `GIT_EXTERNAL_DIFF` are the same class. For `gh`: `GH_PROMPT_DISABLED=1`,
`GH_NO_UPDATE_NOTIFIER=1`, `GH_PAGER=cat`; read commands get `stdin(Stdio::null())`.

## S4 — Argument injection controls
`owner/repo` must match `^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$`; branch names are validated with
`git check-ref-format --branch` (also rejects a leading `-`, spaces, `..`, `~^:?*[\`); issue/PR
numbers are parsed as `u64`; `--repo` is always explicit. `--` is used only where the tool accepts
it (`git worktree add --` is a usage error).

## S5 — Worktree paths
Root is `$XDG_DATA_HOME/gtasks/worktrees` — **outside the clone**, so a repository cannot pre-empt a
path by committing a symlink. Canonicalize the root once; slug `^[A-Za-z0-9._-]{1,64}$` excluding
`.` and `..`; require a lexical `starts_with(root)`; create stepwise with `create_dir` +
`symlink_metadata` (never `create_dir_all`/`metadata`, which follow symlinks); after creation
verify `symlink_metadata().is_dir() && !is_symlink()`. The residual same-user TOCTOU is documented;
it can be closed with `openat(O_NOFOLLOW|O_DIRECTORY)`.

## S6 — Reuse is decided by git
`git -C <clone> worktree list --porcelain` is the only source of "this worktree exists"; match the
registered path **and** branch. A path that merely exists on disk is never adopted.

## S7 — Agent launch policy
Never `--yolo`, never `--accept-hooks`, never `--tui`. Default `--ignore-rules` (a repo-committed
`AGENTS.md`/`.cursorrules`/`.hermes.md` cannot steer the agent). A settings toggle may re-enable
worktree instructions with a warning; `--safe-mode` is offered for a first run in a fresh clone.
Approvals stay on.

## S8 — Seed is data, not instructions
Seed = `repo#N` + URL + the user's own note, fenced as UNTRUSTED. No title, no body (the agent reads
them with `gh`). Delivered through **stdin**, never argv.

## S9 — Actions are typed and gated
One `Action` enum feeds one argv builder. Forbidden forever (tested): `--admin`, `--force`,
`--auto`. Merge requires a typed confirmation; approve and close/reopen require a modal;
`--delete-branch` is a separate action, off by default, behind a second confirmation that names the
branch (it deletes the local branch too). `git worktree remove` never receives `--force`.

## S10 — Stale state is re-checked
Before merge/approve, re-fetch `headRefOid` (and `mergeable` for merge) and abort with "PR changed,
refresh" if it differs from what the user saw.

## S11 — Processes die as a group
`kill_on_drop(true)`, `process_group(0)`, `kill(-pgid)` on timeout/quit, and
`prctl(PR_SET_PDEATHSIG, SIGTERM)` in `pre_exec`. **No retry on write actions** — a killed comment
or merge has an unknown outcome; surface "verify on GitHub". Writes get their own longer timeout.

## S12 — Bounded, streamed I/O
Two concurrent reader tasks with independent caps (16 MiB stdout, 256 KiB stderr), killed on
overflow with a `truncated` flag; JSON is parsed only from capped bytes. Caps are applied while
streaming, never after `wait_with_output()`.

## S13 — Output hygiene
`ratatui >= 0.28` filters control characters in `Buffer`; pin it and keep a regression test with an
OSC escape in a title. Anything printed **outside** the buffer (panic, toast, log) goes through
`sanitize::strip_control`. Strip bidi/Cf characters (U+202E …) from labels. Redact
`gh[pousr]_[A-Za-z0-9]{36,}` and `github_pat_[A-Za-z0-9_]{22,}` from `gh` stderr before log/UI;
never log the argv of comment/merge actions.

## S14 — Files and URLs
Config and state files are created `0600` (directory `0700`). `xdg-open` is called only for
`https://` URLs on `github.com` / `www.github.com`. `gh repo clone` requires a **non-existing**
target checked with `symlink_metadata`, under a configured root.

## S15 — Signals
SIGINT/SIGTERM/SIGHUP restore the terminal, kill the child process group and exit non-zero.
`TuiGuard` restores on `Drop` (panic included); teardown uses `let _ =`, never `unwrap`.
