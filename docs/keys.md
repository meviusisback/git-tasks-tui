# Keys (what the code does today)

Every line below is taken from the key handler in `src/app.rs`; if the two ever
disagree, the handler is right and this file is a bug.

| Key | Action |
|---|---|
| `q` | quit |
| `Ctrl-C` | quit — the escape hatch, always live, except with the agent focused, where it interrupts the agent |
| `Esc` | back **one level**: overlay → confirmation → search → detail → list → repos. It **never quits** and **never steals agent keys**: with the agent focused `Esc` goes to the agent (like `Tab`; `Shift`+arrows leave the box). At the outermost level it says `Esc — nothing to close` in the status line |
| `?` | the overlay of keys (it says which work today and which are promised) |
| `←` / `→` | move between the columns: left (repos, list) ↔ right (detail); `Tab` cycles the panes, plus the agent box while the active tab's agent operates. Mouse: left-click focuses a box or opens a tab, drag on a divider resizes the cross (saved across restarts), the wheel scrolls the agent box history |
| `Tab` | next pane: list → repos → detail → list, plus the agent box while the active tab's agent operates |
| `Shift` + `←`/`↑`/`↓`/`→` | move focus between the four boxes following the screen layout (left column repos/list, right column detail/agent) |
| `↑` / `↓` | move the selection in the focused pane; in the repos pane it moves between repos and the list below follows; in detail/agent it scrolls |
| `Enter` | in the repos pane: drill into that repo's items; in the list: bookmark the row — open its tab, show it and move the focus right (the open tab owns the detail; without tabs the detail follows the list); in the search box: leave the box |
| `t` / `T` | next / previous tab — the open tab owns the detail (its issue shows even across a repo filter, even closed) and its agent comes back below; scrolling never drags the detail off a tab. Never touches the repo selection on the left |
| `x` / `X` | close the active tab first, then the agent record |
| `w` | focus the agent pane (worktree + agent instructions live there) |
| `R` | repo selection menu (the rows re-read from GitHub on open, so new repos join without a restart): `Space` marks, `a` marks all, `n` unmarks all, `↑`/`↓` moves, `Enter` applies, `Esc` cancels. Picks are saved to `state.json` (0600); nothing marked means **all repos** |
| `/` | search: filters the visible list on the title and the number (`/42` finds `#42`) |
| `f` | state filter on a single repo: open → closed → all states (numeric, oldest first). Closed history is fetched on demand; on "all repos" it says to pick one repo first |
| `d` | diff of the selected PR in an overlay (`↑`/`↓` scroll, wheel scrolls, `Enter`/`Esc` closes): fetched once, then cached; first 200 lines with the total named when truncated |
| `o` | open the selected item in the browser |
| `r` | refresh — it works in every pane, including inside a drilled-in repo. The list also re-reads itself every `refresh_s` seconds while idle (default 300, `0` turns it off) |
| `a` | ask to act on the selected PR: opens a confirmation showing the exact command |
| `y` | in the confirmation: run the command |
| `n` / `Esc` | in the confirmation: cancel — nothing runs |
| `m` | actions menu on the list only (elsewhere it refuses): method and branch deletion are a choice, then the same confirmation as `a`. After a merge lands, one question offers to delete its local leftovers (worktree + head branch) |
| `g` | in the repos pane: the repo's sessions — every agent and past run on its worktrees, open or closed, in one menu; `Enter` reopens (the recorded session first, nothing sent) or picks an agent for a fresh run; no worktrees yet names a fresh tree. Otherwise: choose the agent + worktree for the selected PR or issue: with a single previous session behind it, `g` offers that first (`y` = resume, nothing sent · `n` = choose); otherwise first the agent menu (hermes, pi, omp, opencode, agy — only installed ones), then the worktree — use an existing tree, `n` name a new parallel one (head branch suggested, `issue-N` for an issue), `r` rename, `d` delete (asks first, the branch stays). `Enter` on a tree where that agent ran before offers its previous session back first (`y` = resume, nothing sent · `n` = keep choosing). No local clone: offers to clone it first, then continues here |
| `W` | the worktree menu directly, without the agent flow: the same list/use/new/rename/delete for the selected PR; picking a tree for work leads to the agent menu with that tree preset. With the repos pane focused, `W` grows a tree instead: branch suggested (`<repo>-dev`, editable), agent menu follows, confirming asks the creation question and launches. `W` also works on an empty list filtered to one repo. No local clone is not a dead end: the app offers to `gh repo clone` it into the first clone root, then continues. `Enter` on a repo with zero issues/PRs offers to start a new branch the same way |
| `s` | settings menu: `↑`/`↓` move, `Enter`/`Space` flips, `Esc` closes — sandboxed agents, terminal font size (Small/Medium/Large), terminal font. Saved to `config.toml`; the font rows also rewrite the Foot `font=` line (restart the terminal to apply) |
| click on a tab | opens it (same strip the keyboard uses; separators switch nothing, clicks while a dialog is open are ignored); double-click on a list or repos row opens it like `Enter`; the `✕` at the end of a tab closes it like `x` |

`a` on an **issue** does nothing and says why: issues are not merged. `a` always uses
the first action (`FIRST_ACTION_METHOD`, today `squash`) without `--delete-branch`;
the full choice of method × delete-branch lives in `m`.

`g` starts the agent **inside the agent box** (bottom-right, about 60% of the
right column): the program never closes, the agent streams into the pane.
Before anything runs, `g` asks two things: which agent (hermes, pi, omp,
opencode, agy) and on which worktree. A worktree marker (`◉` without a Nerd Font)
in front of a list row means a worktree directory already exists for that
branch — `g` then works on it instead of creating one. While the box has
focus every key but `Shift`+arrows is agent input — typing,
arrows, `Tab` (completion), `Esc` (vim mode, popups), `Ctrl-C` (interrupt).
`Shift`+arrows move between boxes and out of the agent box, the agent keeps running either way. The
mouse wheel scrolls the box back through history (fullscreen agents like Pi
draw on the box's main screen — their alternate screen is folded away, so
the history is there for every agent); typing jumps back to the live tail.
Quitting kills the agent. A finished run leaves its record card in
the detail column.

`p`, `f`, `c` are names that were discussed and
**are not promised anywhere on screen**.

A fresh start (no `state.json` yet) focuses the repos pane: the first
keypress picks a repo, `Enter` drills into its issues, `Enter` again opens
a tab. A restart resumes where you left off — pane, filter, tabs included.

The list is cached in `items-cache.json` next to `state.json`: startup
paints the cache instantly, then a background fetch checks for changes
("up to date" vs a replaced list).

Icons appear only when the host has a Nerd Font (font directories are
scanned once); `GTASKS_NERD=1` forces them on, `GTASKS_NERD=0` off.

A key that is promised in the overlay and does nothing is a lie on screen;
the test `coming_next_keys_produce_no_screen_change` fails when one of them starts
working, so the overlay shrinks at the same moment the stage lands.
