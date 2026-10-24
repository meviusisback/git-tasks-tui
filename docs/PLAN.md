# git-tasks TUI (Rust) — Implementation Plan

> **Rev 3** — folded @coder's architecture review **and** @auditor's security review (3 HIGH / 5 MEDIUM folded as frozen invariants). Full audit: `~/.hermes/cache/scratch/git-tasks-tui-security-review.md`.
> **For Hermes:** implement with `test-driven-development`, one task at a time; `requesting-code-review` before merge; `security-audit` (TREE scope) before any public release.

**Goal:** a terminal app in Rust that reproduces the `git-tasks` Hermes Desktop plugin — pick repos via `gh`, list open PRs/issues, expand details, act on them, create git worktrees and start agentic work (Hermes only in the MVP) in the same UI.

**Architecture:** one binary TUI (ratatui + crossterm, Elm-style `Msg`/`update`) on tokio. GitHub access is `gh` spawned with **argv arrays** (never a shell) and `--json` parsed by serde. Worktrees live **outside the clone** under `$XDG_DATA_HOME/gtasks/worktrees/`, and "does this worktree exist" is answered **only** by `git worktree list --porcelain`. Agentic work runs `hermes -p <profile> chat --query-file -` with the TUI synchronously suspended.

**Tech stack:** Rust stable (edition 2021), `ratatui >= 0.28`, `crossterm` (`event-stream`), `tokio` (with `process`, `signal`), `serde`/`serde_json`, `toml`, `clap`, `directories`, `pulldown-cmark`, `tracing` + `tracing-subscriber`, `anyhow`, `thiserror`, `shell-words`.

**Repo:** new `~/repo/git-tasks-tui` → `meviusisback/git-tasks-tui`, MIT. Binary `gtasks`.

---

## Current context (corrected)

- Plugin reference (live, 2418 lines, sha `9c475df2…`): `~/.hermes/desktop-plugins/git-tasks/plugin.js`. Source repo: `~/repo/git-tasks/desktop/plugin.js` (plus `test/`, `scripts/`, `docs/`).
- Host: Omarchy/Arch, Fish, `gh` 2.103.0 authenticated as `meviusisback`, Hermes v0.22.5 at `~/.local/bin/hermes`, profiles in `~/.hermes/profiles/*`.
- **Rust toolchain installed** (2026-10-24): `rustup 1.29.1` from pacman, toolchain `stable 1.99.1`, `clippy` + `rustfmt` present. `cargo` is at `/usr/sbin/cargo`. → Task 0 done.
- Linux only in the MVP.

## Non-goals (MVP)

- No GitHub API/token backend (v2). No embedded chat. No gateway-based session presence (v2).
- No notifications, no inline review comments, no CI log viewer.

---

## Feature parity matrix

| Plugin feature | MVP in Rust |
|---|---|
| Pick repos (`gh repo list`, manual `owner/repo`) | Yes — selector, persisted |
| Multi-repo list; Open/All/Closed; PRs/Issues; search; group by repo; sort updated | Yes |
| Parallel loading, manual refresh, auto-refresh (single-flight) | Yes — concurrency ≤4, `r`, configurable interval |
| Detail: meta, body, comments, labels, checks, review decision, mergeability | Yes — markdown → text (`pulldown-cmark`) |
| Approve / comment / close / reopen / merge | Yes — all gated (approve included) |
| CI dot | Yes (`statusCheckRollup`) |
| Worktree discover / create / reuse / go-to | Yes — outside the clone; reuse via `git worktree list` |
| Missing clone → `gh repo clone` | Yes, non-existing target + explicit confirm |
| Bot profile + optional skill | Yes — profiles from `~/.hermes/profiles` |
| Session chip | Partial: PID + `/proc` starttime, behind a trait |
| Data source Auto/gh/API | `gh` only |
| Open in browser, copy link | Yes — https + host allowlist |

---

## Module map (frozen)

