# git-tasks TUI (Rust) — Implementation Plan

> **Rev 4** — folded @coder's architecture review, @auditor's security review (r0), and @auditor's doc re-review r1 (**C1 blocking: `--query-file -` cannot seed an interactive session; C2 terminal ownership; C3 per-child env; C4 new invariant S16; C5 five point fixes**).
> **Rev 4.1** — Task 1 accepted (scaffold + spike evidence). Its automated seed-file/PTY tests move to Task 11a, with the negative control. Coordination protocol added (ref-only ops, per-agent worktrees, auditors read `origin/*`).
> **Rev 4.2** — r2 review of Task 1: **FAIL** on two S5/S14 blockers (F-1/F-2, both reproduced with the binary) plus 6 findings. Added **Task 1b (hardening)** and settled F-3: one `crossterm`, 0.28 via `ratatui::crossterm`.
> **Rev 4.3** — Task 1b merged to `main` (linear cherry-pick; CI action SHAs corrected). Review r4 measured **G-2** (a `0600` seed in an attacker-writable dir is swappable → instruction injection) and folded it into S8/S14 + Task 11a, together with G-3 (dash negative control) and G-4 (PTY child leak). `portable-pty` stays: the duplicate gate is dev-aware.
> **Rev 4.4** — r5b matrix PASS. **R5-1**: rotation is startup-only, so S14 now says exactly that and Task 5 owns in-session rotation. Task 2 carries the three harness API hooks verbatim (inspectable `Argv`+`env`, pure `resolve`, single `git` composition point).
> **Rev 4.5** — logged L1/L2/L3 merged (`c50ada8`: rotation tests, `try_init`, `..` pre-scan) and **Task 11a-seed merged** (`f3669d2`: `SeedFile`, `ensure_runtime_dir` → `logging::ensure_state_dir`, C1 interactive + dash negative tests, harness vendored in CI). Task 2 API (`f969fa0`) is in review.
> **Rev 4.6** — P2 fixed (`3584d3c`: the init guard runs before any fs work, so a refused second init cannot rotate the live log) and **Task 3 merged** (`c97b316`: serde models + pure filter/sort/group, 8 tests, no panic on malformed JSON).
> **Canonical security text: [`docs/security.md`](security.md) (S1–S16).** If this plan and that file disagree, `docs/security.md` wins.
> **For Hermes:** implement with `test-driven-development`, one task at a time; `requesting-code-review` before merge; `security-audit` (TREE scope) before any public release.

**Goal:** a terminal app in Rust that reproduces the `git-tasks` Hermes Desktop plugin — pick repos via `gh`, list open PRs/issues, expand details, act on them, create git worktrees and start agentic work (Hermes only in the MVP) in the same UI.

**Architecture:** one binary TUI (ratatui + crossterm, Elm-style `Msg`/`update`) on tokio. GitHub access is `gh` spawned with **argv arrays** (never a shell) and `--json` parsed by serde. Worktrees live **outside the clone** under `$XDG_DATA_HOME/gtasks/worktrees/`, and "does this worktree exist" is answered **only** by `git worktree list --porcelain`. Agentic work runs `hermes -p <profile> chat --query-file <seed-file>` (a `0600` file, **not** `-`) with the TUI synchronously suspended and the child owning the terminal.

**Tech stack:** Rust stable (edition 2021), `ratatui >= 0.28`, `crossterm` (`event-stream`), `tokio` (with `process`, `signal`), `serde`/`serde_json`, `toml`, `clap`, `directories`, `pulldown-cmark`, `tracing` + `tracing-subscriber`, `anyhow`, `thiserror`, `shell-words`.

**Repo:** `~/repo/git-tasks-tui` → `meviusisback/git-tasks-tui` (public, MIT). Binary `gtasks`.

---

## Current context

