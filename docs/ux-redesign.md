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
| `q` | always | quit (with a confirmation if the agent is running) |

Every destructive action keeps the existing rule: a confirmation that shows the
**exact argv**, `y` runs it, `n`/`Esc` cancels.

## Detail box: the fields the plugin shows

state · author · labels · assignees · created/updated (relative) · head branch ·
base branch · checks (passed/failed/pending, with the failing names) · mergeable
state · review decision · linked issue · url. Empty fields are **omitted**, not
shown blank — a blank field reads as "none" and that is a claim.

## Repo selection

`selected_repos` is a new config key: a list of `owner/name`. `repos` stays the
full list; `selected_repos` defaults to all of it. The menu toggles membership and
writes the config back (0600, atomic replace). The items list only ever shows the
selected repos.

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
| 2 | `task/11-repo-menu` | `selected_repos` in config, `R` multi-select menu, list filtered | toggling a repo changes the list after restart |
| 3 | `task/12-drilldown` | repo → items, `Esc` back, items keep the repo filter | drill in/out with arrows only |
| 4 | `task/13-detail` | structured detail box with the fields above | every field is either true or absent |
| 5 | `task/14-actions` | `m` actions menu on `CheckedArgv` (merge method, delete branch, comment, close) | the confirmation still shows the exact argv |
| 6 | `task/15-worktree` | `w` menu, create/switch, path safety | traversal tests green |
| 7 | `task/16-agent` | right-column agent, process group, session menu, 70% width | agent survives a timeout and the UI resumes |
| 8 | `task/17-polish` | Catppuccin Mocha, per-section colors, final legend | a screenshot pass with no unreadable pair |

Theme: Catppuccin Mocha tokens in one module (`src/theme.rs`), no colour literals
elsewhere; the legend and the status line share the same palette.

## Non-negotiables

1. Untrusted text is displayed with escape sequences sanitised and never reaches
   argv or a shell.
2. Actions still go through `CheckedArgv`; no new path to spawn.
3. The agent and the worktrees do not weaken any existing test: the suite stays
   green, clippy `--all-targets -D warnings` stays at 0.
4. Every stage is reviewed on a tagged tree before merge (`pre-push` enforces it).
