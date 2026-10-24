# Review ledger

One row per reviewed tag. Written by the auditor, committed by the coordinator. The point is that
**the verdict lives next to the tag**, so no one has to chase a DM to know whether a SHA is approved —
and the merge message only has to point here.

Rule: a tag is `PASS` only if its row carries the hashes that were actually reviewed. A row whose
branch moved afterwards is stale by definition and must be re-tagged.

| Tag | Commit | Verdict | What was verified | Open at verdict time |
|---|---|---|---|---|
| `review/task01-r3` | `32f2a71` | **PASS** | F-1…F-8 closed; `invariants` 4/4 → **8/0** vs the 4 failures on `d96ed17`; controls 16/16; gate rc 0; both action SHAs resolved at the GitHub API | none |
| `review/logging-l1l3` | `d115b70` | **PASS** | L1 rotation (threshold parameterised, `.1` bytes + `0600`), L2 `try_init`, L3 pre-scan for `..`; 10 tests | **P2** — a failed second init still rotated the live log (proved: `backup created = true`) |
| `review/logging-p2` | `3584d3c` | **PASS** | P2 closed with the guard taken **before** any fs work; probe: `backup created = false`, `inode unchanged = true`, `len unchanged = true`; 13 tests | none |
| `review/task11a-r2` | `f3669d2` | **PASS** | G-1 closed structurally (`ensure_runtime_dir` → `logging::ensure_state_dir`); seed `0600` + `create_new` + `Drop` unlink; both live tests `#[ignore]`; harness vendored and run by CI (controls 16/0, invariants 8/0, gate rc 0) | **S-1** — `SeedFile::create_in` did not call `ensure_runtime_dir`: G-2 closed by discipline, not by construction |
| `review/task11a-r3` | `4044d2e` | **PASS** | **S-1 closed**: `create_in` calls `ensure_runtime_dir(dir)?` first → G-2 closed **by construction**; the three seed probes green on main | none for this tag |
| `review/task03-r1` | `c97b316` | **PASS** | no panic surface (grep + probe: `""`, `null`, `{}`, `[{}]`, negative / 2^64 / null / string `number`, 200-deep nesting, duplicate keys all `Err`); payload **cannot spoof** `repo`/`kind` (`#[serde(skip)]`); 21 tests | M-1…M-5 (INFO), all addressed later (M-2 in S12, M-5 by `sort_assumes_z_timestamps`) |
| `review/task02-r1` | `f969fa0` | **FAIL** | S1 PASS, S3 PASS (B1/B2 with controls), S12 PASS, S16 single composition point | T2-1 `resolve` ignores `slug`; T2-2 no absolute-`program` check; T2-3 vacuous flag test |
| `review/task02-r2` | `cc9db6e` | **FAIL** | value-based `env_from` (dirty parent simulated without mutating the process), `GroupGuard` closes S11's early-return leak, `resolve_program` verifies every binary, control tests run real `git`; 18 tests | **T2-1 only (blocker)**; T2-2 downgraded to LOW; T2-3 moved to the choke point |
| `review/task02-r3` | `c2997af` | **FAIL** | T2-1 **fixed** (path keyed on the slug), T2-4 git ref grammar, T2-6 `PR_SET_PDEATHSIG`, T2-7 clone-name cap; 19 tests | **B-1**: builders emitted a relative `program` while the spawn refuses non-absolute → the app could not spawn at all. Fix: resolve at construction |
| `review/task02-merged` | `90db490` | merged | T2-1 fix + `PDEATHSIG` + branch grammar + name cap cherry-picked onto main from `c2997af`; conflicts in `lib.rs` resolved; 36 tests, gate rc 0 | B-1 (resolved at the call site in `fetch_one`, not yet at construction) |
| `main@866d7ff` | `866d7ff` | **FAIL** | S11/S12 PASS: one shared reader deadline (2 s total), `truncated = true` on an incomplete read, detached readers bounded by `Caps`, group kill intact. S13 PASS. S14/R5-1 PASS: rotation on the write that crosses the bound | **B-1**; **S15-1** SIGINT not handled; **S12-1** `Timeout` had no `truncated`; **S14-1** rotation failure swallowed; INFO: U+2028/U+2029/U+FEFF |
| `main@c6f3559` | `c6f3559` | **FAIL** | slug ✓ (probe green); `check_argv` called from `run_pipe_with_caps` before spawn, error not panic, effective on hand-built `Argv`; `is_retryable_spawn_error` + `only_etxtbsy_is_retryable` | **B-1** and **S15-1** unchanged; **T2-3b** literal match let `-f`, `-d`, `--delete-branch`, `--force=true` through |
| `main@e0cbe1d` | `e0cbe1d` | awaiting verdict | **prctl return checked**: a failed `PR_SET_PDEATHSIG` now fails the spawn instead of running without the death-signal; **worktree path de-flattened**: `owner/repo` stays two components, so `a--b/c` and `a/b--c` no longer share a directory (`distinct_slugs_can_never_flatten_into_one_directory`). S11/S12 now say out loud that the timeout bounds the child, not its readers (T2-5). 26 suites green | T2-3c clusters (`-fd`), M-3 |
| `main@625b7e9` | `625b7e9` | **PASS** — supersedes `77a844e`, `5d7cc37`, `a200e21`, `34fc468` (`src/` identical to `e0cbe1d`: `git diff --stat e0cbe1d..main -- src/` empty) | **B-1 closed at product level**: `Binaries::detect()` verifies once, `git()` absolute with `run_pipe` → `Ok`, `fetch_one(&b, "cli/cli", Pr, 2)` → 2 real PRs [14783, 14781]. **Worktree path no longer flattened**: `acme/widget` → `/data/worktrees/acme/widget/main`, `other-org/widget` → `…/other-org/widget/main`, and the colliding pair `a--b/c` vs `a/b--c` is now distinct; the same slug gives the same path (deterministic). `pre_exec` checks `prctl` and fails the spawn; S11 now says SIGKILL, matching the code. S15-1 regression: SIGINT/SIGTERM/SIGHUP → `1049l` + cursor + exit 0. Choke point 6/6 refused, no false positives. Suite 113 tests / 26 binaries, invariants 8/0, controls 16/0, gate ok | **T2-3c** and **M-3** were listed here — **both closed in `e6d3b0d`** (`check_argv` decomposes clusters: `-fd`, `-df`, `-af` refused, `-n` legal; the render test uses the model probe's OSC-52 bytes) |
| `main@b470a86` | `b470a86` | awaiting verdict | **Task 9, action 1**: `o` opens the selected item in the browser. The URL is gated first (https + `github.com`/`www.github.com`, no userinfo, no non-443 port, no control characters or spaces), `xdg-open` is resolved once at startup, the child is detached and inherits no stdio. Tests: the request is only made for a safe URL, an unsafe one never reaches the launcher, the launcher receives exactly one argument (`tests/browser_open.rs`), a relative launcher is refused | worktree action (writes), agent integration |
| `main@d64978f` | `d64978f` | awaiting verdict | **auditor probe vendored** as `tests/audit_task02.rs`, adapted to the current API: slug keys the path, a relative program is refused, and the builders' argv is actually spawnable (`git --version`, `gh --version` through `run_pipe`). 3/3 green — the third assertion is the one whose absence let B-1 merge | T2-3c clusters, M-3 |
| `main@e6d3b0d` | `e6d3b0d` | awaiting verdict | **T2-3c closed**: `check_argv` decomposes short-flag clusters (`-fd`, `-df`, `-af` refused; `-n` legal). **M-3 closed**: the render test now uses the same hostile bytes as the model probe (OSC-52 clipboard write + RLO), asserting the visible text survives and nothing else does | nothing outstanding in this ledger |
| `main@d957556` | `d957556` | **PASS** | **S10 URL gate holds** — 25-URL matrix: refused `http`, `file:`, `javascript:`, `github.com.evil.example`, `github.co`, credentials (`user:pw@`, `***@`), port `8443`, `:0443`, empty port, empty host, uppercase `HTTPS://`, `github.com\@evil`, `evil.example\github.com`, `\n` in the URL, Cyrillic homograph `gіthub.com`, `%2F@`, `[::1]:443`; accepted `https://github.com/…`, `www.github.com`, `:443`, `?query#frag`; `open_args` is a single element, never a shell string. B-1/prctl/slug/S15-1 regressions green. Suite 118 tests / 26 binaries, invariants 8/0, controls 16/0, gate ok. U-1 (separators `Cf`) **closed in `33823d9`** — the gate now refuses U+2028/U+2029/U+FEFF, same list as `sanitize` | T2-3c and M-3 were listed here; **both closed in `e6d3b0d`** |
| `main@648f2e8` | `648f2e8` | **PASS** | **S10 closed end-to-end**: `open_in_browser` checks the launcher is absolute, validates through `open_args`, spawns with stdin/stdout/stderr null; a fake launcher receives exactly one argument, and a hostile URL is refused **without spawning**. `Binaries { gh, git, hermes, xdg_open }` keeps `xdg-open` optional, so `o` degrades instead of crashing. `builders_produce_a_runnable_argv` is on main. Suite 124 tests / 27 binaries, invariants 8/0, controls 16/0, gate ok, S15-1 ok. **B-2 (zombie per opening) closed in `003d47f`** — the child is reaped by a thread (`child.wait()`), not with a process-wide `SIGCHLD=SIG_IGN` that would break `try_wait` in `run_pipe`; test `the_launcher_is_reaped` polls `/proc/<pid>`. **U-1 closed in `33823d9`** | T2-3c and M-3 were listed here; **both closed in `e6d3b0d`** |
| `main@66ee08b` | `66ee08b` | awaiting verdict | **process**: `scripts/review-state.sh` prints the remote tip and the newest ledger row, so a review starts from the tree that exists instead of a remembered SHA. No code change | nothing outstanding |
| `main@11811b3` | `11811b3` | awaiting verdict | `scripts/review-state.sh` now counts the commits between the newest ledger row and the remote tip; a reviewer runs it first and knows whether the SHA in hand is still the tree | nothing outstanding |
| `main@81b14fe` | `81b14fe` | awaiting verdict | `review-state.sh` excludes docs-only commits when counting unreviewed code, so the ledger and the tip can finally agree | nothing outstanding |
| `main@a8c7d83` | `a8c7d83` | awaiting verdict | **r21 LOW closed**: `read_private` opens with `O_NOFOLLOW`, so a symlinked config fails at `open` instead of between `symlink_metadata` and `read_to_string` (unit tests: symlinked file refused, regular file read). **r21 INFO closed**: `Span::plain` sanitises its text, so the public constructor cannot bypass S13. Audited-good from r21: fetch args, `OutputCap`, markdown spans, atomic private writes | nothing outstanding |

## Standing rules this ledger encodes

1. **Merge is the last step.** The merge message carries `reviewed-sha: <tag sha>` and
   `key-file-sha256: <hash of the file the verdict cites>`.
2. **Hash the pushed tree**: `git show <ref>:<path> | sha256sum`, never the worktree before the push.
3. **A control must fail without the fix.** Every security test has a paired control that demonstrates
   the threat on the unwrapped path (`tests/env.rs` `*_control`, harness `controls.sh`, the `--version`
   trap in the logging tests).
4. **Skip loudly.** An assertion whose subject does not exist yet is `SKIP ... needs task/NN`, never a
   silent pass.
5. **Fix-forward, not history rewrite.** A post-merge finding becomes a small commit plus a new row
   here; a `FAIL` row stays in the ledger.

## Carried debt (not blocking any tag)

Nothing security-relevant is outstanding. The next functional block is Task 9 part 2 (worktree for a
PR) and Task 11 (agent), neither of which has been written yet.


## Closed since this ledger started

- **S-1** `create_in` calls `ensure_runtime_dir` (`4044d2e`).
- **T2-1** path keyed on the slug (`c2997af`).
- **P2** init guard taken before any fs work (`3584d3c`).
- **T2-5** reader join deadline (`866d7ff`).
- **R5-1** in-session log rotation (`b990b65`).
- **S15-1 / S12-1 / S14-1 / T2-3b / U+2028-29-FEFF** (`77a844e`).
- **B-1 / T2-2** binaries resolved once at startup and injected into the builders (`5d7cc37`).
- **prctl return** and **slug flattening** (`e0cbe1d`).
- **T2-3c** short-flag clusters and **M-3** hostile render bytes (`e6d3b0d`).
