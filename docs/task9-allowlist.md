<!-- Agreed design, written by the security auditor before any Task 9 code exists.
     Status: ACCEPTED — implementation pending (Task 9). The lists below were measured
     from `gh 2.103.0 --help` on the dev machine, not recalled. -->

# Task 9 — flag allowlist: the shape to write before the code

Goal: make "an argv the app never intended" **impossible to construct**, not "unlikely to pass a
deny-list". A deny-list fails open the moment `gh` adds a shorthand; an allowlist fails closed.

## The three rules

1. **One translation point per action.** `enum Action` + exactly one function
   `fn argv_for(&Binaries, &Action) -> anyhow::Result<Argv>`. No other code path may produce a
   `gh`/`git` argv for an action. `fetch` keeps its own (it is a read, already reviewed).
2. **Positive allowlist per action, declared next to the action**, taken from the real binary
   (below: `gh 2.103.0`), never from memory.
3. **Values are separate elements, and positionals are typed.** `["--body", text]`, never
   `["--body=text"]`. A value bound to its flag is consumed as a value **even when it starts with
   `-`** (measured: `git commit -m "-x --admin"` records the literal message), so a comment beginning
   with a bullet is fine and must not be refused. What must never happen is a *positional* starting
   with `-`: `git check-ref-format --branch -x` → rc 128, i.e. the parser takes it for a flag. Hence
   typed positionals (table below), not "is this string safe".

## Per-action allowlist (measured from `gh --help`)

```rust
enum Action {
    Merge  { pr: u64, method: MergeMethod, delete_branch: bool },
    Comment{ item: ItemRef, body: String },
    Close  { item: ItemRef, reason: Option<CloseReason>, comment: Option<String> },
    Checkout { pr: u64 },       // gh pr checkout, into the managed worktree
}

const MERGE_FLAGS:  &[&str] = &["--repo", "--squash", "--merge", "--rebase", "--delete-branch"];
const COMMENT_FLAGS:&[&str] = &["--repo", "--body"];
const CLOSE_FLAGS:  &[&str] = &["--repo", "--reason", "--comment"];
const CHECKOUT_FLAGS:&[&str]=&["--repo", "--branch", "--detach"];

// Deliberately ABSENT from every list, and a test asserts they are:
//   --admin, --auto, --disable-auto, --force, -f, --body-file/-F, --editor/-e,
//   --web/-w, --attach, --delete-last, --edit-last, --worktree,
//   --author-email/-A, --subject/-t, --match-head-commit, --recurse-submodules
```

`--repo` is in every list because it is how the app targets the repository it displays, and its value
is a slug validated by `config::validate_repo_slug` (already exists).

## The check that makes it a guarantee

```rust
pub fn argv_for(b: &Binaries, action: &Action) -> anyhow::Result<Argv> {
    let (flags, args) = match action { /* build from the list only */ };
    let argv = git_or_gh(b, repo_path(action), args);
    ensure_allowlisted(&argv, flags)?;   // every token starting with '-' is in `flags`
    Ok(argv)
}

fn ensure_allowlisted(argv: &Argv, allowed: &[&str]) -> anyhow::Result<()> {
    for arg in &argv.args {
        let text = arg.to_string_lossy();
        if !text.starts_with('-') { continue; }
        ensure!(allowed.contains(&text.as_str()), "flag {text:?} is not allowlisted for this action");
    }
    Ok(())
}
```

Note the direction: `ensure_allowlisted` validates the argv the code *just built*, so it is not a
filter a caller can forget — it runs on the only path that exists. Keep `check_argv`'s deny-list in
`run_pipe_with_caps` as a second line (it catches a future call site that bypasses `argv_for`), but
the guarantee must not depend on it.

## Positionals get types, not strings

| positional | type | validation |
|---|---|---|
| PR/issue number | `u64` | formatted from the integer; a string never reaches argv |
| repo | `RepoSlug` | `config::validate_repo_slug` (charset, no `..`, no leading `-`) |
| branch | `Branch` | `git check-ref-format --branch` **or** the existing charset+dot rules in `worktree::resolve_in` |
| body/comment | `String` | rendered through `sanitize`; **may** start with `-` (it is a value bound to its flag, measured safe); reject only NUL |

## Tests that make the class unreachable

1. `every_action_argv_is_allowlisted` — iterate the `Action` enum (exhaustive `match`, so a new
   variant is a compile error until it declares its list) and assert `argv ⊂ FLAGS[action]`.
2. `no_action_emits_a_forbidden_flag` — the control: for each action, injecting `--admin` into the
   args must be refused. Without this the first test passes on an empty list.
3. `a_value_stays_a_value` — `body = "-admin"` must be **accepted and passed as a value** (measured:
   `git commit -m "-x --admin"` records it literally); the counterpart, `a_positional_cannot_be_a_flag`
   — a branch named `-dfdf` must be refused by the type, not by a string inspection.
4. `destructive_actions_require_confirmation` — allowlist prevents *injection*, not *intent*: the app
   must still ask (the existing `Modal::Confirm`), and that is a separate test.

## What NOT to do

- Don't add flags to the lists "just in case". The list is the security boundary: every element is a
  decision someone reviewed.
- Don't let `Argv` carry a "permissions" field: a `pub` field can be forged by any test or future
  caller, which reintroduces exactly the hole `Span::plain` had.
- Don't rely on `gh` rejecting a flag. `gh pr merge -a` fails today, but the day it becomes an alias
  for `--auto` the argv is already built and valid.
