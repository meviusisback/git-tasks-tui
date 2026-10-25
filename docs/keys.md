# Keys (what the code does today)

Every line below is taken from the key handler in `src/app.rs`; if the two ever
disagree, the handler is right and this file is a bug.

| Key | Action |
|---|---|
| `q` | quit |
| `Ctrl-C` | quit — the escape hatch, always live, in every mode |
| `Esc` | back **one level**: overlay → confirmation → search → agent → detail → list → repos. It **never quits**: at the outermost level it says `Esc — nothing to close` in the status line |
| `?` | the overlay of keys (it says which work today and which are promised) |
| `←` / `→` | move between the columns: left (repos, list) ↔ right (detail); `Tab` cycles all four panes. The mouse is not supported — keyboard only |
| `Tab` | next pane: list → repos → detail → agent → list (for people who expect it) |
| `↑` / `↓` | move the selection in the focused pane; in the repos pane it moves between repos and the list below follows; in detail/agent it scrolls |
| `Enter` | in the repos pane: drill into that repo's items; in the list: open a tab for the row (the detail follows the active tab); in the search box: leave the box |
| `t` / `T` | next / previous tab (says `no tabs — Enter opens one` when empty) |
| `x` / `X` | close the active tab first, then the agent record |
| `w` | focus the agent pane (worktree + agent instructions live there) |
| `R` | repo selection menu: `Space` marks, `a` marks all, `n` unmarks all, `↑`/`↓` moves, `Enter` applies, `Esc` cancels. Picks are saved to `state.json` (0600); nothing marked means **all repos** |
| `/` | search: filters the visible list on the title and the number (`/42` finds `#42`) |
| `o` | open the selected item in the browser |
| `r` | refresh — it works in every pane, including inside a drilled-in repo |
| `a` | ask to act on the selected PR: opens a confirmation showing the exact command |
| `y` | in the confirmation: run the command |
| `n` / `Esc` | in the confirmation: cancel — nothing runs |

`a` on an **issue** does nothing and says why: issues are not merged. The merge
method is a constant (`FIRST_ACTION_METHOD`, today `squash`) and `--delete-branch`
is always off, so neither is reachable from the keyboard yet.

`m`, `s` are **planned, not implemented** — and the overlay announces
exactly these two, in its own section ("coming in the next stages"), with the
stage that will bring each one. `w` used to be promised here; it now focuses
the agent pane, so it moved above. `p`, `f`, `c` are names that were discussed
and **are not promised anywhere on screen**: this document used to claim the
overlay covered seven keys when it covered three, and a document that speaks for
the screen has to be checked against it (K-1, 2026-10-25).

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