- Plugin reference (live, 2418 lines, sha `9c475df2…`): `~/.hermes/desktop-plugins/git-tasks/plugin.js`. Source repo: `~/repo/git-tasks/desktop/plugin.js`.
- Host: Omarchy/Arch, Fish, `gh` 2.103.0, Hermes v0.22.5 at `~/.local/bin/hermes`, profiles in `~/.hermes/profiles/*`.
- **Rust toolchain installed** (2026-10-24): `rustup 1.29.1` (pacman), toolchain `stable 1.99.1`, `clippy` + `rustfmt`. `cargo` at `/usr/sbin/cargo`. → Task 0 done.
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
src/state.rs           -- JSON state (0600)
src/fetch.rs           -- orchestration: semaphore, per-repo isolation, single-flight refresh
src/gh/mod.rs          -- Gh: argv-only spawn, timeout, group kill, streaming caps
src/gh/repos.rs        -- repo list / repo view
src/gh/items.rs        -- pr list + issue list
src/gh/detail.rs       -- pr/issue view
src/actions.rs         -- typed Action enum → argv; confirmations; no retry on writes
src/binaries.rs        -- resolve+verify gh/git/hermes (absolute, owner, mode)
src/env.rs             -- per-child env allowlists (never GIT_*)
src/sanitize.rs        -- control/bidi stripping, redaction for logs/errors/UI
src/open.rs            -- xdg-open (https + host allowlist) + clipboard chain
src/editor.rs          -- $EDITOR via shell-words → argv, tempfile
src/worktree.rs        -- discover, path policy, create/remove, reuse from porcelain, repo trust (S16)
src/agent.rs           -- build_command(), build_seed_message(), seed file, run_suspended()
src/presence.rs        -- trait PresenceSource + PidPresence (pid + /proc starttime)
src/ui/{mod,layout,repos,list,detail,confirm,settings,status}.rs
```

**State split:** `DataStore` (domain) / `UiState` (pane, selection, scroll, modals, filters) / `App::update(Msg)` unit-testable without terminal or tokio.
**Effects split:** every external effect = pure `build_*()` (argv/cwd/env) + thin `run_*()`.

---

## Security invariants (summary; canonical text in `docs/security.md`)

**S1** never a shell (no command strings; `sq()` not ported). **S2** absolute verified binaries (owner/mode). **S3** per-child env allowlist: gh gets `GH_TOKEN`/`GITHUB_TOKEN`, git and the agent never do; never `GIT_*`. **S4** repo/branch/number validation; `--json`/`--limit` constants. **S5** worktrees outside the clone, stepwise `create_dir` + `symlink_metadata`, dirs `0700`. **S6** reuse only from `git worktree list --porcelain` (path AND branch). **S7** agent flags: never `--yolo`/`--accept-hooks`/`--tui`/`--oneshot`/`--worktree`/`-c`/`-r`; default `--ignore-rules`; profile `^[a-z0-9][a-z0-9_-]{0,63}$`. **S8** seed = repo#N + URL + user note, fenced, **`0600` file via `--query-file <path>`** (never `-`, never argv). **S9** typed actions; `--admin`/`--force`/`--auto` forbidden; delete-branch (merge **and** close) separate + second confirm. **S10** re-check `headRefOid`/`mergeable` before merge/approve. **S11** pipe children: `kill_on_drop`, `process_group(0)`, `kill(-pgid)`, `PDEATHSIG`; no retry on writes. **S12** two streaming readers with caps. **S13** sanitize everything outside the Buffer; strip bidi; redact tokens; **never log argv or the seed**. **S14** `0600`/`0700` files, https+host allowlist, non-existing clone target. **S15** two spawn profiles — pipe children vs interactive child (`tcsetpgrp`, SIGINT forwarded). **S16** repository trust: every git call `-c core.hooksPath=<nonexistent> -c core.fsmonitor=false -c core.pager=cat` + `GIT_TERMINAL_PROMPT=0`; a dir is a clone only if `canonicalize(rev-parse --absolute-git-dir) == canonicalize(<dir>/.git)`; `.git` file redirects rejected.

---

## Agent CLI contract (v0.22.5; full text in `docs/agent-cli.md`)

```
hermes -p <profile> chat --query-file <seed-file> --in <worktree> [--ignore-rules]
```

- `<seed-file>`: regular file `0600` under `$XDG_RUNTIME_DIR/gtasks` (dir `0700`), unlinked after exit.
- **`--query-file -` is forbidden** (C1): with `-` the CLI does `stdin.read()` to EOF, and a non-TTY stdin forces `oneshot` (`gateway_chat.py:377-378`, `:170-171`) — no interaction, no approvals.
- Assert `stdin.isatty() && stdout.isatty()` before spawning; never `--oneshot`.
- Never `--yolo`, `--accept-hooks`, `--tui`, `--worktree`/`-w`, `-c`/`--continue`, `-r`/`--resume`.
- The child owns the terminal while it runs (S15).

---

## Task list (vertical; after Task 8 the app is usable read-only)

### Task 0 — Toolchain — DONE (2026-10-24)
`rustup 1.29.1` + `stable 1.99.1` + `clippy`/`rustfmt`, verified.

### Task 1 — Scaffold + logging + CLI spike — DONE (d96ed17, verified)
**Delivered:** `cargo init --name gtasks` (lib + thin bin), deps, `tracing` → `$XDG_STATE_HOME/gtasks/gtasks.log` (dir 0700 / file 0600), CI fmt+clippy+test, `rust-toolchain.toml`. Verified by the coordinator: `cargo fmt --check`, `clippy -D warnings` clean, `cargo build`, `gtasks 0.1.0`, log line present. PTY spike (`script -qec … --query-file <file>`) shows the session stays interactive (exit 124) — recorded in `docs/agent-cli.md`.
**Deferred to Task 11a** (where `agent.rs` exists): `agent_seed_file_0600_and_unlinked`, the PTY test `agent_stays_interactive_after_first_turn`, **and its negative control** (`--query-file -` must exit 2 — a test that also passes with the bug has no teeth).
**Accepted deviations:** `edition = "2024"` (cargo default on 1.99), `tracing-subscriber` with `env-filter`.

### Task 1b — Hardening after the r2 review (branch `fix/task-01-hardening`, owner @coder)
**Files:** `src/main.rs`, `src/agent.rs`, `tests/security_fs.rs` (or the existing `tests/agent_seed.rs`), `Cargo.toml`, `.github/workflows/ci.yml`, `rust-toolchain.toml`.
All reproduced by the coordinator with the real binary before assignment; all closed **before** Task 2.
- **F-1 (MEDIUM, S5/S14)** `ensure_state_dir` **and the new `agent::ensure_runtime_dir`** both use `DirBuilder::recursive(true)` + chmod on the path: a symlinked dir becomes the target (foreign dir → `0700`, receives the log or the seed). Fix: per-component `create_dir` + `symlink_metadata`, open `O_DIRECTORY|O_NOFOLLOW`, `set_permissions` on the fd.
- **F-2 (LOW, S14)** `open_log_file`: no `O_NOFOLLOW`, mode applied after creation (foreign file → `0600` + log bytes). Fix: `.mode(0o600).custom_flags(O_NOFOLLOW)`, `ELOOP` → stderr fallback; `set_permissions` only as a belt for existing regular files.
- **Tests** (must fail on the old code): `state_dir_symlink_is_not_followed`, `log_file_symlink_is_not_followed` — victim mode/size/content unchanged, `Err` returned. They must exercise the real init path: `--version` exits inside clap **before** logging starts (this is why the manual repro initially looked clean).
- **F-3 (MEDIUM)** two `crossterm` in the tree (0.28.1 via ratatui, 0.29.0 direct). **Decision: option A** — `crossterm = "0.28"`, imports via `ratatui::crossterm::*`; a CI step fails if a duplicate reappears.
- **F-4 (MEDIUM)** CI: pinned action SHAs, `permissions: contents: read`, `--locked`, `components: rustfmt, clippy`, `timeout-minutes: 15`, `concurrency`.
- **F-5 (LOW)** log rotates **at startup** at 5 MiB (`gtasks.log.1`, one generation); in-session re-check carried to Task 5 (R5-1). S14 updated.
- **F-6 (LOW)** toolchain channel `1.99.1`, `rust-version = "1.85"`.
- **F-7 (INFO)** `#![deny(clippy::print_stdout)]`, allow only on the stderr fallback.
- **F-8 (INFO)** `[lints.clippy] all = "deny"`; `cargo machete` once the deps are in use.