```
src/main.rs            -- clap args, tracing init, terminal lifecycle, event loop, signals
src/app.rs             -- App { data: DataStore, ui: UiState }, update()
src/msg.rs             -- enum Msg { Key, Tick, FetchDone, DetailDone, ActionDone, Refresh, ... }
src/model.rs           -- serde types + PURE fns: filter_items, sort_items, group_by_repo
src/config.rs          -- TOML config (0600)
src/state.rs           -- JSON state (0600): selected repos, worktree→session map
src/fetch.rs           -- orchestration: semaphore, per-repo isolation, single-flight refresh
src/gh/mod.rs          -- Gh: argv-only spawn, timeout, process-group kill, streaming caps
src/gh/repos.rs        -- repo list / repo view
src/gh/items.rs        -- pr list + issue list
src/gh/detail.rs       -- pr/issue view
src/actions.rs         -- typed Action enum → argv; confirmations; no retry on writes
src/binaries.rs        -- resolve+verify gh/git/hermes (absolute, owner, mode)
src/env.rs             -- child env allowlist (env_clear + explicit vars, never GIT_*)
src/sanitize.rs        -- control/bidi stripping, redaction for logs/errors/UI
src/open.rs            -- xdg-open (https + host allowlist) + clipboard chain
src/editor.rs          -- $EDITOR via shell-words → argv, tempfile
src/worktree.rs        -- discover, path policy, create/remove, reuse from porcelain
src/agent.rs           -- build_command(), build_seed_message(), run_suspended()
src/presence.rs        -- trait PresenceSource + PidPresence (pid + /proc starttime)
src/ui/{mod,layout,repos,list,detail,confirm,settings,status}.rs
```

**State split:** `DataStore` (domain, no crossterm) / `UiState` (pane, selection, scroll, modals, filters) / `App::update(Msg)` unit-testable without terminal or tokio.
**Effects split:** every external effect = pure `build_*()` (argv/cwd/env) + thin `run_*()`.

---

## Security invariants (frozen — each has a named test)

**S1 — Never a shell.** No `sh -c`, no `Command::new("sh")`, no command *strings*. A function returning a command string is a review failure; the plugin's `sq()` must not be ported.

**S2 — Absolute, verified binaries.** `gh`, `git`, `hermes` resolved at startup (config override or fixed trusted list); verify `symlink_metadata` is a regular file, `mode & 0o022 == 0`, `uid == geteuid() || uid == 0`. Never rely on `PATH` per launch.

**S3 — Child env is chosen, not inherited.** `env_clear()` + allowlist (`PATH`, `HOME`, `TERM`, `LANG`/`LC_*`, `TZ`, intentional `GH_*`). **Never** `GIT_*` (`GIT_CONFIG_COUNT/KEY_n/VALUE_n` = arbitrary config → hook RCE; `GIT_DIR` overrides `git -C`; `GIT_ASKPASS`/`GIT_SSH_COMMAND`/`GIT_EXTERNAL_DIFF` same class). `gh`: `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1`, `GH_PAGER=cat`; read commands `stdin(Stdio::null())`.

**S4 — Argument injection controls.** `owner/repo` matches `^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$`; branch via `git check-ref-format --branch`; number as `u64`; `--repo` always explicit. `--` only where the tool accepts it (`git worktree add --` is rc 129 — not there).

**S5 — Worktree paths.** Root `$XDG_DATA_HOME/gtasks/worktrees` (outside the clone: a repo cannot pre-empt a path). Canonicalize the **root** once; slug `^[A-Za-z0-9._-]{1,64}$` (no `.`/`..`); lexical `starts_with(root)`; create **stepwise** with `create_dir` + `symlink_metadata` (never `create_dir_all`/`metadata`); after creation verify `symlink_metadata().is_dir() && !is_symlink()`. Residual same-user TOCTOU documented (or `openat(O_NOFOLLOW|O_DIRECTORY)`).

