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
| `main@77a844e` | `77a844e` | awaiting verdict | S15-1 **closed**: `SIGINT` handled — pty probe `kill -INT` → `RC=0`, `1049l` present, no leftover process. T2-3b **closed**: matching normalises on `=`, short forms and `--delete-branch` refused (`tests/choke_point.rs`, 9 spellings). S14-1 **closed**: rotation failure logs a warning. S12-1 **closed**: `Timeout` carries `truncated`. INFO closed: U+2028/U+2029/U+FEFF stripped (`tests/sanitize.rs`) | **B-1** (builders resolve at construction, `Binaries` at startup → Task 2b/10b); short-flag clusters (`-fd`) still not decomposed; Task 8 detail wired (`3cc153d`) |

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

- **B-1** `gh()`/`git()` must return `Result<Argv>` with the program resolved at construction (a
  `Binaries { gh, git, hermes }` value built at startup, injected into the builders and into tests).
  Today `fetch_one`/`fetch_detail` resolve at the call site, which works but leaves the trap for the
  next caller. Task 2b/10b.
- **T2-3c** short-flag clusters (`-fd`) are not decomposed; the long-term answer is an allowlist of the
  flags the app actually uses.
- **T2-2** startup resolution — same change as B-1.
- **`slug.replace('/', "--")` is not injective** (`a--b/c` and `a/b--c` collide); a length-prefixed or
  hashed component removes the ambiguity.
- **`pre_exec`** ignores `prctl`'s return value.
- **M-3** the render test should reuse the same hostile bytes as the model probe.

## Closed since this ledger started

- **S-1** `create_in` calls `ensure_runtime_dir` (`4044d2e`).
- **T2-1** path keyed on the slug (`c2997af`).
- **P2** init guard taken before any fs work (`3584d3c`).
- **T2-5** reader join deadline (`866d7ff`).
- **R5-1** in-session log rotation (`b990b65`).
- **S15-1 / S12-1 / S14-1 / T2-3b / U+2028-29-FEFF** (`77a844e`).
