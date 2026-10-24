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

# H-3: the same treatment for the tag guard. A `pre-push` that accepts
# everything is worse than no hook at all, because its presence is taken as proof
# that pushes are gated — and the `for` loop above only ever checked the bit.
# Refuse-direction: a sha that is not a commit cannot carry a tag, so the hook
# MUST refuse it. This direction does not depend on the state of the repository.
untagged=1234567890123456789012345678901234567890
if printf 'refs/heads/main %s refs/heads/main %s\n' "$untagged" "$untagged" \
     | sh .githooks/pre-push >/dev/null 2>&1; then
    echo "install-hooks: pre-push ACCEPTS an untagged commit — the tag guard is not running" >&2
    exit 1
fi
# Accept-direction: a commit that does carry a review tag must pass. The tag is
# temporary and removed again, so the installer leaves no trace in the repository.
probe_tag="review/selftest-$$"
git tag "$probe_tag" HEAD 2>/dev/null || true
accepted=0
printf 'refs/heads/main %s refs/heads/main %s\n' "$(git rev-parse HEAD)" "$(git rev-parse HEAD)" \
     | sh .githooks/pre-push >/dev/null 2>&1 && accepted=1
git tag -d "$probe_tag" >/dev/null 2>&1 || true
[ "$accepted" = 1 ] || {
    echo "install-hooks: pre-push REFUSES a tagged commit — it is not the right file" >&2
    exit 1
}

# Printed AFTER the last check that can fail: an installer that announces success
# and then exits 1 is the same inversion it exists to prevent.
echo "hooks installed and verified: core.hooksPath=$(git config --get core.hooksPath)"
echo "pre-push verified: refuses an untagged commit, accepts a tagged one"