### Task 2 — `gh` wrapper (argv-only, bounded, group-killed) + env/binaries
**Files:** `src/gh/mod.rs`, `src/binaries.rs`, `src/env.rs`, `tests/gh_args.rs`, `tests/gh_process.rs`, `tests/env_harness.rs`.
**Work:** S2, S3 (per child), S4, S11, S12. **S16 git flags** live in the git builder (shared with Task 10).
**API hooks required by the harness (review r5b, verbatim):** (a) the `gh` wrapper exposes the built `Argv` **and** the child `env` as inspectable values, so tests assert without spawning; (b) `resolve(clone, slug, branch) -> Result<PathBuf>` as a **pure** function (the Task 10b resolver); (c) a single function where the `git` invocation is composed, so the `-c core.hooksPath=… -c core.fsmonitor=false -c core.pager=cat` assertion has exactly one home.
**Tests:** `argv_never_shell_metachars`, `argv_forbids_admin_force_auto`, `binaries_are_absolute_and_not_group_writable`, `child_env_has_no_git_vars`, `hung_gh_kills_process_group`, `stderr_flood_capped`, `huge_json_capped_before_parse`, `both_pipes_drained_no_deadlock`, **harness B** (`git_config_count_is_stripped` + control, `git_dir_does_not_hijack_c_flag` + control, `agent_env_has_no_gh_token`, `gh_env_knobs_present`).
**Verify:** `cargo test gh_args gh_process env_harness`.

