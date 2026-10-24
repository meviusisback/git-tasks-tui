#!/bin/sh
# One command per clone: point git at the versioned hooks directory.
#
# H-2: an earlier version of this comment blamed relative paths in linked
# worktrees. Measured (git 2.56): a **relative** `core.hooksPath` works in a
# worktree too — in a non-bare repository git runs hooks from the root of the
# working tree. The silent skip came from the **executable bit**: git ignores a
# hook that is not executable and only prints a hint, so "installed" and
# "running" are different states. The `test -x` below is what makes the claim
# checkable instead of promised.
set -eu
top=$(git rev-parse --show-toplevel); cd "$top"
# Absolute for clarity, not necessity: relative works, as measured.
git config core.hooksPath "$top/.githooks"
chmod +x .githooks/* 2>/dev/null || true
for hook in commit-msg pre-push; do
    if [ ! -x ".githooks/$hook" ]; then
        echo "install-hooks: .githooks/$hook is not executable — git would skip it" >&2
        exit 1
    fi
done

# H-2b: `test -x` checks a state, and a state can be restored by the `chmod`
# above. This checks the **behaviour**: the hook must accept a true claim and
# refuse a false one. Same reason probe 1b beats probe 1 — look at what the
# thing does, not at what the file looks like.
probe=$(mktemp)
trap 'rm -f "$probe"' EXIT INT TERM
printf 'probe\n\nClaims: .githooks/commit-msg contains "rev-parse"\n' > "$probe"
if ! sh .githooks/commit-msg "$probe" >/dev/null 2>&1; then
    echo "install-hooks: the hook REFUSES a true claim — it is not the right file" >&2
    exit 1
fi
printf 'probe\n\nClaims: .githooks/commit-msg contains "no-such-text-zzz"\n' > "$probe"
if sh .githooks/commit-msg "$probe" >/dev/null 2>&1; then
    echo "install-hooks: the hook ACCEPTS a false claim — it is not running the check" >&2
    exit 1
fi

# H-3: the same distinction for `pre-push`. A hook that is present, executable
# and **mute** is worse than a missing one: the ledger could cite it as a
# guarantee. Two directions again — an untagged commit must be refused, a tagged
# one must pass. The untagged sha does not exist on purpose: `git tag
# --points-at` prints nothing for it, so the verdict cannot depend on repo state.
untagged=1234567890123456789012345678901234567890
if printf 'refs/heads/main %s refs/heads/main %s\n' "$untagged" "$untagged" \
    | sh .githooks/pre-push >/dev/null 2>&1; then
    echo "install-hooks: pre-push ACCEPTS an untagged commit — the tag guard is not running" >&2
    exit 1
fi

selftest_tag="review/install-selftest-$$"
head_sha=$(git rev-parse HEAD)
git tag -f "$selftest_tag" "$head_sha" >/dev/null 2>&1 || true
if ! printf 'refs/heads/main %s refs/heads/main %s\n' "$head_sha" "$head_sha" \
    | sh .githooks/pre-push >/dev/null 2>&1; then
    git tag -d "$selftest_tag" >/dev/null 2>&1 || true
    echo "install-hooks: pre-push REFUSES a tagged commit — the guard refuses everything" >&2
    exit 1
fi
git tag -d "$selftest_tag" >/dev/null 2>&1 || true

echo "hooks installed and verified: commit-msg answers both ways, pre-push refuses an untagged commit and accepts a tagged one"