**S6 — Reuse decided by git, not the filesystem.** `git -C <clone> worktree list --porcelain` is the only source of "exists"; match path AND branch. A committed symlink at a would-be path is never adopted (H-2 proven). Plugin worktrees are adopted only when registered and inside a canonical clone root.

**S7 — Agent launch policy.** Never `--yolo`, `--accept-hooks`, `--tui`. Default `--ignore-rules` (skips `AGENTS.md`/`.cursorrules`/`SOUL.md`/memory/skills — repo-committed instructions cannot steer the agent); settings toggle "trust worktree instructions" re-enables with a warning; `--safe-mode` offered for a first run in a fresh clone. Approvals stay ON.

**S8 — Seed is data, not instructions.** Seed = `repo#N` + URL + the user's own note, fenced `<<<UNTRUSTED … >>>`. **No title/body** (the agent has `gh`; a 40 KB body also breaks `ARG_MAX`). Delivered via **stdin** `--query-file -`, never positional (ps-visible).

**S9 — Actions typed and gated.** One `Action` enum → one argv builder. Forbidden forever, by test: `--admin`, `--force`, `--auto`. Merge = typed confirm; approve and close/reopen = modal; `--delete-branch` is a **separate** action, off by default, second confirm naming the branch (deletes the local branch too). `git worktree remove` never gets `--force`.

**S10 — Stale state re-checked.** Before merge/approve re-fetch `headRefOid` (+ `mergeable`); abort "PR changed, refresh" on mismatch.

**S11 — Processes die as a group.** `kill_on_drop(true)`; `process_group(0)`; timeout/quit → `kill(-pgid)`; `pre_exec` → `prctl(PR_SET_PDEATHSIG, SIGTERM)`. **No retry on writes** (unknown outcome — surface "verify on GitHub"); writes get their own longer timeout.

**S12 — Bounded, streamed I/O.** Two concurrent readers with independent caps (16 MiB stdout / 256 KiB stderr), kill on overflow + `truncated`; parse JSON only from capped bytes. Never check caps after `wait_with_output()`.

**S13 — Output hygiene.** `ratatui >= 0.28` (filters control chars in `Buffer`); pin + test with `"\x1b]52;c;ZXZpbA==\x07"` in a title. Everything printed **outside** the buffer (panic, toast, log) goes through `sanitize::strip_control`. Strip bidi/Cf (U+202E etc., keep `\n`/`\t`) from labels. Redact `gh[pousr]_[A-Za-z0-9]{36,}` and `github_pat_[A-Za-z0-9_]{22,}` from `gh` stderr; never log argv of comment/merge.

**S14 — Files and URLs.** Config/state `0600`, dir `0700`. `xdg-open` only `https://` on `github.com`/`www.github.com`. `gh repo clone` requires a **non-existing** target checked with `symlink_metadata`, under a configured root.

**S15 — Signals.** SIGINT/SIGTERM/SIGHUP → restore terminal, kill child group, exit non-zero. `TuiGuard` on `Drop`; teardown uses `let _ =`, never `unwrap`.

---

## Agent CLI contract (verified v0.22.5)

```
hermes -p <profile> chat --query-file - --in <worktree> [--ignore-rules]
# stdin: the fenced seed
```

- `-p, --profile` — global flag.
- `chat --query-file -` — seed from stdin (mutually exclusive with `-q`); confirm in the Task 1 spike that a TTY session is seeded (not one-shot) and record it in `docs/agent-cli.md`.
- `--in DIR` — working directory.
- `--ignore-rules` — skip `AGENTS.md`/`SOUL.md`/`.cursorrules`/memory/skills injection (S7).
- Never `--yolo` ("Bypass all dangerous command approval prompts"), `--accept-hooks`, `--tui`.

---

## Task list (vertical; after Task 8 the app is usable read-only)

### Task 0 — Toolchain — DONE (2026-10-24)
`rustup 1.29.1` (pacman) + toolchain `stable 1.99.1` + `clippy`/`rustfmt`. Verified `cargo/rustc/clippy/fmt --version`.