### Task 3 — Models + pure list fns — DONE (`c97b316`)
**Delivered:** `src/model.rs` (serde types with defaults; `repo`/`kind` filled by the fetcher because `gh` does not carry them), `parse_list` returning `Err` on malformed JSON (no panic), `matches`/`filter_items` (case-insensitive over number, title, author, labels), `sort_items` (updated/created/number, asc/desc), `group_by_repo` (first-seen order). Eight tests with inline fixtures — no `gh` needed.
**Files:** `src/model.rs`, `tests/fixtures/*.json`, `tests/parse.rs`.
**Work:** serde types + `filter_items`/`sort_items`/`group_by_repo`.
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
**Carried over (R5-1):** the log currently rotates only at startup; the event loop must re-check the size (interval or a `MakeWriter` that reopens) so a long-running TUI keeps the 5 MiB bound.
**Tests:** key→state transitions; one `TestBackend` golden; `TuiGuard` restores after `Child` exit 1; `bidi_chars_stripped_from_labels`; `terminal_escape_in_title_is_inert`.
**Verify:** `cargo test app sanitize`.

### Task 6 — Repo selector
**Files:** `src/ui/repos.rs`, `src/gh/repos.rs`, `tests/repo_select.rs`.
**Work:** `gh repo list --limit 200 --json nameWithOwner,isPrivate`; multi-select; manual add validated; persist; max 50.
**Tests:** reject ``, `../x`, `a b/c`, `a/b/c`, `-x/y`; dedupe; cap.
**Verify:** `cargo test repo_select`.

### Task 7a — Fetch a single repo
**Files:** `src/fetch.rs`, `tests/fetch_one.rs`.
**Work:** `fetch_one(repo, filters)` with constant field lists; `FetchError::{Timeout,OutputCap,Parse}`.
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
**Work:** `enum Action { Comment, Approve, Close, Reopen, CloseAndDeleteBranch, Merge{method}, MergeAndDeleteBranch{method}, OpenUrl, CopyLink }` → one argv builder; approve/close/reopen modal; merge typed confirm; delete-branch separate + second confirm; re-fetch `headRefOid`/`mergeable`; no retry on writes.
**Tests:** `merge_and_approve_require_confirm`, `merge_aborts_on_head_sha_change`, `delete_branch_needs_second_confirm_and_defaults_off` (merge **and** close), `no_retry_on_write_actions`, forbidden flags across all actions.
**Verify:** `cargo test actions_args`.

### Task 9.5 — Open / clipboard / `$EDITOR`
**Files:** `src/open.rs`, `src/editor.rs`, `tests/open_editor.rs`.
**Work:** `xdg-open` https+host allowlist; clipboard chain (stdin); editor via `shell-words` on `$EDITOR`, tempfile, abort on empty/unchanged; interactive child profile (S15).
**Tests:** URL allowlist rejects `file://`, `javascript:`, other hosts; editor non-zero → no comment; argv construction.
**Verify:** `cargo test open_editor`.

### Task 10a — Worktree discovery + repo trust (read-only)
**Files:** `src/worktree.rs`, `tests/worktree_discover.rs`, `tests/repo_trust.rs`.
**Work:** scan `clone_roots`; `remoteToRepo`; **S16** identity check (`--absolute-git-dir` vs `<dir>/.git`, `.git` file redirect rejected) and git flags; **`git worktree list --porcelain`** as the only truth (S6); adopt plugin worktrees only when registered and canonical inside a clone root.
**Tests:** remote parsing; non-git dirs ignored; symlinked root refused; `repo_hook_does_not_run_on_worktree_add` (+ control without `-c`), `dot_git_file_redirect_is_rejected`.
**Verify:** `cargo test worktree_discover repo_trust`.

