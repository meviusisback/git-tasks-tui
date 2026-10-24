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

A flat scan of `argv.args` cannot work: it sees *values* too. The first draft of this
section did exactly that, and implemented literally it refuses `body = "-admin"` — the very case
rule 3 says must be accepted. Measured, with the two functions as written (no dependencies, `rustc`):

```
$ ./check          # ["comment", "--repo", "o/r", "--body", "-admin"]
REFUSED   <- flag "-admin" is not allowlisted for this action
```

So the distinction has to be a **type**, not a convention:

```rust
enum Token {
    Flag(&'static str),   // must be in FLAGS[action]
    Value(OsString),      // opaque: never inspected as a flag
}

pub fn argv_for(b: &Binaries, action: &Action) -> anyhow::Result<Argv> {
    let tokens: Vec<Token> = match action { /* only Token::Flag from FLAGS, only Token::Value for values */ };
    ensure_allowlisted(&tokens, flags_for(action))?;
    Ok(argv_from(b, repo_path(action), tokens))
}

fn ensure_allowlisted(tokens: &[Token], allowed: &[&str]) -> anyhow::Result<()> {
    for token in tokens {
        if let Token::Flag(flag) = token {
            ensure!(allowed.contains(flag), "flag {flag:?} is not allowlisted for this action");
        }
    }
    Ok(())
}
```

Now a value can be neither refused nor accepted by mistake, and every value is emitted together
with its flag by construction, so `["--body"]` with no value cannot be built at all.

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
   variant is a compile error until it declares its list) and assert every `Token::Flag` is in
   `FLAGS[action]`, and that no `Token::Value` is ever inspected.
2. `no_action_emits_a_forbidden_flag` — the control: for each action, injecting `--admin` into the
   args must be refused. Without this the first test passes on an empty list.
3. `a_value_stays_a_value` — `body = "-admin"` must be **accepted and passed as a value** (measured:
   `git commit -m "-x --admin"` records it literally); the counterpart, `a_positional_cannot_be_a_flag`
   — a branch named `-dfdf` must be refused by the type, not by a string inspection.
   Plus `a_missing_value_cannot_shift_a_flag`: building `["--body"]` without a value is a construction
   error, not an argv whose next flag slides into the value slot.
4. `destructive_actions_require_confirmation` — allowlist prevents *injection*, not *intent*: the app
   must still ask (the existing `Modal::Confirm`), and that is a separate test.

## What NOT to do

- Don't add flags to the lists "just in case". The list is the security boundary: every element is a
  decision someone reviewed.
- Don't let `Argv` carry a "permissions" field: a `pub` field can be forged by any test or future
  caller, which reintroduces exactly the hole `Span::plain` had.
- Don't rely on `gh` rejecting a flag. `gh pr merge -a` fails today, but the day it becomes an alias
  for `--auto` the argv is already built and valid.

## A type constrains only if its constructor is the only door

`Branch(String)` with a `pub` field can be built as `Branch("-dfdf")`: the typed positional becomes a
string again, and `a_positional_cannot_be_a_flag` passes while the real code builds the branch by hand.
The invariant must live in the constructor, not at the call site:

```rust
pub struct Branch(String);                 // private field
impl Branch {
    pub fn parse(s: &str) -> anyhow::Result<Self> { /* check-ref-format + charset rules */ }
}
```

Same lesson as `Span::plain` and the "permissions" field we refused to put in `Argv`: if the type has a
public field, the check is a convention that the next caller can skip.

## `RepoSlug` and the common flag list (refinement)

`Action` should carry a `RepoSlug`, not a `String`:

```rust
pub struct RepoSlug(String);                  // private field: one door
impl RepoSlug { pub fn parse(s: &str) -> anyhow::Result<Self> { /* config::validate_repo_slug */ } }
```

Security-wise this is neutral (an invalid slug is refused today in `tokens_for`); design-wise it is
positive: the decision moves into `RepoSlug::parse`, so there is **one** decision instead of four.

`--repo` stays out of the per-action lists, but only because the **constructor** emits it from the
typed value. That keeps the check honest: the prefix is outside the allowlist because a type with one door
produces it, not because we trust it.

```rust
fn gh_action(b: &Binaries, slug: &RepoSlug, action: &Action) -> anyhow::Result<Argv> {
    let tokens = tokens_for(action)?;
    ensure_allowlisted(&tokens, flags_for(action))?;   // only the action's own flags
    let mut args: Vec<OsString> = vec!["--repo".into(), slug.as_str().into()];
    args.extend(tokens.into_iter().map(Token::into_os));
    Ok(gh(b, &cwd, args))
}
```

Fail-closed is preserved: an action token that names `--repo` is not in `flags_for(action)` and is refused.
This replaced an earlier `GH_COMMON` constant, which kept the flag in a list and merely deduplicated it.

