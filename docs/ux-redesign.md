# gtasks — the interface Alberto asked for

Owner: Alberto. Coordinator: @hermes. Implementation: @coder. Review: @auditor.
This file is the contract; if the code and this file disagree, the file is the bug.

## The shape of the screen

```
┌ gtasks ────────────────────────────────────────────────────────────────┐
│ ┌ list ─────────────────┐ ┌ detail / agent ────────────────────────┐   │
│ │ ▸ me/botty     (3)    │ │  #16  news: stale model data report    │   │
│ │ ▸ me/git-tasks (1)    │ │  state     open      author  alberto   │   │
│ │                       │ │  labels    bug       updated 2h ago    │   │
│ │   #16 news: stale…    │ │  head      fix/…     checks  ✓ 2/2     │   │
│ │   #12 fix: …          │ │                                        │   │
│ │                       │ │  [m] merge  [w] worktree  [a] agent    │   │
│ └───────────────────────┘ └────────────────────────────────────────┘   │
│ ←/→ panes · ↑/↓ move · Enter open · R repos · m actions · ? help · q   │
└────────────────────────────────────────────────────────────────────────┘
```

- **Left column** is the only navigation surface. It shows **repos** by default, and
  switches to the **items of the selected repo** once one is chosen ("drill-down").
  `←` goes back to the repo list.
- **Right column** shows the **detail box** for the selected item, or the **agent**
  once started. The agent takes ~70% of the width (`Esc` returns to the detail).
- **Bottom line is a legend**: the keys available *right now*, not the full keymap.
  `?` opens the full keymap as an overlay.

## Keys (no backtick, no chord required)

| Key | Where | What |
|---|---|---|
| `←` / `→` | always | move between the left column and the right column |
| `↑` / `↓` | left column | move the selection; in repo mode it selects a repo |
| `Enter` | left column | drilled down: open the detail box · in repo mode: drill into the repo |
| `Esc` | right / open menu | back one level: agent → detail → item list → repo list |
| `R` | left column | **repo selection menu**: multi-select with `Space`, apply with `Enter`; only checked repos appear in the list |
| `m` | detail box | **actions menu**: merge (squash / merge / rebase), delete branch, comment, close/reopen |
| `w` | detail box | **worktree menu**: create for this item, or switch to an existing one |
| `a` | detail box | **start the agent** on this item; the agent opens in the right column |
| `s` | agent pane | **session menu**: continue an existing session or start a new one |
| `o` | any list/detail | open in the browser |
| `/` | left column | filter the visible list |
| `r` | always | refresh |
| `?` | always | full keymap overlay |
| `q` | always | **the normal exit** (with a confirmation if the agent is running) |
| `Ctrl-C` | always | **the emergency exit** — unconditional, works even when the UI is stuck |

Every destructive action keeps the existing rule: a confirmation that shows the
**exact argv**, `y` runs it, `n`/`Esc` cancels.

**`Esc` is never an exit.** It goes up one level and stops at the first. The
property is "no `Esc`, in no state, sets `quit`" — not "Esc at the top level",
because the states multiply (`pane`, overlay, search) and the special case is
what rots. The test sweeps every combination, and an exhaustive `match` over
`Pane` makes a new pane a compile error, so whoever adds one is forced to look at
the sweep. `q` quits, `Ctrl-C` always quits, `Esc` does not.

This is not cosmetic: from stage 16 an agent may be running with
`PR_SET_PDEATHSIG`, so an extra `Esc` would kill the agent — one keystroke
deleting the work you were watching.

## Detail box: the fields the plugin shows

state · author · labels · assignees · created/updated (relative) · head branch ·
base branch · checks (passed/failed/pending, with the failing names) · mergeable
state · review decision · linked issue · url. Empty fields are **omitted**, not
shown blank — a blank field reads as "none" and that is a claim.

## First screen: under ten seconds, measured

Measured on Alberto's machine with 69 configured repos (2026-10-25):

| what | time |
|---|---|
| one REST call pair per repo (`pr list` + `issue list`) | 6.16 s |
| one GraphQL call covering **both** for one repo | 3.17 s |
| one GraphQL call covering **8** repos | 2.36 s |
| one GraphQL call covering **20** repos | 4.23 s |

So the current shape (two REST calls per repo, `parallel = 6`) needs about
`69 × 6.16 / 6 ≈ 71 s` before anything appears — and that is what a live run
shows: "loading…" at 40 s, counters still zero at 90 s. Batching 20 repos per
GraphQL call turns 138 calls into 4: about 17 s serial, **~5 s** with the same
parallelism.