### Task 10b — Worktree create/remove
**Files:** `src/worktree.rs`, `tests/worktree_create.rs`.
**Work:** S5 + S6 + S9: paths under `$XDG_DATA_HOME/gtasks/worktrees` (`0700` dirs); stepwise creation; post-create verification; remove without `--force` (dirty → show reason, stop).
**Tests:** **harness A** (`committed_symlink_at_in_clone_path_is_never_adopted`, `outside_root_symlink_blocks_creation`, `worktree_branch_mismatch_is_not_reused`), `reuse_only_from_worktree_list_porcelain`, `create_dir_does_not_follow_symlinks`, `worktree_remove_never_force`, `worktree_path_stays_under_root`, collision `-2`; `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_AUTHOR_*` pinned.
**Verify:** `cargo test worktree_create`.

### Task 11a — Agent command + seed (pure)
**Status: seed half DONE on main (`f3669d2`)** — `SeedFile` (`create_new`, `0600`, post-check, `Drop` unlink), `ensure_runtime_dir` delegating to `logging::ensure_state_dir` (single implementation; closes G-1 and, with the guaranteed private dir, G-2), the live PTY test and the negative control `agent_query_file_dash_exits_2_empty` (verified against v0.22.5: exit 2, "is empty"), harness vendored under `tests/invariants/` with CI steps. **Remaining:** `build_seed_message`, `build_command` (argv, `--query-file <file>`, no `--oneshot`, profile regex), `isatty` precondition, agent env (no `GH_TOKEN`, no `GIT_*`), and `tests/agent_argv.rs`.
**Files:** `src/agent.rs`, `tests/agent_argv.rs`.
**Work:** `build_seed_message` (minimal + fenced, S8); `build_command` → `--query-file <seed-file>` (never `-`), absolute `hermes`, per-child env (no `GH_TOKEN`), `--ignore-rules` per config, profile regex; `isatty` precondition.
**Tests:** `agent_argv_has_no_yolo_no_accept_hooks`, `agent_launch_uses_ignore_rules`, `seed_uses_0600_file_never_dash_or_argv`, seed never contains body/title, `agent_env_has_no_gh_token` (shared with harness B).
**Carried over from review r4 (G-2/G-3/G-4):** reuse the hardened directory helper (extract `fsx::ensure_private_dir` from `logging::ensure_state_dir`) for the runtime dir — a `0600` seed in an attacker-writable directory is swappable, because unlink rights come from the directory, not the file mode (S8). Tests: `runtime_dir_symlink_is_not_followed`, `seed_is_not_written_through_symlinked_dir`, `seed_can_be_replaced_when_the_dir_is_writable` (documents why the dir must be private). Add the C1 negative control `agent_query_file_dash_exits_2_empty` (exit 2 "is empty"); **never** pass a bogus `-p`, because profile validation runs before the query read and yields exit 1. In the live PTY test, kill the child **before** the asserts (or in a `Drop`), otherwise a failed assert leaks a `hermes` session.
**Dev-deps:** keep `portable-pty` (the duplicate gate is dev-aware: `thiserror 1.x` is only `warning(dev-only)`). Do **not** use `script -c` (a command string = S1 class), and note `script -- argv` does not propagate the exit code (measured). If dev-deps must be zero, use `libc::openpty`: real argv, real exit status.
**Verify:** `cargo test agent_argv`.

### Task 11b — Suspend / run / resume (+ terminal ownership)
**Files:** `src/agent.rs`, `src/main.rs`.
**Work:** synchronous suspend window; stop tick/fetch; blocking child; `tcsetpgrp(child)` + SIGINT forwarding; on return `tcsetpgrp(self)` + termios + `clear()` + redraw; `TuiGuard`; signal handlers per S15 (never kill the interactive child on a Ctrl-C aimed at it).
**Tests:** stub `hermes` recording cwd/argv exits 1 → TUI restored; `tui_restored_on_signal`; `interactive_child_is_not_group_killed`.
**Verify:** manual run on a scratch repo.

### Task 12 — Presence behind a trait
**Files:** `src/presence.rs`, `tests/presence.rs`.
**Work:** `trait PresenceSource`; `PidPresence` via `/proc/<pid>/stat` starttime; chip; 10 s refresh.
**Tests:** fake `/proc`; pid reuse → `ended`; missing → `ended`.
**Verify:** `cargo test presence`.

