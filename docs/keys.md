# Keys (what the code does today)

Every line below is taken from the key handler in `src/app.rs`; if the two ever
disagree, the handler is right and this file is a bug.

| Key | Action |
|---|---|
| `q` | quit |
| `Ctrl-C` | quit — the escape hatch, always live, in every mode |
| `Esc` | back **one level**: overlay → confirmation → search → detail → repos → list. It **never quits**: at the outermost level it says `Esc — nothing to close` in the status line |
| `?` | the overlay of keys (it says which work today and which are promised) |
| `←` / `→` | move between the panes: repos ↔ list ↔ detail |
| `Tab` | next pane (the same cycle, for people who expect it) |
| `↑` / `↓` | move the selection in the focused pane; in the repos pane it moves between repos |
| `Enter` | in the repos pane: drill into that repo's items; in the list: open the detail; in the search box: leave the box |
| `R` | repo selection menu: `Space` marks, `↑`/`↓` moves, `Enter` applies, `Esc` cancels. Picks are saved to `state.json` (0600); nothing marked means **all repos** |
| `/` | search: filters the visible list on the title and the number (`/42` finds `#42`) |
| `o` | open the selected item in the browser |
| `r` | refresh — it works in every pane, including inside a drilled-in repo |
| `a` | ask to act on the selected PR: opens a confirmation showing the exact command |
| `y` | in the confirmation: run the command |
| `n` / `Esc` | in the confirmation: cancel — nothing runs |

`a` on an **issue** does nothing and says why: issues are not merged. The merge
method is a constant (`FIRST_ACTION_METHOD`, today `squash`) and `--delete-branch`
is always off, so neither is reachable from the keyboard yet.

`m`, `w`, `s`, `p`, `f`, `c`, `x` are **planned, not implemented** — and the
overlay says so, in its own section. A key that is promised in the overlay and
does nothing is a lie on screen; a test (`coming_next_keys_produce_no_screen_change`)
fails when one of them starts working, so the overlay shrinks at the same moment.
