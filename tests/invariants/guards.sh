#!/usr/bin/env bash
# guards.sh — the project's own guards, exercised the way **git** exercises them.
#
# `pre-push` and `commit-msg` police every push and every commit, and until now their only
# witness was a conversation: the cases were measured by hand once, pasted into a message,
# and never run again. A guard whose proof lives in a chat log rots silently — the next
# edit to a `grep` or a `case` can turn it into a guard that always passes, and the symptom
# would be *nothing happening*, which nobody reports.
#
# So this suite builds a throwaway repository pair (a bare remote and a work tree with the
# real hooks installed), lands a **tagged baseline** on main, and then pushes changes for
# real. Each case states the verdict it expects; the hook has to produce it.
#
# The accepting cases matter as much as the refusing ones: a guard that refuses everything
# would pass half of this file and be worse than no guard at all.

set -uo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

ROOT=$(git rev-parse --show-toplevel 2>/dev/null || echo "$PWD")

# A throwaway pair: bare remote + work tree with the repository's real hooks. main is
# already there and already tagged, because that is the only state in which the rule is
# interesting: the first push of a new branch is strict **by design** (no diff to inspect).
guard_fixture() {
    local d
    d=$(new_dir) || return 1
    git init -q --bare "$d/remote.git" || return 1
    git init -q -b main "$d/work" || return 1
    (
        cd "$d/work" || exit 1
        git config user.email guards@example.invalid
        git config user.name guards
        git config commit.gpgsign false
        git remote add origin "$d/remote.git"
        mkdir -p .githooks scripts
        cp "$ROOT/.githooks/commit-msg" .githooks/ || exit 1
        cp "$ROOT/.githooks/pre-push" .githooks/ || exit 1
        cp "$ROOT/scripts/check-claims.sh" scripts/ || exit 1
        chmod +x .githooks/commit-msg .githooks/pre-push
        git config core.hooksPath "$d/work/.githooks"

        # Baseline: the hooks and the script are committed (a claim needs its witness IN
        # the commit), tagged, and published — so main exists on the remote with a tag.
        mkdir -p src
        printf 'baseline\n' > src/main.rs
        git add -A >/dev/null 2>&1
        git commit -q --no-verify -m "baseline" >/dev/null 2>&1 || exit 1
        git tag review/baseline >/dev/null 2>&1 || exit 1
        # `--no-verify`: the fixture must not depend on the subject under test. With the
        # guard refusing everything, a baseline pushed through it would fail the *fixture*
        # and hide which case caught the mutation.
        git push -q --no-verify origin review/baseline >/dev/null 2>&1 || exit 1
        git push -q --no-verify origin HEAD:main >/dev/null 2>&1 || exit 1
    ) || { rm -rf "$d"; return 1; }
    printf '%s\n' "$d"
}

# Commit a change described as `path` arguments, then push HEAD:main.
# Prints the push exit code; stderr lands in $d/push.err.
guard_push() { # guard_push <dir> <paths...>
    local d=$1; shift
    local p
    for p in "$@"; do
        mkdir -p "$d/work/$(dirname "$p")"
        printf 'changed\n' > "$d/work/$p"
    done
    (
        cd "$d/work" || exit 1
        git add -A >/dev/null 2>&1
        git commit -q --no-verify -m "case" >/dev/null 2>&1
    )
    ( cd "$d/work" && git push -q origin HEAD:main >/dev/null 2>"$d/push.err" )
    printf '%s' "$?"
}

guard_publish_tag() { # guard_publish_tag <dir> <tag>
    ( cd "$1/work" && git tag "$2" >/dev/null 2>&1 && git push -q origin "$2" >/dev/null 2>&1 )
}