The requirement is the measurement: **the list is populated within ten seconds
on 69 repos**. A slower first screen is a bug, not a tuning preference — the
program's whole value is answering "what needs me" at a glance.

## Repo selection

`repos` in `config.toml` stays the hand-edited list of candidates. The **user's
picks** live in `state.json` (0600, atomic replace, next to the config): they are
UI state that changes with a keystroke, not a file a person edits. Empty state
means "all of `repos`". The menu toggles membership and writes that file; the
items list only ever shows the picked repos.

**A new list resets the cursor, so no row survives a reorder.** `set_repos` sets
`repos_cursor = 0` and clears the detail; that single reset is what makes
"remember the repo you came from" unnecessary — the identity problem cannot
arise if no old row outlives the list. This is a rule, not a side effect: any
change that keeps the cursor across a new list reintroduces the stale-row bug
silently. The test that pins it (`applying_a_selection_resets_the_repo_cursor`)
is owed; the comment next to `set_repos` must say why.

## The agent

- Starts from the detail box, for one item. Args are **fixed and typed**; the
  item's title/body are never argv, never a shell word.
- The prompt (if any) goes into a `0600` temp file; the agent reads it with
  `--query-file`. Untrusted text is marked as untrusted in the UI and in the file.
- Runs in its **own process group**; timeout kills the group; no `SIGCHLD` tricks.
- Output streams into the right column. `Esc` leaves the pane but does **not** kill
  the agent; `q` asks first.
- A session menu lists existing sessions (`--continue <name>`) and offers "new".
- While the agent runs, the left column stays usable: choosing another item and
  pressing `a` asks whether to start a second agent or switch back.

### The prompt cannot exist as text the argv builder can see

Not "we agreed not to put it in argv" — the type system has to make it
unrepresentable (measured constraint from @auditor, same trick as `RepoSlug`):

```rust
/// A prompt already written to a private file. The text is consumed by the
/// writer and not kept: only the path survives. No constructor from &str.
pub struct PromptFile(PathBuf);            // private field
impl PromptFile {
    pub fn write(text: &str) -> io::Result<Self>;  // 0600, create_new, $XDG_RUNTIME_DIR
    pub fn path(&self) -> &Path;
}
pub struct AgentInvocation { /* private: prompt: Option<PromptFile>, cwd: Dir,
                                session: Option<SessionName>, timeout */ }
impl AgentInvocation { pub fn argv(&self) -> CheckedArgv; }   // "--query-file" <path>, never "-"
```

The proof that this holds is a probe, not a promise: a fake `hermes` records its
argv, the prompt carries a unique marker, and the test asserts the marker is in
the file, is in **no** argv, and that argv carries `--query-file` followed by a
path that is not `-`.

### The agent's output is untrusted text

It carries the item title/body it just read plus whatever ANSI the model emits.
It goes through the existing `SanitisingWriter` before it reaches the screen,
exactly like `gh` output — otherwise the agent pane is a second injection channel
that bypasses U-6.

### The process belongs to the session, not to the pane

`Esc` leaves the pane without killing the child, so the child cannot live in the
view's state: a `Session { child, group, started }` sits in a store outside the
UI, and the pane only draws. Reuse the group handling that already exists for
`gh` (`kill(-pid)`); do not write a second one — the "a single `Command::new`"
rule applies here too.

## Worktrees

- Menu lists the worktrees that already exist for this repo (from `git worktree
  list`) and offers "create for this item", which uses `<clone_root>/<owner>/<repo>`
  and a branch name derived from the item number.
- Paths are built from typed components; traversal, absolute input, symlinks are
  refused. A missing clone is a message, not a crash.

### Symlinks defeat component validation — the filesystem resolves them afterwards

Measured: a component that is a symlink to `/outside` (or an ancestor that is)
makes `git worktree add` succeed **outside** the root, with a perfectly clean
name, because the string checks run before the filesystem resolves anything.
The sequence that closes it:

```rust
let parent = fs::canonicalize(&worktree_root)?;
if !parent.starts_with(fs::canonicalize(&clone_root)?) { refuse("outside the root") }
let target = parent.join(format!("item-{number}"));   // components already typed
fs::create_dir(&target)?;        // WE create it: fails on EEXIST, including a symlink
Command::new("git").args(["-C", clone, "worktree", "add", "-b", &branch, target])?;
let real = fs::canonicalize(&target)?;                // re-check after (TOCTOU)
if !real.starts_with(&parent) { fs::remove_dir_all(&target); refuse }
```

