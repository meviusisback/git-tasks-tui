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

## Worktrees

- Menu lists the worktrees that already exist for this repo (from `git worktree
  list`) and offers "create for this item", which uses `<clone_root>/<owner>/<repo>`
  and a branch name derived from the item number.
- Paths are built from typed components; traversal, absolute input, symlinks are
  refused. A missing clone is a message, not a crash.

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