guards_pre_push() {
    group "GUARD pre-push — refuses code without a published tag, accepts what it should"
    local d rc said

    # 1. code, no tag on the new commit → refused
    d=$(guard_fixture) || { fail "fixture" "cannot build the throwaway pair"; return; }
    rc=$(guard_push "$d" "src/other.rs")
    said=$(head -2 "$d/push.err" | tr '\n' ' ')
    assert_eq "code without a tag is refused" "1" "$rc"
    assert_contains "and names the rule" "$said" "no review tag"
    rm -rf "$d"

    # 2. code, tag published → accepted
    d=$(guard_fixture) || { fail "fixture" "cannot build the throwaway pair"; return; }
    ( cd "$d/work" && printf 'v2\n' > src/other.rs && mkdir -p src && git add -A >/dev/null 2>&1 \
        && git commit -q --no-verify -m case >/dev/null 2>&1 )
    guard_publish_tag "$d" "review/case-2"
    rc=$( cd "$d/work" && git push -q origin HEAD:main >/dev/null 2>"$d/push.err"; printf '%s' "$?" )
    assert_eq "code with a published review tag is accepted" "0" "$rc"
    [ "$rc" = 0 ] || pass "  (stderr: $(head -1 "$d/push.err"))"
    rm -rf "$d"

    # 3. a local tag that was never published is not a tag (H-4)
    d=$(guard_fixture) || { fail "fixture" "cannot build the throwaway pair"; return; }
    ( cd "$d/work" && printf 'v3\n' > src/other.rs && mkdir -p src && git add -A >/dev/null 2>&1 \
        && git commit -q --no-verify -m case >/dev/null 2>&1 && git tag review/never-pushed )
    rc=$( cd "$d/work" && git push -q origin HEAD:main >/dev/null 2>"$d/push.err"; printf '%s' "$?" )
    assert_eq "a tag that never left the machine is refused" "1" "$rc"
    rm -rf "$d"

    # 4. docs only, no tag → accepted
    d=$(guard_fixture) || { fail "fixture" "cannot build the throwaway pair"; return; }
    rc=$(guard_push "$d" "docs/notes.md")
    assert_eq "a docs-only push needs no tag" "0" "$rc"
    [ "$rc" = 0 ] || pass "  (stderr: $(head -1 "$d/push.err"))"
    rm -rf "$d"

    # 5. docs + code in the same push → strict
    d=$(guard_fixture) || { fail "fixture" "cannot build the throwaway pair"; return; }
    rc=$(guard_push "$d" "docs/notes.md" "src/lib.rs")
    assert_eq "docs alongside code does not exempt the push" "1" "$rc"
    rm -rf "$d"

    # 6. a path that merely contains docs/ is not a docs path
    d=$(guard_fixture) || { fail "fixture" "cannot build the throwaway pair"; return; }
    rc=$(guard_push "$d" "src/docs/sneaky.rs")
    assert_eq "src/docs/x.rs is code, not documentation" "1" "$rc"
    rm -rf "$d"

    # 7. an empty index is required: what you prepared must be what you push
    d=$(guard_fixture) || { fail "fixture" "cannot build the throwaway pair"; return; }
    ( cd "$d/work" && mkdir -p src && printf 'staged\n' > src/staged.rs && git add src/staged.rs >/dev/null 2>&1 )
    rc=$( cd "$d/work" && git push -q origin HEAD:main >/dev/null 2>"$d/push.err"; printf '%s' "$?" )
    said=$(head -1 "$d/push.err")
    assert_eq "a dirty index is refused" "1" "$rc"
    assert_contains "and the message says what to do" "$said" "the index is not empty"
    rm -rf "$d"
}

guards_installer() {
    group "GUARD install-hooks — installs, and proves itself behaviourally"
    local d out rc hp
    d=$(new_dir) || { fail "fixture" "cannot create a temp dir"; return; }
    git init -q -b main "$d" || { fail "fixture" "cannot init"; rm -rf "$d"; return; }
    (
        cd "$d" || exit 1
        git config user.email guards@example.invalid
        git config user.name guards
        mkdir -p .githooks scripts
        cp "$ROOT/.githooks/commit-msg" .githooks/ || exit 1
        cp "$ROOT/.githooks/pre-push" .githooks/ || exit 1
        cp "$ROOT/scripts/check-claims.sh" scripts/ || exit 1
        cp "$ROOT/scripts/install-hooks.sh" scripts/ || exit 1
        # The installer's own self-test cites `.githooks/commit-msg`, and a claim needs its
        # witness in the commit: without this commit the installer would report a false
        # negative and be right to.
        git add -A >/dev/null 2>&1
        git commit -q --no-verify -m baseline >/dev/null 2>&1 || exit 1
    )
    out=$( cd "$d" && sh scripts/install-hooks.sh 2>&1 ); rc=$?
    assert_eq "the installer exits 0" "0" "$rc"
    assert_contains "and it verifies its own work behaviourally" "$out" "verified"
    hp=$( cd "$d" && git config core.hooksPath )
    assert_contains "core.hooksPath now points at .githooks" "$hp" ".githooks"
    rm -rf "$d"
}

guards_commit_msg() {
    group "GUARD commit-msg — the citation battery (one implementation, in lib.sh)"
    if check_claim_verdicts "$ROOT" 2>/tmp/claims.err; then
        pass "nine claim cases agree with the hook"
    else
        fail "the citation battery" "$(head -2 /tmp/claims.err | tr '\n' ' ')"
    fi
}

main() {
    have git || { echo "git required" >&2; exit 2; }
    guards_pre_push
    guards_installer
    guards_commit_msg
    summary
}

main "$@"