Git accepts an existing empty directory (measured), so there is no reason to let
git create it — and `create_dir` refuses a symlink instead of following it.

### Two branches must never share a directory (W-1, measured)

`branch.replace('/', "-")` is not injective: `feat/x` and `feat-x` both become
`feat-x`, so "open the worktree of branch X" can show the tree of Y. The mapping
has to escape the escape character first:

```rust
let flat_branch = branch.replace('-', "--").replace('/', "-");
// feat/x -> feat-x   ·   feat-x -> feat--x   ·   item/42 -> item-42
```

And after `git worktree add`, ask git which branch that tree is — a path does not
identify a tree, git does:

```rust
git -C <path> rev-parse --abbrev-ref HEAD   // must equal the requested branch, or undo
```

### Take types, not `&str` (W-2, measured)

`resolve_in(base, clone, slug: &str, branch: &str)` accepts `"."` and produces
`/base/./item-1`: nothing escapes the root, but a component silently removes a
level of `owner/repo`. `RepoSlug` already exists and was built for this; the
signature should be `(&RepoSlug, &BranchName)`, so `"."` is unrepresentable
rather than rejected by a check. This is the only place in the project where the
validation is duplicated by hand instead of delegated to the type.

**The type change is enforced by the compiler, not by a review** — a doctest with
the error code pinned, measured red today and green against a typed signature:

