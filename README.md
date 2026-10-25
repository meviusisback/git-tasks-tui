# git-tasks-tui

A terminal counterpart to the [git-tasks](https://github.com/meviusisback/git-tasks) Hermes Desktop plugin:
multi-repo GitHub issues and pull requests, detail view, review actions, git worktrees and agentic
work — in a single Rust binary.

**Status: MVP in development.** The implementation plan is in [`docs/PLAN.md`](docs/PLAN.md);
the security invariants are in [`docs/security.md`](docs/security.md).

## What it does (MVP)

- Pick repositories to follow (via `gh repo list` or by typing `owner/repo`).
- One list across repos: open/all/closed, PRs/issues, search, group by repo.
- Expand a PR/issue: body, comments, labels, checks, review decision, mergeability.
- Act without leaving the terminal: comment, approve, close/reopen, merge (merge/squash/rebase).
- Create a git worktree for an item and start a Hermes session in it, with the issue/PR in context.

## Requirements

- Linux
- [`gh`](https://cli.github.com/) installed and authenticated (`gh auth login`)
- [Hermes Agent](https://hermes-agent.nousresearch.com/) (`hermes`) for agentic work
- Rust toolchain (`rustup default stable`)

## Build and run

```sh
cargo build --release
./target/release/gtasks
```

## Design rules

- **No shell, ever.** Every external command is an argv array; nothing user-supplied is re-parsed.
- **`gh` is the only backend** in the MVP; the app never holds a token.
- **Worktrees live outside the clone** (`$XDG_DATA_HOME/gtasks/worktrees/`), and
  "does this worktree exist" is answered only by `git worktree list --porcelain`.
- **The agent runs with `--ignore-rules` by default** (repo-committed instruction files cannot steer it)
  and never with `--yolo`.
- **`config.toml` is hand-edited, `state.json` is not.** Repository checkboxes change with one
  keypress, so the selection lives in the state file (0600, atomic rename) — never in the config
  a person edits by hand.

## License

MIT — see [LICENSE](LICENSE).
