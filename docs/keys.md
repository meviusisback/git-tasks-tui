# Keys (what the code does today)

| Key | Action |
|---|---|
| `q` / `Esc` | quit |
| `a` | ask to act on the selected PR: opens a confirmation showing the exact command |
| `y` | in the confirmation: run the command |
| `n` / `Esc` | in the confirmation: cancel — nothing runs |
| `Enter` | open the detail of the selected item |
| `Tab` | next pane: list → **repos** → detail |
| `↑` / `↓` | move the selection; in the repos pane it **picks a repo** (row 0 = all repos) |
| `/` | search (type to filter, `Enter` leaves the box, `Esc` closes it) |
| `o` | open the selected item in the browser |
| `r` | refresh |

`a` on an **issue** does nothing and says why: issues are not merged. The merge
method is a constant (`FIRST_ACTION_METHOD`, today `squash`) and `--delete-branch`
is always off, so neither is reachable from the keyboard yet. `?`, `f`, `w`, `c`,
`m`, `x`, `s`, `p` are planned, not implemented.