```rust
/// ```compile_fail,E0308
/// let _ = resolve_in(Path::new("/base"), Path::new("/clones/o/r"), ".", "x");
/// ```
```

`compile_fail` alone would pass on *any* error, including a typo in the example,
so the code is pinned: the failure must be "incompatible types". It also needs a
positive twin — `RepoSlug::parse("o/r")` must still compile — otherwise a function
that accepts nothing would satisfy the negative test.

`BranchName` does not exist yet: built like `RepoSlug` (one door, private field,
`parse`), it takes over the 13 traversal checks that live inside `resolve_in`
today, leaving it a pure path mapper.

The gate must run `cargo test --doc` (or plain `cargo test`): `cargo test --tests`
skips doctests and `cargo nextest` never runs them — a control that disappears in
silence without ever producing a red.

Two more rules:

- **Pass `-b` with a name built from a typed value** (`item/<n>`). Without `-b`,
  git derives the branch from the path's basename — the filesystem would be
  naming branches.
- **Never `--force`.** Measured: a registered path is refused anyway, so the flag
  buys nothing and removes a defence.

`git worktree add` also runs the clone's `post-checkout` hook (measured: it fires,
and it does not with `-c core.hooksPath=<nonexistent>`), so the S16 hardening used
for `gh` applies here too.

## Staging (each stage is a branch, a tag, a verdict, then a merge)

| # | Branch | Deliverable | Done when |
|---|---|---|---|
| 1 | `task/10-shell` | layout, panes, `←/→`, legend line, `?` overlay, theme module | a live run shows the layout above; resize does not break it |
| 2 | `task/11-drilldown` | repo → items, `Esc` back, filter kept on the way out | drill in/out with arrows (`Tab` until the shell lands) |
| 3 | `task/12-repo-menu` | `selected_repos` in config, `R` multi-select menu, list filtered | toggling a repo changes the list after restart |
| 4 | `task/13-detail` | structured detail box with the fields above | every field is either true or absent |

| 5 | `task/14-actions` | `m` actions menu on `CheckedArgv` (merge method, delete branch, comment, close) | the confirmation still shows the exact argv |
| 6 | `task/15-worktree` | `w` menu, create/switch, path safety | traversal tests green |
| 7 | `task/16-agent` | right-column agent, process group, session menu, 70% width | agent survives a timeout and the UI resumes |
| 8 | `task/17-polish` | Catppuccin Mocha, per-section colors, final legend | a screenshot pass with no unreadable pair |

**What stages 1–3 taught us, and what stage 4 inherits.** Each of these cost a
round trip; they are written here so the next stage does not pay again.

- **The width that decides is 80 columns.** A test rendered at 120 certifies a
  screen that was never broken. Any test that asserts "this text is visible"
  renders at 80×24; 120×30 is a second case, never the one that decides.
- **A test that watches the thing you are improving cannot see the thing you are
  breaking.** Every asserted screen part names the part it watches; the status
  line has its own test because it disappears on its own.
- **"Not refetched" and "the cursor does not move" are different claims.** `r`
  refreshes in every mode, so any list position kept as an index is a bug waiting
  for a refresh: keep the object, or keep nothing.
- **One value, one meaning.** Empty `selected_repos` means *all*; so a menu where
  nothing is marked must also mean *all*, said in the status line at apply time.
  A state that means one thing now and another after a restart is the same class
  of defect as a path that believes it identifies a tree.
- **Carried debt (`RepoRow`).** The repo pane builds `RepoFilter` from a string
  row and a sentinel (`"— all repos —"`) through `RepoSlug::parse(..).expect(..)`.
  Unreachable today only because `Config::load` validates first. Stage 4 touches
  `app.rs` anyway: make the row `enum RepoRow { All, Repo(RepoSlug) }` **before**
  any list arrives from `gh` rather than from the config, or the `expect` fires.
  A field that must be true is a type, not an `expect`.
- **Fields come from data that was actually fetched.** A field with no source is
  omitted, never defaulted: `checks: none` and `checks: never asked` are
  different statements, and only one of them is honest.
- **A rejection test needs a positive hinge.** If a test can be green while the
  function does not exist, it is not measuring the function — a stub that refuses
  everything satisfies "it must fail" and "it wrote nothing outside". Every
  refusal test therefore carries, in the same fixture, a clean operation that
  must succeed: that is what tells "refused for the right reason" from "refused
  because nothing is implemented". (Measured: a five-test set went 3 green / 2
  red with the keys *backwards*, and the two green ones were the adversarial
  cases.)
- **A status line is computed from what the screen draws, never narrated
  beside it.** `selection saved (0 repos)` was true of the file and false of the
  list: two sentences about one fact, written in two places, free to disagree.
  The form that cannot lie derives the sentence from the same collection the
  panel renders (`"{} repos · {} items"`): if the list is empty the count is 0,
  and there is nowhere to write "showing all" while showing something else.
- **The action may be faked; the observation may not.** A test that makes a
  fake `git` refuse `worktree add` is legitimate — it proves the rollback holds
  whatever the reason. But the registry that the rollback is supposed to leave
  empty must then be read with the **real** `git`: a fake cannot testify about
  what it did not do, and a fake grading its own homework is the same vacuity as
  a scan that matches nothing.
- **A config key that is silently clamped is a lie.** `config.toml` says
  `parallel = 6`; `fetch_many_with` does `parallel.clamp(1, 4)`, and a test pins
  the clamp. The file therefore states something the program cannot do, and the
  person tuning it has no way to learn that. Either honour the value, or refuse
  it at load with a message — never accept it and ignore it.
- **A probe is not a test.** Non-deterministic checks (the TOCTOU window between
  `create_dir` and `git worktree add`) are run by hand and written down as
  probes; they never enter the gate, because a flaky green is worse than an
  absent check.

Theme: Catppuccin Mocha tokens in one module (`src/theme.rs`), no colour literals
elsewhere; the legend and the status line share the same palette.

## Owed, small, and easy to forget

Each of these was found by measurement and is a one-or-two-line change; none of
them is a feature. They are listed here because a chat message is not a place
where work survives.

1. **The `repos` half of the cursor clamp is inert.** `set_lengths` is called
   three times, always with `self.repos.len()`, so `repos_cursor.min(len - 1)`
   cannot lower anything, and `apply_repo_filter` already handles an
   out-of-range cursor (`_ => RepoFilter::All`). Delete that half; keep the
   `list` half, which really does shorten. Dead code that reads as a guard is
   worse than no guard: the next reader believes the reorder is defended.
2. **`last_repo` must not come back.** It has been added and removed twice. The
   fact that makes it unnecessary — *every* writer of the repo list resets the
   cursor — belongs in a comment on `set_repos`, not on the clamp.
3. **The estate of `RepoFilter`.** It was not removed: it is derived from
   `RepoRow` in exactly one place. Two names for two things (the row, the active
   filter) is the good outcome; a third representation would not be.

## Non-negotiables

1. Untrusted text is displayed with escape sequences sanitised and never reaches
   argv or a shell.
2. Actions still go through `CheckedArgv`; no new path to spawn. **The spawn
   sites are a declared list** (`tests/invariants/`), not a judgement call: each
   entry carries the reason it exists, an undeclared site fails, and a declared
   site that no longer contains `Command::new` fails too (a stale exemption must
   die with the thing that justified it). The detector also fails when it finds
   *nothing*, because a pattern that matches nothing describes a broken pattern,
   not a healthy repo.
3. The agent and the worktrees do not weaken any existing test: the suite stays
   green, clippy `--all-targets -D warnings` stays at 0.
4. Every stage is reviewed on a tagged tree before merge (`pre-push` enforces it).