### Task 1 — Scaffold + logging + CLI spike
**Files:** `Cargo.toml`, `src/main.rs`, `.gitignore`, `rust-toolchain.toml`, `.github/workflows/ci.yml`, `docs/agent-cli.md`.
**Work:** `cargo init --name gtasks`; deps; `tracing` → `$XDG_STATE_HOME/gtasks/gtasks.log`; CI fmt/clippy/test. Spike: paste real `hermes --help`/`chat --help`; prove `chat --query-file -` seeds a TTY session.
**Verify:** `cargo run -- --version`; log file created; CI green.

### Task 2 — `gh` wrapper (argv-only, bounded, group-killed)
**Files:** `src/gh/mod.rs`, `src/binaries.rs`, `src/env.rs`, `tests/gh_args.rs`, `tests/gh_process.rs`.
**Work:** S2, S3, S4, S11, S12 in the wrapper.
**Tests:** `argv_never_shell_metachars`, `argv_forbids_admin_force_auto`, `binaries_are_absolute_and_not_group_writable`, `child_env_has_no_git_vars`, `hung_gh_kills_process_group`, `stderr_flood_capped`, `huge_json_capped_before_parse`, `both_pipes_drained_no_deadlock`.
**Verify:** `cargo test gh_args gh_process`.

### Task 3 — Models + pure list fns
**Files:** `src/model.rs`, `tests/fixtures/*.json`, `tests/parse.rs`.
**Work:** serde types (camelCase, unknown fields tolerated) + `filter_items`/`sort_items`/`group_by_repo`.
**Tests:** fixtures; invalid types error, no panic; truth tables.
**Verify:** `cargo test parse`.

### Task 4 — Config + state
**Files:** `src/config.rs`, `src/state.rs`, `tests/config_roundtrip.rs`.
**Work:** `Config { repos, clone_roots, worktree_root, parallel, timeout_s, write_timeout_s, refresh_s, default_profile, limit, trust_worktree_rules }`; `0600`/`0700`; `--import-from-plugin`.
**Tests:** round-trip; defaults; invalid → error, no overwrite; `state_and_config_are_0600`; import maps `selectedRepos` only.
**Verify:** `cargo test config_roundtrip`.

### Task 5 — `App::update` skeleton (no fetch)
**Files:** `src/app.rs`, `src/msg.rs`, `src/sanitize.rs`, `src/ui/{mod,layout,status}.rs`.
**Work:** `Msg`, `App::update`, pane focus, modal stack, status line; `TuiGuard`; sanitize helpers.
**Tests:** key→state transitions; one `TestBackend` golden; `TuiGuard` restores after `Child` exit 1; `bidi_chars_stripped_from_labels`; `terminal_escape_in_title_is_inert`.
**Verify:** `cargo test app sanitize`.

### Task 6 — Repo selector
**Files:** `src/ui/repos.rs`, `src/gh/repos.rs`, `tests/repo_select.rs`.
**Work:** `gh repo list --limit 200 --json nameWithOwner,isPrivate`; multi-select; manual add validated (S4); persist; max 50.
**Tests:** reject ``, `../x`, `a b/c`, `a/b/c`, `-x/y`; dedupe; cap.
**Verify:** `cargo test repo_select`.

### Task 7a — Fetch a single repo
**Files:** `src/fetch.rs`, `tests/fetch_one.rs`.
**Work:** `fetch_one(repo, filters)` with the plugin's field lists; `FetchError::{Timeout,OutputCap,Parse}`.
**Tests:** fixture success; timeout; malformed JSON; no panic.
**Verify:** `cargo test fetch_one`.

### Task 8 — Detail pane (+ markdown)
**Files:** `src/gh/detail.rs`, `src/ui/detail.rs`, `tests/detail.rs`.
**Work:** `pr/issue view --json …` (incl. `headRefOid`); markdown → plain text, wrap, scroll; missing fields `—`.
**Tests:** long body wraps; code fence → plain lines; missing fields; scroll bounds.
**Verify:** `cargo test detail`.