### Task 13 — Settings
**Files:** `src/ui/settings.rs`, `tests/settings.rs`.
**Work:** clone roots, worktree root, parallel, timeouts, limit, refresh, default profile, `trust_worktree_rules` (warning).
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

## Coordination protocol (multiple agents in one repo)

- **Coordinator is ref-only**: no `checkout` in the shared tree; move branches with `git branch -f` / `git worktree`, push refs (`git push origin <branch>:main`), and **always read the remote back** (`git ls-remote`, `gh api …/commits`) after a push — "push said OK" is not evidence it landed. (Learned the hard way: a docs commit sat only in the local ref while main stayed behind.)
- **One dedicated worktree per coding agent** (`../git-tasks-tui-coder`), never the same checkout. Shared trees caused a docs commit to swallow someone else's scaffold, and a reset of the shared branch wiped a review tip twice.
- **A branch under review is frozen**: no reset, no rebase, no new commits. Other work goes to a different branch **and** a different worktree.
- **Tag the reviewed commit** (`review/<task>-rN`) and push the tag: a force-push of the branch can then never lose the exact commit under review.
- **Auditors never touch a working tree**: review from `origin/*` (`git show origin/main:path`), deliver harness files on an `audit/*` branch.
- **Docs are the coordinator's**: agents propose text, the coordinator commits it.
- **Merge is the last step, after PASS.** The merge commit message carries `reviewed-sha: <branch sha>` and `key-file-sha256: <hash of src/main.rs>`, so the verdict→merge chain is verifiable after the fact.
- Every phase ends with a remote read-back (SHA + file content) before it is reported as done.

## Validation

- `cargo test` (hermetic: fake `gh`/`git`/`hermes`), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- Manual: 3 real repos → list → detail → comment on a scratch issue → worktree → launch Hermes → resume → presence → cleanup.
- Read-only end-to-end against `meviusisback/git-tasks`.

## Risks and tradeoffs

- `--ignore-rules` also drops SOUL.md/memory (security vs persona). Default: ignored, toggle available.
- Two worktree conventions (plugin inside the clone, ours in XDG data). Reuse via `git worktree list`.
- `gh` JSON is the contract; pin a minimum version, keep fixtures.
- PID presence is best-effort; gateway source is v2.
- Same-user TOCTOU on worktree creation accepted and documented (or closed with `openat(O_NOFOLLOW)`).
- `tcsetpgrp` semantics are declared from POSIX reasoning; the Task 1 PTY spike must confirm them.

## Decisions

1. Binary `gtasks`, repo `git-tasks-tui`.
2. Suspend-and-run; `--ignore-rules` by default (toggle).
3. Repo list starts empty; `--import-from-plugin` explicit.
4. No API/token backend in the MVP.
5. Presence via PID+starttime behind a trait; no `state.db` reads.
6. Worktrees outside the clone; reuse only from `git worktree list --porcelain`.
7. Seed via a `0600` file (`--query-file <path>`), never `-` and never argv (C1).
8. Two spawn profiles: pipe children vs interactive child (terminal ownership, S15).
9. One `crossterm`: **0.28** through `ratatui::crossterm::{event,terminal}`; no direct 0.29 dependency (one type universe for input and backend).

## Remaining questions

- `--json`/headless output mode for scripting?
- PR-head worktree: default or always ask?

---

## Review status

- **@coder (folded):** stack, module map, DataStore/UiState, pure list fns, `open.rs`/`editor.rs`, presence trait, synchronous suspend, `/proc` starttime, worktree rules, Elm `Msg`, build/run split, tests moved into tasks, vertical order, `tracing`, markdown, CLI verified.
- **@auditor r0 (folded):** 3 HIGH, 5 MEDIUM, 6 LOW, 4 INFO → S1–S15.
- **@auditor r1 (folded):** C1 `--query-file -` blocker (seed file, isatty, PTY test), C2/S15 terminal ownership (two spawn profiles, `tcsetpgrp`), C3/S3 per-child env (gh keeps its token, agent does not), C4 new **S16** repository trust (hooks flags + git-dir identity, both proven), C5 five point fixes (close `-d`, worktree dirs `0700`, extra forbidden agent flags + profile regex, constant `--json`/`--limit`, no seed in logs + sanitize agent output). Harnesses A and B are specified; @auditor will deliver them as files on an `audit/harness-*` branch for Task 2 and 10b.