### Task 7b — Parallel fetch + filters + single-flight
**Files:** `src/fetch.rs`, `src/app.rs`, `tests/fetch_parallel.rs`.
**Work:** semaphore (≤4), merge, `failed=N`, filters/group/sort, tick, manual `r`, single-flight.
**Tests:** one repo fails → `failed=1`; tick → one batch; `single_flight_refresh`; filters re-render without refetch.
**Verify:** `cargo test fetch_parallel`.

### Task 9 — Actions (typed, gated, no retry)
**Files:** `src/actions.rs`, `src/ui/confirm.rs`, `tests/actions_args.rs`.
**Work:** `enum Action { Comment, Approve, Close, Reopen, Merge{method}, MergeAndDeleteBranch{method}, OpenUrl, CopyLink }` → one argv builder; approve/close/reopen modal; merge typed confirm; delete-branch separate + second confirm; re-fetch `headRefOid`/`mergeable`; no retry on writes.
**Tests:** `merge_and_approve_require_confirm`, `merge_aborts_on_head_sha_change`, `delete_branch_needs_second_confirm_and_defaults_off`, `no_retry_on_write_actions`, forbidden flags across all actions.
**Verify:** `cargo test actions_args`.

### Task 9.5 — Open / clipboard / `$EDITOR`
**Files:** `src/open.rs`, `src/editor.rs`, `tests/open_editor.rs`.
**Work:** `xdg-open` https+host allowlist (S14); clipboard `wl-copy → xclip → xsel` (stdin); editor via `shell-words` on `$EDITOR` (fallback `vi`), tempfile, abort on empty/unchanged.
**Tests:** URL allowlist rejects `file://`, `javascript:`, other hosts; editor non-zero → no comment; argv construction.
**Verify:** `cargo test open_editor`.

### Task 10a — Worktree discovery (read-only)
**Files:** `src/worktree.rs`, `tests/worktree_discover.rs`.
**Work:** scan `clone_roots` for `.git`; `remoteToRepo`; **`git worktree list --porcelain`** as the only truth (S6); adopt plugin worktrees only if registered and inside a canonical root.
**Tests:** remote parsing (`git@`, `https://`, `.git`, `ssh://`); non-git dirs ignored; symlinked root refused.
**Verify:** `cargo test worktree_discover`.

### Task 10b — Worktree create/remove
**Files:** `src/worktree.rs`, `tests/worktree_create.rs`.
**Work:** S5 + S6 + S9: paths under `$XDG_DATA_HOME/gtasks/worktrees`; stepwise creation; post-create verification; remove without `--force` (dirty → show reason, stop).
**Tests:** `committed_symlink_cannot_preempt_worktree_path`, `reuse_only_from_worktree_list_porcelain`, `create_dir_does_not_follow_symlinks`, `worktree_remove_never_force`, `worktree_path_stays_under_root`, collision `-2`; `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_AUTHOR_*` pinned.
**Verify:** `cargo test worktree_create`.

### Task 11a — Agent command + seed (pure)
**Files:** `src/agent.rs`, `tests/agent_argv.rs`.
**Work:** `build_seed_message` (minimal + fenced, S8); `build_command` (S2/S3/S7/S8: absolute `hermes`, env allowlist, `--ignore-rules` per config, stdin seed); profile name `^[a-z0-9_-]+$`.
**Tests:** `agent_argv_has_no_yolo_no_accept_hooks`, `agent_launch_uses_ignore_rules`, `seed_fences_untrusted_text_and_prefers_stdin`, seed never contains body/title.
**Verify:** `cargo test agent_argv`.

### Task 11b — Suspend / run / resume (+ signals)
**Files:** `src/agent.rs`, `src/main.rs`.
**Work:** synchronous suspend window (decision 5), stop tick/fetch, blocking child, unconditional `clear()` + redraw; `TuiGuard`; signal handlers (S15).
**Tests:** stub `hermes` recording cwd/argv exits 1 → TUI restored; `tui_restored_on_signal`.
**Verify:** manual run on a scratch repo.

### Task 12 — Presence behind a trait
**Files:** `src/presence.rs`, `tests/presence.rs`.
**Work:** `trait PresenceSource`; `PidPresence` via `/proc/<pid>/stat` starttime; chip; 10 s refresh.
**Tests:** fake `/proc`; pid reuse → `ended`; missing → `ended`.
**Verify:** `cargo test presence`.

### Task 13 — Settings
**Files:** `src/ui/settings.rs`, `tests/settings.rs`.
**Work:** clone roots, worktree root, parallel, timeouts, limit, refresh, default profile, `trust_worktree_rules` (with warning).
**Tests:** interval 0 rejected; `~` expanded; parallel clamped; toggling trust updates the launch argv.
**Verify:** `cargo test settings`.

### Task 14 — Packaging + docs
**Files:** `README.md`, `install.sh`, `docs/{keys,agent-cli,security}.md`, `LICENSE`.
**Work:** install to `~/.local/bin/gtasks`; keys/config documented; `docs/security.md` states the worktree is untrusted and the agent runs with user credentials and approvals ON.
**Verify:** fresh shell `gtasks --version`.

### Task 15 — Security audit (no new features)
**Files:** `docs/security.md`.
**Work:** `security-audit` TREE + re-run @auditor; fix; add missing tests.
**Verify:** audit PASS, clippy clean, full suite green.

---

## Validation

- `cargo test` (hermetic: fake `gh`/`git`/`hermes`), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- Manual: 3 real repos → list → detail → comment on a scratch issue → worktree → launch Hermes → resume → presence → cleanup.
- Read-only end-to-end against `meviusisback/git-tasks`.

## Risks and tradeoffs

- **`--ignore-rules` also drops SOUL.md/memory** for the session (security vs persona). Default: ignored, toggle available. Documented.
- Two worktree conventions exist (plugin: `.worktrees/<branch-slug>` inside the clone; ours: XDG data dir). Reuse works through `git worktree list`, not path guessing.
- `gh` JSON is the contract; pin a minimum version, keep fixtures.
- PID presence is best-effort; gateway source is v2.
- Same-user TOCTOU on worktree creation accepted and documented (or closed with `openat(O_NOFOLLOW)`).

## Decisions

1. Binary `gtasks`, repo `git-tasks-tui`.
2. Suspend-and-run; `--ignore-rules` by default (toggle).
3. Repo list starts empty; `--import-from-plugin` explicit.
4. No API/token backend in the MVP.
5. Presence via PID+starttime behind a trait; no `state.db` reads.
6. Worktrees outside the clone; reuse only from `git worktree list --porcelain`.

## Remaining questions

- `--json`/headless output mode for scripting?
- PR-head worktree: default or always ask?
- Accept @auditor's offer of ready-made harnesses for the two proven cases (committed-symlink pre-emption, `GIT_CONFIG_COUNT`) once Task 2/10 exist?

---

## Review status

- **@coder (folded):** stack confirmed; module map; DataStore/UiState; pure list fns; `open.rs`/`editor.rs`; presence trait; synchronous suspend; `/proc` starttime; worktree reuse rules; Elm `Msg`; build/run split; security tests moved into tasks; vertical order; `tracing`; markdown; CLI verified.
- **@auditor (folded):** 3 HIGH (agent launch policy, worktree reuse hijack, destructive flags), 5 MEDIUM (path containment, child env/binaries, ungated approve + stale state, process-group kill, streaming caps), 6 LOW (escape/bidi, file modes, redaction, xdg-open, clone target), 4 INFO (keep `sq()` out, `--` caveats, missing toolchain, corrected plugin path). All folded into S1–S15 and per-task tests.
